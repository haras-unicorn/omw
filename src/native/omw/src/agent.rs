//! Agent bootstrap. Builds the shared provider/tooling/bus registries plus the
//! optional endpoint HTTP server, then runs every configured agent for one
//! iteration (`run`) or loops it (`loop`, restarting on failure with exponential
//! backoff).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use futures_util::future::join_all;
use tokio::net::TcpListener;

use crate::config::{AgentConfig, Config};
use crate::endpoint::{ServerState, router};
use crate::host::bus::MessageBus;
use crate::host::ctx::AgentContext;
use crate::host::endpoint::EndpointRegistry;
use crate::host::streams::{CancelRegistry, StreamRegistry};
use crate::provider::build_registry as build_providers;
use crate::runtime::RunOutcome;
use crate::tooling::build_registry as build_tooling;

/// Run every configured agent once, then aggregate their results.
pub async fn run_agents(cfg: &Config) -> anyhow::Result<()> {
  let shared = Arc::new(Shared::build(cfg).await?);
  collect_agent_results(
    join_all(cfg.agents.iter().map(|agent| {
      let config = cfg.clone();
      let agent = agent.clone();
      let shared = Arc::clone(&shared);
      tokio::spawn(async move { run_agent(&config, &agent, &shared).await })
    }))
    .await
    .into_iter()
    .map(|task| match task {
      Ok(result) => result,
      Err(join_error) => Err(anyhow::Error::from(join_error)),
    })
    .collect(),
  )
}

/// Run every configured agent in a loop forever, restarting immediately on
/// success and with exponential backoff (100ms doubling up to a 30s cap) on
/// failure so a wedged agent does not spin the CPU.
pub async fn loop_agents(cfg: &Config) -> anyhow::Result<()> {
  let shared = Arc::new(Shared::build(cfg).await?);
  join_all(cfg.agents.iter().map(|agent| {
    let config = cfg.clone();
    let agent = agent.clone();
    let shared = Arc::clone(&shared);
    tokio::spawn(async move {
      let cap = Duration::from_secs(30);
      let mut delay = Duration::from_millis(100);
      loop {
        match run_agent(&config, &agent, &shared).await {
          Ok(outcome) => {
            tracing::info!(agent = %agent.name, ?outcome, "agent iteration completed");
            delay = Duration::from_millis(100);
          }
          Err(error) => {
            tracing::error!(agent = %agent.name, error = %error, "agent iteration failed");
            tracing::debug!(
              agent = %agent.name,
              delay_ms = delay.as_millis(),
              "backing off before retrying the agent"
            );
            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2).min(cap);
          }
        }
      }
    })
  }))
  .await;
  Ok(())
}

/// Build a fresh [`AgentContext`] for one agent and run its brain once.
/// Returns the terminal outcome so a loop can log it and restart.
async fn run_agent(
  config: &Config,
  agent: &AgentConfig,
  shared: &Shared,
) -> anyhow::Result<RunOutcome> {
  let impl_cfg = config.runtime.get(&agent.runtime).with_context(|| {
    format!(
      "agent {:?} references unknown runtime {:?}",
      agent.name, agent.runtime
    )
  })?;
  let runtime =
    crate::runtime::build(&agent.runtime, &impl_cfg.kind, &impl_cfg.params)?;
  let ctx = AgentContext::new(
    agent.name.clone(),
    PathBuf::from(&agent.script),
    shared.providers.clone(),
    shared.tooling.clone(),
    Arc::clone(&shared.bus),
    Arc::new(StreamRegistry::new()),
    Arc::new(CancelRegistry::new()),
    Arc::new(CancelRegistry::new()),
    Arc::new(CancelRegistry::new()),
    shared.endpoint_registry.clone(),
  )?;
  tracing::info!(agent = %agent.name, runtime = %agent.runtime, "agent iteration starting");
  let outcome = runtime.runtime.run(&ctx).await;
  match &outcome {
    Ok(RunOutcome::Completed) => {
      tracing::info!(agent = %agent.name, "agent run completed")
    }
    Ok(RunOutcome::Exited(message)) => {
      tracing::info!(agent = %agent.name, message = %message, "agent exited")
    }
    Err(error) => {
      tracing::error!(agent = %agent.name, error = %error, "agent run failed")
    }
  }
  outcome
}

/// Aggregate the results of every agent task into one, erroring if any of
/// them failed.
fn collect_agent_results(
  results: Vec<anyhow::Result<RunOutcome>>,
) -> anyhow::Result<()> {
  let mut errors: Vec<String> = Vec::new();
  for result in results {
    if let Err(error) = result {
      errors.push(error.to_string());
    }
  }
  if errors.is_empty() {
    Ok(())
  } else {
    Err(anyhow::anyhow!(
      "{} agent run(s) failed: {}",
      errors.len(),
      errors.join("; ")
    ))
  }
}

/// The process-level registries shared by every agent in this process.
struct Shared {
  providers: HashMap<String, crate::provider::ProviderEntry>,
  tooling: HashMap<String, crate::tooling::ToolingEntry>,
  bus: Arc<MessageBus>,
  endpoint_registry: Option<Arc<EndpointRegistry>>,
  endpoint_task: Option<tokio::task::JoinHandle<()>>,
}

impl Shared {
  async fn build(cfg: &Config) -> anyhow::Result<Self> {
    let providers = build_providers(cfg)?;
    let tooling = build_tooling(cfg).await?;
    let bus = Arc::new(MessageBus::new());
    let (endpoint_registry, endpoint_task) = if let Some(endpoint) =
      &cfg.endpoint
    {
      let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
      let addr = endpoint.listen.parse::<SocketAddr>().with_context(|| {
        format!(
          "endpoint listen address {:?} is not a valid socket address",
          endpoint.listen
        )
      })?;
      let listener = TcpListener::bind(addr).await.with_context(|| {
        format!("failed to bind endpoint listener on {}", endpoint.listen)
      })?;
      let app =
        router(ServerState::new(Arc::clone(&bus), Arc::clone(&registry)));
      let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
          tracing::error!(error = %error, "endpoint server failed");
        }
      });
      (Some(registry), Some(task))
    } else {
      (None, None)
    };
    Ok(Self {
      providers,
      tooling,
      bus,
      endpoint_registry,
      endpoint_task,
    })
  }
}

impl Drop for Shared {
  fn drop(&mut self) {
    if let Some(task) = self.endpoint_task.take() {
      task.abort();
      tracing::info!("endpoint server stopped");
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn collect_agent_results_succeeds_when_all_succeed() -> anyhow::Result<()> {
    let results = vec![
      Ok(RunOutcome::Completed),
      Ok(RunOutcome::Exited("bye".to_string())),
    ];
    collect_agent_results(results)?;
    Ok(())
  }

  #[test]
  fn collect_agent_results_aggregates_errors() {
    let results = vec![
      Err(anyhow::anyhow!("first agent failed")),
      Err(anyhow::anyhow!("second agent failed")),
    ];
    let error = collect_agent_results(results).unwrap_err();
    assert!(error.to_string().contains("2 agent run(s) failed"));
    assert!(error.to_string().contains("first agent failed"));
    assert!(error.to_string().contains("second agent failed"));
  }

  #[test]
  fn collect_agent_results_reports_join_errors() {
    let results = vec![Err(anyhow::anyhow!("agent task joined with an error"))];
    let error = collect_agent_results(results).unwrap_err();
    assert!(
      error
        .to_string()
        .contains("agent task joined with an error")
    );
  }
}
