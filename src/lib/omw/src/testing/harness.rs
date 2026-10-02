//! The live asserted-run harness.
//!
//! Drives a config through the controlled run path against the in-config
//! `kind = "mock"` doubles, consumes the trace as it happens, and stops each
//! `outcome = "asserted"` agent as soon as its assertions settle. Non-asserted
//! agents are checked when their terminal outcome arrives. Once every listed
//! agent has a verdict the harness forces the whole run down, so a brain that
//! loops or blocks cannot hang a test.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::agent::{Backends, Registries, StopRegistry, run_agents_controlled};
use crate::config::Config;
use crate::host::trace::TraceEvent;
use crate::runtime::RunOutcome;
use crate::shutdown::Shutdown;

use super::assert::{AgentAssertion, Assertions, Matcher, OutcomeAssertion};

/// The result of one asserted run.
#[derive(Debug, Clone)]
pub struct Report {
  /// One entry per agent named in `[assertions]`, keyed by agent name.
  pub agents: BTreeMap<String, AgentReport>,
  /// A failure of the run itself (bootstrap, join, or a brain error), if any.
  pub error: Option<String>,
  /// Scripted queue state of the mock back ends at the end of the run, for
  /// failure diagnostics. Real back ends contribute nothing.
  pub snapshots: Snapshots,
}

impl Report {
  /// Whether every asserted agent passed and the run itself did not fail.
  pub fn passed(&self) -> bool {
    self.error.is_none() && self.agents.values().all(|agent| agent.passed)
  }
}

/// The mock back ends' scripted queue state, keyed by configured name, as
/// reported by [`Provider::snapshot`](crate::provider::Provider::snapshot) and
/// friends. A real back end contributes no entry.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Snapshots {
  /// Configured provider name -> its snapshot.
  pub providers: BTreeMap<String, Value>,
  /// Configured tooling name -> its snapshot.
  pub tooling: BTreeMap<String, Value>,
  /// The endpoint's snapshot, when one is configured.
  pub endpoint: Option<Value>,
}

/// One agent's assertion verdict.
#[derive(Debug, Clone)]
pub struct AgentReport {
  /// The agent this verdict is for.
  pub agent: String,
  /// Whether the agent's assertions passed.
  pub passed: bool,
  /// Whether the verdict came from `outcome = "asserted"` (stop on settle).
  pub asserted: bool,
  /// The agent's terminal outcome, when the run produced one.
  pub outcome: Option<RunOutcome>,
  /// The rendered diff when the agent failed.
  pub diff: Option<String>,
  /// Every trace event the matcher observed, in order.
  pub observed: Vec<TraceEvent>,
  /// The cursor: the index of the next unsatisfied assertion. Equal to
  /// `assertions` when the agent passed.
  pub cursor: usize,
  /// The number of event assertions.
  pub assertions: usize,
  /// Whether the verdict came from the harness timeout rather than a normal
  /// outcome or settle.
  pub timed_out: bool,
}

/// Drives one asserted run.
pub struct Harness<'a> {
  config: &'a Config,
  registries: &'a Registries,
  assertions: &'a Assertions,
}

impl<'a> Harness<'a> {
  /// A harness over `config`, `registries`, and the expected `assertions`.
  pub fn new(
    config: &'a Config,
    registries: &'a Registries,
    assertions: &'a Assertions,
  ) -> Self {
    Self {
      config,
      registries,
      assertions,
    }
  }

