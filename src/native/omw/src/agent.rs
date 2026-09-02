//! Agent bootstrap: turns a parsed [`Config`] into concrete agent runtimes and
//! runs one iteration. Kept out of `main.rs` so the binary entrypoint stays a
//! thin shell.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::join_all;

use crate::config::{AgentConfig, Config};
use crate::host::bus::MessageBus;
use crate::host::ctx::AgentContext;
use crate::host::streams::StreamRegistry;
use crate::provider::{ProviderEntry, build_registry as build_providers};
use crate::tooling::{ToolingEntry, build_registry as build_tooling};

/// Build the runtime for the configured agents and run a single iteration.
pub async fn run_agents(cfg: &Config) -> anyhow::Result<()> {
  let shared = Arc::new(Shared::build(cfg).await?);
  collect_agent_results(
    join_all(cfg.agents.iter().map(|agent| {
      let config = cfg.clone();
      let agent = agent.clone();
      let shared = Arc::clone(&shared);
      tokio::spawn(async move { run_agent(&config, &agent, &shared).await })
    }))
    .await,
  )
}

/// Build the runtime for the configured agents and loop them forever, with
/// exponential backoff on failures so a wedged agent does not spin the CPU.
pub async fn loop_agents(cfg: &Config) -> anyhow::Result<()> {
  let shared = Arc::new(Shared::build(cfg).await?);
  let _: Vec<_> = join_all(cfg.agents.iter().map(|agent| {
    let config = cfg.clone();
    let agent = agent.clone();
    let shared = Arc::clone(&shared);
    tokio::spawn(async move {
      let cap = std::time::Duration::from_secs(30);
      let mut delay = std::time::Duration::from_millis(100);
      loop {
        let result = run_agent(&config, &agent, &shared).await;
        if let Err(error) = result {
          tracing::error!(error = %error, "agent {} failed", agent.name);
          tracing::debug!(
            agent = %agent.name,
            delay_ms = delay.as_millis(),
            "backing off before retrying the agent"
          );
          tokio::time::sleep(delay).await;
          delay = delay.saturating_mul(2).min(cap);
        } else {
          delay = std::time::Duration::from_millis(100);
        }
      }
    })
  }))
  .await;
  Ok(())
}

/// Build the per-agent runtime and run it for one iteration.
async fn run_agent(
  cfg: &Config,
  agent: &AgentConfig,
  shared: &Shared,
) -> anyhow::Result<()> {
  let span = tracing::info_span!(
    "run_agent",
    agent = %agent.name,
    runtime = %agent.runtime,
    script = %agent.script,
  );
  let _entered = span.enter();
  tracing::info!("agent run starting");

  let ctx = AgentContext::new(
    agent.name.clone(),
    PathBuf::from(&agent.script),
    shared.providers.clone(),
    shared.tooling.clone(),
    Arc::clone(&shared.bus),
    Arc::new(StreamRegistry::new()),
  )?;

  let runtime =
    crate::runtime::build(&agent.runtime, cfg.runtime.get(&agent.runtime))?;
  let outcome = runtime.run(&ctx).await.inspect_err(|e| {
    tracing::error!(agent = %agent.name, error = %e, "agent run failed");
  })?;
  tracing::info!(?outcome, agent = %agent.name, "agent run finished");

  Ok(())
}

/// Collect all agent results into a single anyhow::Result
fn collect_agent_results(
  results: Vec<Result<Result<(), anyhow::Error>, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
  let errors: Vec<anyhow::Error> = results
    .into_iter()
    .filter_map(|r| match r {
      Ok(Ok(())) => None,
      Ok(Err(e)) => Some(e),
      Err(join_err) => Some(anyhow::anyhow!(join_err)),
    })
    .collect();

  if errors.is_empty() {
    Ok(())
  } else {
    Err(anyhow::anyhow!(
      "{} agent(s) failed: {}",
      errors.len(),
      errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
    ))
  }
}

/// Shared, per-process state: the registries of configured provider/tooling
/// instances plus the inter-agent message bus.
struct Shared {
  providers: HashMap<String, ProviderEntry>,
  tooling: HashMap<String, ToolingEntry>,
  bus: Arc<MessageBus>,
}

impl Shared {
  /// Build the provider/tooling registries and the shared message bus once.
  async fn build(cfg: &Config) -> anyhow::Result<Self> {
    let providers = build_providers(cfg)?;
    let tooling = build_tooling(cfg).await?;
    let bus = Arc::new(MessageBus::new());
    Ok(Self {
      providers,
      tooling,
      bus,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn collect_agent_results_succeeds_when_all_succeed() {
    let results: Vec<
      Result<Result<(), anyhow::Error>, tokio::task::JoinError>,
    > = std::iter::repeat_with(|| Ok(Ok(()))).take(3).collect();
    assert!(collect_agent_results(results).is_ok());
  }

  #[test]
  fn collect_agent_results_aggregates_errors() -> anyhow::Result<()> {
    let results =
      vec![Ok(Ok(())), Ok(Err(anyhow::anyhow!("agent two failed")))];
    let err = match collect_agent_results(results) {
      Ok(()) => anyhow::bail!("expected an aggregated error"),
      Err(e) => e,
    };
    assert!(err.to_string().contains("1 agent(s) failed"), "{err}");
    Ok(())
  }

  #[test]
  fn collect_agent_results_reports_join_errors() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()?;
    let handle = rt.spawn(async {});
    handle.abort();
    let join_err = match rt.block_on(handle) {
      Ok(()) => anyhow::bail!("expected a join error"),
      Err(e) => e,
    };

    let results = vec![Ok(Ok(())), Err(join_err)];
    let err = match collect_agent_results(results) {
      Ok(()) => anyhow::bail!("expected an aggregated error"),
      Err(e) => e,
    };
    assert!(err.to_string().contains("1 agent(s) failed"), "{err}");
    Ok(())
  }
}