  /// Run every agent once, stopping `asserted` agents as their assertions
  /// settle, and return the per-agent verdicts.
  pub async fn run(&self) -> Report {
    let (tx, mut rx) = broadcast::channel(self.config.tunables.trace_buffer);
    let shutdown = Shutdown::new();
    let stops = StopRegistry::default();

    // Build the back ends here so the harness keeps handles to the exact mock
    // instances and can snapshot their scripted queues after the run.
    let built = if self.config.tunables.allow_unlocked_secrets {
      crate::secret::allow_unlocked(|| self.build_backends())
    } else {
      self.build_backends()
    };
    let built = match built {
      Ok(built) => built,
      Err(error) => {
        return Report {
          agents: BTreeMap::new(),
          error: Some(error.to_string()),
          snapshots: Snapshots::default(),
        };
      }
    };
    let provider_handles: Vec<(String, Arc<dyn crate::provider::Provider>)> =
      built
        .providers
        .iter()
        .map(|(name, entry)| (name.clone(), Arc::clone(entry.inner())))
        .collect();
    let tooling_handles: Vec<(String, Arc<dyn crate::tooling::Tooling>)> =
      built
        .tooling
        .iter()
        .map(|(name, entry)| (name.clone(), Arc::clone(entry.inner())))
        .collect();
    let endpoint_handle = built
      .endpoint
      .as_ref()
      .map(|entry| Arc::clone(entry.inner()));

    let mut states: BTreeMap<String, AgentState> = self
      .assertions
      .assertions
      .iter()
      .map(|(agent, assertion)| (agent.clone(), AgentState::new(assertion)))
      .collect();

    // Asserted agents whose assertions are already satisfied (e.g. an empty
    // `events` list) settle before the run even starts.
    for (agent, state) in states.iter_mut() {
      if state.asserted && state.matcher.matched() {
        state.settle_ok();
        stops.request_stop(agent);
        tracing::info!(target: "omw-test", agent = %agent, "assertions settled");
      }
    }

    let run = run_agents_controlled(
      self.config,
      self.registries,
      tx,
      shutdown.clone(),
      stops.clone(),
      Some(built),
    );
    let mut run = Box::pin(run);
    let mut run_result = None;
    let mut error = None;
    let mut timed_out = false;

    {
      let drive = async {
        loop {
          if !states.is_empty() && states.values().all(AgentState::settled) {
            break;
          }
          tokio::select! {
            biased;
            result = &mut run => {
              run_result = Some(result);
              break;
            }
            received = rx.recv() => match received {
              Ok(event) => observe(&mut states, &stops, event),
              Err(broadcast::error::RecvError::Lagged(missed)) => {
                error = Some(lagged_message(missed));
                break;
              }
              Err(broadcast::error::RecvError::Closed) => break,
            },
          }
        }
      };
      tokio::pin!(drive);
      match self.config.tunables.test_timeout() {
        Some(duration) => {
          if tokio::time::timeout(duration, &mut drive).await.is_err() {
            timed_out = true;
          }
        }
        None => drive.await,
      }
    }

    // Drain whatever was buffered as the run ended, so a final `Outcome` is
    // never lost to a scheduling race.
    loop {
      match rx.try_recv() {
        Ok(event) => observe(&mut states, &stops, event),
        Err(broadcast::error::TryRecvError::Lagged(missed)) => {
          error.get_or_insert_with(|| lagged_message(missed));
        }
        Err(
          broadcast::error::TryRecvError::Empty
          | broadcast::error::TryRecvError::Closed,
        ) => break,
      }
    }

    // Force any straggler down and let the run unwind.
    if run_result.is_none() {
      shutdown.request();
      run_result = Some(run.await);
    }
    if let Some(Err(run_error)) = run_result
      && error.is_none()
    {
      error = Some(run_error.to_string());
    }

    // Any listed agent without a verdict failed: the run ended (or timed out)
    // first.
    for (agent, state) in states.iter_mut() {
      if !state.settled() {
        state.timed_out = timed_out;
        let reason = if timed_out {
          format!("agent {agent:?} timed out before its assertions settled")
        } else {
          format!("the run ended before agent {agent:?}'s assertions settled")
        };
        state.fail(reason);
      }
    }

    let agents = states
      .into_iter()
      .map(|(agent, state)| {
        let report = AgentReport {
          agent: agent.clone(),
          passed: state.passed(),
          asserted: state.asserted,
          diff: state.diff(),
          outcome: state.outcome,
          observed: state.matcher.observed().to_vec(),
          cursor: state.matcher.cursor(),
          assertions: state.assertions,
          timed_out: state.timed_out,
        };
        (agent, report)
      })
      .collect();
    let snapshots = Snapshots {
      providers: provider_handles
        .iter()
        .filter_map(|(name, provider)| {
          provider.snapshot().map(|value| (name.clone(), value))
        })
        .collect(),
      tooling: tooling_handles
        .iter()
        .filter_map(|(name, tooling)| {
          tooling.snapshot().map(|value| (name.clone(), value))
        })
        .collect(),
      endpoint: endpoint_handle.and_then(|endpoint| endpoint.snapshot()),
    };
    Report {
      agents,
      error,
      snapshots,
    }
  }

  /// Build the provider/tooling/endpoint back ends from the config so the
  /// harness can hand the exact instances to the controlled run and snapshot
  /// them afterwards.
  fn build_backends(&self) -> anyhow::Result<Backends> {
    Ok(Backends {
      providers: self.registries.providers.build_entries(self.config)?,
      tooling: self.registries.tooling.build_entries(self.config)?,
      endpoint: self.registries.endpoints.build_entry(self.config)?,
    })
  }
}

/// Feed one trace event into the matching agent's state.
fn observe(
  states: &mut BTreeMap<String, AgentState>,
  stops: &StopRegistry,
  event: TraceEvent,
) {
  let agent = event.agent().to_owned();
  let Some(state) = states.get_mut(&agent) else {
    return;
  };
  match &event {
    TraceEvent::Outcome { outcome, .. } => state.on_outcome(outcome.clone()),
    _ => {
      if state.observe(&event) && state.asserted {
        stops.request_stop(&agent);
        tracing::info!(target: "omw-test", agent = %agent, "assertions settled");
      }
    }
  }
}

fn lagged_message(missed: u64) -> String {
  format!(
    "trace receiver lagged by {missed} events; raise the trace buffer size"
  )
}

/// The streaming assertion state for one agent.
struct AgentState {
  matcher: Matcher,
  outcome_assertion: Option<OutcomeAssertion>,
  /// Whether `outcome = "asserted"`: stop the agent once its events settle.
  asserted: bool,
  outcome: Option<RunOutcome>,
  /// The number of event assertions.
  assertions: usize,
  /// Whether the verdict came from the harness timeout.
  timed_out: bool,
  /// `None` until a verdict is reached.
  verdict: Option<Result<(), String>>,
}

impl AgentState {
  fn new(assertion: &AgentAssertion) -> Self {
    Self {
      matcher: Matcher::new(&assertion.events),
      outcome_assertion: assertion.outcome.clone(),
      asserted: assertion.outcome == Some(OutcomeAssertion::Asserted),
      outcome: None,
      assertions: assertion.events.len(),
      timed_out: false,
      verdict: None,
    }
  }

  /// Whether a verdict has been reached.
  fn settled(&self) -> bool {
    self.verdict.is_some()
  }

  /// Whether the reached verdict is a pass.
  fn passed(&self) -> bool {
    matches!(self.verdict, Some(Ok(())))
  }

  /// The failure diff, when the verdict is a failure.
  fn diff(&self) -> Option<String> {
    match &self.verdict {
      Some(Err(diff)) => Some(diff.clone()),
      _ => None,
    }
  }

  fn settle_ok(&mut self) {
    self.verdict = Some(Ok(()));
  }

  fn fail(&mut self, diff: String) {
    self.verdict = Some(Err(diff));
  }

  /// Advance the matcher; returns whether it just became fully matched.
  fn observe(&mut self, event: &TraceEvent) -> bool {
    if self.settled() {
      return false;
    }
    let was = self.matcher.matched();
    self.matcher.observe(event);
    let now = self.matcher.matched();
    if now && !was {
      if self.asserted {
        self.settle_ok();
      }
      return true;
    }
    false
  }

  /// Evaluate the terminal outcome: the outcome assertion plus whether the
  /// event assertions were fully satisfied.
  fn on_outcome(&mut self, outcome: RunOutcome) {
    if self.settled() {
      return;
    }
    self.outcome = Some(outcome.clone());
    if let Some(expected) = &self.outcome_assertion
      && *expected != OutcomeAssertion::Asserted
      && !expected.matches(&outcome)
    {
      self.fail(format!(
        "outcome mismatch\n  expected: {expected:?}\n  actual:   {outcome:?}"
      ));
      return;
    }
    if self.matcher.matched() {
      self.settle_ok();
    } else {
      self.fail(self.matcher.failure());
    }
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;
  use std::time::Duration;

  use serde_json::json;

  use super::super::assert::{Assertions, parse};
  use super::*;
  use crate::config::{AgentConfig, Config, Format, ImplConfig, Tunables};
  use crate::host::ctx::AgentContext;
  use crate::runtime::{RunOutcome, Runtime};

  /// How a [`TestRuntime`] behaves.
  #[derive(Clone, Copy)]
  enum Mode {
    /// Emit one `start` call, then finish.
    Complete,
    /// Emit `start`, a couple of `delta` calls, then `end`, then loop until
    /// stopped/shut down.
    Stream,
    /// Emit `start`, then loop until stopped/shut down.
    Loop,
    /// Emit `start`, then loop forever ignoring every flag.
    Stubborn,
  }

  struct TestRuntime {
    mode: Mode,
  }

  #[async_trait::async_trait]
  impl Runtime for TestRuntime {
    fn kind() -> &'static str {
      "test"
    }

    async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
      ctx.trace_call("start", json!({}));
      match self.mode {
        Mode::Complete => {}
        Mode::Stream => {
          ctx.trace_call("delta", json!({}));
          ctx.trace_call("delta", json!({}));
          ctx.trace_call("end", json!({}));
          while !(ctx.stop_requested()
            || ctx.shutdown_requested()
            || ctx.reload_requested())
          {
            tokio::time::sleep(Duration::from_millis(2)).await;
          }
        }
        Mode::Loop => {
          while !(ctx.stop_requested()
            || ctx.shutdown_requested()
            || ctx.reload_requested())
          {
            tokio::time::sleep(Duration::from_millis(2)).await;
          }
        }
        Mode::Stubborn => loop {
          tokio::time::sleep(Duration::from_millis(2)).await;
        },
      }
      Ok(RunOutcome::Completed)
    }

    async fn validate(&self, _ctx: &AgentContext) -> anyhow::Result<()> {
      Ok(())
    }
  }

  fn registries() -> anyhow::Result<Registries> {
    let mut registries = Registries::new();
    for (kind, mode) in [
      ("complete", Mode::Complete),
      ("stream", Mode::Stream),
      ("loop", Mode::Loop),
      ("stubborn", Mode::Stubborn),
    ] {
      registries.runtimes.register_factory(kind, move |_, _| {
        Ok(Arc::new(TestRuntime { mode }) as Arc<dyn Runtime>)
      })?;
    }
    Ok(registries)
  }

  /// A config whose agents wire `(name, runtime)` pairs; `runtime` selects one
  /// of the registered [`TestRuntime`] modes.
  fn config(agents: &[(&str, &str)]) -> Config {
    let runtime = agents
      .iter()
      .map(|(_, runtime)| {
        (
          runtime.to_string(),
          ImplConfig {
            kind: runtime.to_string(),
            params: json!({}),
          },
        )
      })
      .collect();
    Config {
      agents: agents
        .iter()
        .map(|(name, runtime)| {
          (
            name.to_string(),
            AgentConfig {
              runtime: runtime.to_string(),
              script: "unused".to_string(),
            },
          )
        })
        .collect(),
      providers: std::collections::HashMap::new(),
      tooling: std::collections::HashMap::new(),
      runtime,
      endpoint: None,
      memory: std::collections::BTreeMap::new(),
      // Keep the abort path instant so a stubborn brain cannot slow tests.
      tunables: Tunables {
        reload_grace_secs: 0,
        interrupt_budget_ms: 0,
        ..Tunables::default()
      },
    }
  }

  fn assertions(body: &str) -> Assertions {
    parse(body, Format::Toml).expect("assertions should parse")
  }

  #[tokio::test]
  async fn asserted_agent_passes_on_settle_and_stops() -> anyhow::Result<()> {
    let registries = registries()?;
    let config = config(&[("alice", "loop")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [{ kind = \"call\", op = \"start\" }]\n",
    );
    let report = Harness::new(&config, &registries, &assertions).run().await;
    assert!(report.error.is_none(), "{:?}", report.error);
    let alice = &report.agents["alice"];
    assert!(alice.passed);
    assert!(alice.asserted);
    Ok(())
  }

  #[tokio::test]
  async fn asserted_agent_fails_when_the_run_ends_first() -> anyhow::Result<()>
  {
    let registries = registries()?;
    let config = config(&[("alice", "complete")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [{ kind = \"call\", op = \"chat\" }]\n",
    );
    let report = Harness::new(&config, &registries, &assertions).run().await;
    let alice = &report.agents["alice"];
    assert!(!alice.passed);
    assert!(
      alice
        .diff
        .as_deref()
        .unwrap_or_default()
        .contains("not satisfied"),
      "{:?}",
      alice.diff
    );
    Ok(())
  }

  #[tokio::test]
  async fn mixed_asserted_and_normal_agents() -> anyhow::Result<()> {
    let registries = registries()?;
    let config = config(&[("alice", "loop"), ("bob", "complete")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [{ kind = \"call\", op = \"start\" }]\n\
       [assertions.bob]\noutcome = \"completed\"\nevents = [{ kind = \"call\", op = \"start\" }]\n",
    );
    let report = Harness::new(&config, &registries, &assertions).run().await;
    assert!(report.passed(), "{report:?}");
    assert!(report.agents["alice"].asserted);
    assert!(!report.agents["bob"].asserted);
    Ok(())
  }

  #[tokio::test]
  async fn multiple_asserted_agents_stop_independently() -> anyhow::Result<()> {
    let registries = registries()?;
    let config = config(&[("alice", "loop"), ("bob", "loop")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [{ kind = \"call\", op = \"start\" }]\n\
       [assertions.bob]\noutcome = \"asserted\"\nevents = [{ kind = \"call\", op = \"start\" }]\n",
    );
    let report = Harness::new(&config, &registries, &assertions).run().await;
    assert!(report.passed(), "{report:?}");
    assert_eq!(report.agents.len(), 2);
    Ok(())
  }

  #[tokio::test]
  async fn a_stubborn_brain_is_forced_down_after_settle() -> anyhow::Result<()>
  {
    let registries = registries()?;
    let config = config(&[("alice", "stubborn")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [{ kind = \"call\", op = \"start\" }]\n",
    );
    let report = tokio::time::timeout(
      Duration::from_secs(10),
      Harness::new(&config, &registries, &assertions).run(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("harness hung on a stubborn brain"))?;
    assert!(report.agents["alice"].passed);
    Ok(())
  }

  #[tokio::test]
  async fn a_timed_out_agent_is_marked_as_timed_out() -> anyhow::Result<()> {
    let registries = registries()?;
    let mut config = config(&[("alice", "stubborn")]);
    config.tunables.test_timeout_secs = 1;
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"completed\"\nevents = [{ kind = \"call\", op = \"never\" }]\n",
    );
    let report = tokio::time::timeout(
      Duration::from_secs(10),
      Harness::new(&config, &registries, &assertions).run(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("harness hung on the test timeout"))?;
    let alice = &report.agents["alice"];
    assert!(!alice.passed);
    assert!(alice.timed_out, "{alice:?}");
    assert!(
      alice
        .diff
        .as_deref()
        .unwrap_or_default()
        .contains("timed out"),
      "{:?}",
      alice.diff
    );
    Ok(())
  }

  #[tokio::test]
  async fn a_zero_timeout_disables_the_bound() -> anyhow::Result<()> {
    let registries = registries()?;
    let mut config = config(&[("alice", "loop")]);
    config.tunables.test_timeout_secs = 0;
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [{ kind = \"call\", op = \"start\" }]\n",
    );
    let report = tokio::time::timeout(
      Duration::from_secs(10),
      Harness::new(&config, &registries, &assertions).run(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("harness hung with the timeout disabled"))?;
    assert!(report.agents["alice"].passed);
    Ok(())
  }

  #[tokio::test]
  async fn asserted_agent_settles_when_a_while_run_reaches_its_anchor()
  -> anyhow::Result<()> {
    let registries = registries()?;
    let config = config(&[("alice", "stream")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [\
       { \"$while\" = { kind = \"call\", op = \"delta\" } }, \
       { kind = \"call\", op = \"end\" }]\n",
    );
    let report = tokio::time::timeout(
      Duration::from_secs(10),
      Harness::new(&config, &registries, &assertions).run(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("harness hung on a `$while` anchor"))?;
    assert!(report.passed(), "{report:?}");
    assert!(report.agents["alice"].asserted);
    Ok(())
  }

  #[tokio::test]
  async fn asserted_agent_with_a_trailing_while_settles_before_the_run()
  -> anyhow::Result<()> {
    let registries = registries()?;
    let config = config(&[("alice", "loop")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"asserted\"\nevents = [\
       { \"$while\" = { kind = \"call\", op = \"delta\" } }]\n",
    );
    let report = Harness::new(&config, &registries, &assertions).run().await;
    assert!(report.passed(), "{report:?}");
    assert!(report.agents["alice"].asserted);
    Ok(())
  }

  #[tokio::test]
  async fn normal_agent_fails_on_event_mismatch() -> anyhow::Result<()> {
    let registries = registries()?;
    let config = config(&[("alice", "complete")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = \"completed\"\nevents = [{ kind = \"call\", op = \"chat\" }]\n",
    );
    let report = Harness::new(&config, &registries, &assertions).run().await;
    assert!(!report.agents["alice"].passed);
    Ok(())
  }

  #[tokio::test]
  async fn normal_agent_fails_on_outcome_mismatch() -> anyhow::Result<()> {
    let registries = registries()?;
    let config = config(&[("alice", "complete")]);
    let assertions = assertions(
      "[assertions.alice]\noutcome = { exited = \"bye\" }\nevents = [{ kind = \"call\", op = \"start\" }]\n",
    );
    let report = Harness::new(&config, &registries, &assertions).run().await;
    assert!(!report.agents["alice"].passed);
    assert!(
      report.agents["alice"]
        .diff
        .as_deref()
        .unwrap_or_default()
        .contains("outcome mismatch")
    );
    Ok(())
  }
}
