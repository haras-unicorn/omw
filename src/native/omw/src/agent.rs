//! Agent bootstrap. Builds the shared provider/tooling/bus registries plus the
//! optional endpoint HTTP server, then runs every configured agent for one
//! iteration (`run`) or loops it (`loop`, restarting on failure with exponential
//! backoff).
//!
//! With `--watch`, a [`ScriptWatcher`](crate::watch::ScriptWatcher) tracks each
//! agent's brain script: when the file changes, the agent's current run is
//! ended cooperatively and the next iteration starts immediately. The shared
//! registries (providers, tooling, bus, endpoint) are kept alive across
//! reloads, so inboxes, agent subscriptions and endpoint models survive: the
//! inbox queue is never drained or dropped on reload, and open
//! stream/timer/resource/tool-call pumps are cancelled unless
//! `tunables.cancel_pumps_on_reload` is false. Rhai brains re-read
//! `ctx.script` and wasm brains reload the component on every iteration, so no
//! script cache needs invalidating.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use futures_util::future::join_all;
use tokio::sync::mpsc;

use crate::config::{AgentConfig, Config};
use crate::host::bus::{MessageBus, new_uuid};
use crate::host::ctx::AgentContext;
use crate::host::endpoint::EndpointRegistry;
use crate::host::events::Event;
use crate::host::streams::{CancelRegistry, StreamRegistry};
use crate::provider::build_registry as build_providers;
use crate::runtime::RunOutcome;
use crate::tooling::build_registry as build_tooling;
use crate::watch::ScriptWatcher;

/// Run every configured agent once, then aggregate their results.
///
/// With `watch`, a reload of an agent's script ends its current run early and
/// starts it again immediately, so `run` keeps agents up until they complete
/// without a pending reload. A shutdown abort is terminal: collected as an
/// error. A reload that outlives `run`'s patience restarts the agent in place
/// (same as `loop` treating reload as `continue`).
pub async fn run_agents(cfg: &Config, watch: bool) -> anyhow::Result<()> {
  let shared = Arc::new(Shared::build(cfg).await?);
  let watch_tx = start_watcher(cfg, watch)?;
  collect_agent_results(
    join_all(cfg.agents.iter().map(|agent| {
      let config = cfg.clone();
      let agent = agent.clone();
      let shared = Arc::clone(&shared);
      let watch_tx = watch_tx.clone();
      tokio::spawn(async move {
        match run_agent(&config, &agent, &shared, watch_tx.clone()).await {
          Ok(AgentStop::Completed(outcome)) => Ok(outcome),
          Ok(AgentStop::Shutdown) => {
            Err(anyhow::anyhow!("agent shutting down"))
          }
          Err(error) => Err(error),
        }
      })
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
/// success and with exponential backoff (doubling up to a cap, see
/// `tunables.loop_backoff_*`) on
/// failure so a wedged agent does not spin the CPU.
///
/// With `watch`, a script change restarts the agent immediately (without
/// backoff) instead of waiting for the current iteration to finish.
pub async fn loop_agents(cfg: &Config, watch: bool) -> anyhow::Result<()> {
  let shared = Arc::new(Shared::build(cfg).await?);
  let watch_tx = start_watcher(cfg, watch)?;
  join_all(cfg.agents.iter().map(|agent| {
    let config = cfg.clone();
    let agent = agent.clone();
    let shared = Arc::clone(&shared);
    let watch_tx = watch_tx.clone();
    tokio::spawn(async move {
      let backoff_start = config.tunables.loop_backoff_start();
      let backoff_cap = config.tunables.loop_backoff_cap();
      let mut delay = backoff_start;
      loop {
        match run_agent(&config, &agent, &shared, watch_tx.clone()).await {
          Ok(AgentStop::Completed(completed)) => {
            tracing::info!(agent = %agent.name, ?completed, "agent iteration completed");
            delay = backoff_start;
          }
          Ok(AgentStop::Shutdown) => {
            tracing::info!(agent = %agent.name, "agent shutting down");
            break;
          }
          Err(error) => {
            tracing::error!(agent = %agent.name, error = %error, "agent iteration failed");
            tracing::debug!(
              agent = %agent.name,
              delay_ms = delay.as_millis(),
              "backing off before retrying the agent"
            );
            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2).min(backoff_cap);
          }
        }
      }
    })
  }))
  .await;
  Ok(())
}

/// How a brain iteration ended, derived from *which `select!` branch won* +
/// flag state, never error text. Transport strings in `block_on_reload` /
/// `recv` / `map_trap` stay; the supervisor just stops reading them.
enum RunEnd {
  Done(RunOutcome),
  Aborted(Abort),
  Failed(anyhow::Error),
}

/// A cooperative abort requested from outside the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abort {
  Reload,
  Shutdown,
}

/// How `run_agent` stops: a terminal brain outcome or a terminal shutdown.
/// Reloads never escape: the same context retries in place with pumps
/// cancelled, so `run` keeps agents up and `loop` never sees a reload.
#[derive(Debug)]
enum AgentStop {
  Completed(RunOutcome),
  Shutdown,
}

/// Build a fresh [`AgentContext`] for one agent and run its brain.
///
/// In `watch` mode the same context is kept alive across reloads: the run is
/// retried after a reload abort with the same inbox and subscriptions, only
/// the stale pumps cancelled. Returns how the agent stopped so `run` can
/// collect it and `loop` can restart or break.
async fn run_agent(
  config: &Config,
  agent: &AgentConfig,
  shared: &Shared,
  watch_tx: Option<ReloadTx>,
) -> anyhow::Result<AgentStop> {
  let impl_cfg = config.runtime.get(&agent.runtime).with_context(|| {
    format!(
      "agent {:?} references unknown runtime {:?}",
      agent.name, agent.runtime
    )
  })?;
  let runtime =
    crate::runtime::build(&agent.runtime, &impl_cfg.kind, &impl_cfg.params)?;
  let ctx = AgentContext::with_tunables(
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
    config.tunables,
  )?;
  // Startup gate: a broken script never produces a first iteration.
  // Without `--watch` this fails fast, same as today. With `--watch` the
  // watcher is already registered below, so an edit fixing the script
  // validates and starts the agent normally instead of failing the task.
  let (reload_tx, mut reload_rx) = mpsc::unbounded_channel::<()>();
  if let Some(watch_tx) = &watch_tx {
    watch_tx.register(&agent.name, reload_tx);
  }
  if let Err(error) = runtime.runtime.validate(&ctx).await {
    if watch_tx.is_none() {
      tracing::error!(agent = %agent.name, error = %error, "agent brain failed startup validation");
      return Err(error);
    }
    tracing::warn!(agent = %agent.name, error = %error, "agent brain invalid at startup, waiting for a fixing edit");
    loop {
      if reload_rx.recv().await.is_none() {
        return Err(error);
      }
      drain_reload(&mut reload_rx);
      match runtime.runtime.validate(&ctx).await {
        Ok(()) => break,
        Err(error) => {
          tracing::warn!(agent = %agent.name, error = %error, "agent reload rejected: invalid script, keeping the agent parked");
          if let Some(lifecycle) = shared.bus.lifecycle_of(&agent.name) {
            shared.bus.deliver(
              &agent.name,
              &lifecycle,
              Event::Error(error.to_string()),
            );
          }
        }
      }
    }
  }
  loop {
    // A previous iteration may have aborted on a reload request; clear it so
    // the new iteration starts clean instead of instantly re-aborting. Drop
    // the queued reload/shutdown system event as well so the next `recv`
    // does not spuriously exit.
    ctx.clear_reload();
    if let Err(error) = shared.bus.drain_system(&agent.name) {
      tracing::warn!(agent = %agent.name, error = %error, "failed to drain system events");
    }
    tracing::info!(agent = %agent.name, runtime = %agent.runtime, "agent iteration starting");
    match run_with_reload(&runtime.runtime, &ctx, &shared.bus, &mut reload_rx)
      .await
    {
      RunEnd::Done(RunOutcome::Completed) => {
        tracing::info!(agent = %agent.name, "agent run completed");
        return Ok(AgentStop::Completed(RunOutcome::Completed));
      }
      RunEnd::Done(RunOutcome::Exited(message)) => {
        tracing::info!(agent = %agent.name, message = %message, "agent exited");
        return Ok(AgentStop::Completed(RunOutcome::Exited(message)));
      }
      RunEnd::Aborted(Abort::Shutdown) => {
        tracing::info!(agent = %agent.name, "agent shutting down");
        return Ok(AgentStop::Shutdown);
      }
      RunEnd::Aborted(Abort::Reload) => {
        tracing::info!(agent = %agent.name, "agent run reloaded, restarting with the new script");
        // A reload abort leaves the current iteration's pumps (chat
        // streams, timers, resource subs, tool calls) registered but
        // ownerless; `cancel_pumps_on_reload = false` keeps them across
        // reload, otherwise they are cancelled so stale events cannot leak
        // into the next iteration. The shared bus (inboxes, subscriptions)
        // is untouched, and the inbox queue carries over. The same context
        // is reused, and rhai brains re-read `ctx.script` while wasm brains
        // reload the component, so the next iteration picks up the edit.
        if config.tunables.cancel_pumps_on_reload {
          ctx.streams.cancel_all();
          ctx.timers.cancel_all();
          ctx.resources.cancel_all();
          ctx.tool_calls.cancel_all();
        }
        drain_reload(&mut reload_rx);
        continue;
      }
      RunEnd::Failed(error) => {
        tracing::error!(agent = %agent.name, error = %error, "agent run failed");
        return Err(error);
      }
    }
  }
}

/// Run one brain iteration, ending it early on a reload signal or on process
/// shutdown. `run` and `loop` both funnel through here so `Completed`,
/// reloads, and shutdowns behave the same in either mode.
///
/// Structured as a loop over `select! { finished, shutdown, reload_rx }`: on
/// a reload signal, `runtime.validate(ctx)` runs *while the old run keeps
/// executing*. Valid → the abort path (deliver `reload` + flag + grace +
/// epoch). Invalid → an `error` event tagged with the lifecycle UUID (if
/// subscribed; log-only otherwise), a host-side `warn!`, and back to waiting
/// on the *same* `finished` future: no flag is set, no grace starts, no epoch
/// fires, so the live run provably never exits on an invalid edit.
async fn run_with_reload(
  runtime: &Arc<dyn crate::runtime::Runtime>,
  ctx: &AgentContext,
  bus: &Arc<MessageBus>,
  reload_rx: &mut mpsc::UnboundedReceiver<()>,
) -> RunEnd {
  // With no watcher there is no sender, so `recv` never resolves and the
  // run completes undisturbed. Shutdown always listens.
  let mut finished = Box::pin(runtime.run(ctx));
  let shutdown = shutdown_signal();
  tokio::pin!(shutdown);
  loop {
    tokio::select! {
      biased;
      outcome = &mut finished => return classify_finished(outcome, ctx),
      () = &mut shutdown => {
        let id = bus.lifecycle_of(&ctx.name).unwrap_or_else(new_uuid);
        ctx.request_shutdown();
        bus.deliver(&ctx.name, &id, Event::Shutdown);
        tracing::info!(agent = %ctx.name, "agent shutdown requested");
        return abort_grace(&mut finished, ctx, Abort::Shutdown).await;
      }
      reload = reload_rx.recv() => {
        if reload.is_none() {
          return classify_finished((&mut finished).await, ctx);
        }
        match runtime.validate(ctx).await {
          Ok(()) => {
            // Blocking `host.recv` polls the reload flag every slice and
            // aborts without draining the inbox, so queued events survive
            // into the next iteration. Force-abort as a backstop in case the
            // brain is stuck in a long blocking provider/tool call instead.
            // The `reload` event covers `try-recv` pollers.
            let id = bus.lifecycle_of(&ctx.name).unwrap_or_else(new_uuid);
            ctx.request_reload();
            bus.deliver(&ctx.name, &id, Event::Reload);
            tracing::info!(agent = %ctx.name, "agent reload requested");
            return abort_grace(&mut finished, ctx, Abort::Reload).await;
          }
          Err(error) => {
            tracing::warn!(agent = %ctx.name, error = %error, "agent reload rejected: invalid script, keeping the live run");
            if let Some(lifecycle) = bus.lifecycle_of(&ctx.name) {
              bus.deliver(
                &ctx.name,
                &lifecycle,
                Event::Error(error.to_string()),
              );
            }
          }
        }
      }
    }
  }
}

/// Map a settled run future onto [`RunEnd`] from flag state, never error
/// text: a settled run with a flag set is an abort (a cooperative brain that
/// saw the event and exited cleanly, or errored via the abort string),
/// while an `Err` without a flag is a failure even if its text mentions
/// reload.
fn classify_finished(
  outcome: anyhow::Result<RunOutcome>,
  ctx: &AgentContext,
) -> RunEnd {
  if ctx.shutdown_requested() {
    return RunEnd::Aborted(Abort::Shutdown);
  }
  if ctx.reload_requested() {
    return RunEnd::Aborted(Abort::Reload);
  }
  match outcome {
    Ok(completed) => RunEnd::Done(completed),
    Err(error) => RunEnd::Failed(error),
  }
}

/// Give a requested abort the reload grace to exit cooperatively, then fire
/// the runtime's preemptive interrupt and allow the interrupt budget to
/// unwind. A settled run classifies via flags; an unsettled one reports the
/// abort.
async fn abort_grace<F>(
  finished: &mut std::pin::Pin<Box<F>>,
  ctx: &AgentContext,
  abort: Abort,
) -> RunEnd
where
  F: std::future::Future<Output = anyhow::Result<RunOutcome>> + Send + ?Sized,
{
  let tunables = ctx.tunables();
  match tokio::time::timeout(tunables.reload_grace(), &mut *finished).await {
    Ok(outcome) => classify_finished(outcome, ctx),
    Err(_) => {
      // Grace expired: cooperative tiers (recv abort, `block_on_reload`)
      // did not exit. Fire the runtime's preemptive interrupt; threads
      // parked in blocking calls were already aborted by the helper, so
      // this only kills pure-compute spinners. Then give the interrupt a
      // brief budget to unwind.
      ctx.interrupt();
      match tokio::time::timeout(tunables.interrupt_budget(), &mut *finished)
        .await
      {
        Ok(outcome) => classify_finished(outcome, ctx),
        Err(_) => RunEnd::Aborted(abort),
      }
    }
  }
}

/// Resolve on SIGTERM/SIGINT so the supervisor can shut agents down
/// terminally. Pending forever when no signal arrives.
async fn shutdown_signal() {
  #[cfg(unix)]
  {
    let term =
      tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
    let int =
      tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
    let (Ok(mut term), Ok(mut int)) = (term, int) else {
      std::future::pending::<()>().await;
      return;
    };
    tokio::select! {
      _ = term.recv() => {},
      _ = int.recv() => {},
    }
  }
  #[cfg(not(unix))]
  {
    let _ = tokio::signal::ctrl_c().await;
  }
}

/// Drop any reload signals that arrived while the run was unwinding, so one
/// save cannot restart the agent twice.
fn drain_reload(reload_rx: &mut mpsc::UnboundedReceiver<()>) {
  while reload_rx.try_recv().is_ok() {}
}

/// Agent-name -> reload-sender registry fed by the [`ScriptWatcher`] pump.
/// Held per agent task and cloned where the supervisor needs it.
#[derive(Clone)]
struct ReloadTx {
  senders:
    Arc<std::sync::Mutex<HashMap<String, Vec<mpsc::UnboundedSender<()>>>>>,
}

impl ReloadTx {
  fn register(&self, agent: &str, tx: mpsc::UnboundedSender<()>) {
    let mut senders = self
      .senders
      .lock()
      .unwrap_or_else(|poison| poison.into_inner());
    senders.entry(agent.to_string()).or_default().push(tx);
  }

  fn notify(&self, agents: &[String]) {
    let mut senders = self
      .senders
      .lock()
      .unwrap_or_else(|poison| poison.into_inner());
    for agent in agents {
      if let Some(txs) = senders.get_mut(agent) {
        // Drop receivers from finished iterations so one save restarts the
        // agent once and the registry does not grow over time.
        txs.retain(|tx| !tx.is_closed());
        for tx in txs.iter() {
          let _ = tx.send(());
        }
      }
    }
  }
}

/// Start the script watcher when `watch` is set, returning the registry its
/// pump notifies. Without `watch` there is no pump and no overhead.
fn start_watcher(
  cfg: &Config,
  watch: bool,
) -> anyhow::Result<Option<ReloadTx>> {
  if !watch {
    return Ok(None);
  }
  let reload = ReloadTx {
    senders: Arc::new(std::sync::Mutex::new(HashMap::new())),
  };
  let pump_reload = reload.clone();
  let mut watcher = ScriptWatcher::with_tunables(&cfg.agents, cfg.tunables)?;
  if watcher.is_empty() {
    tracing::warn!("--watch is set but no agent script can be watched");
    return Ok(None);
  }
  tokio::spawn(async move {
    while let Some(agents) = watcher.next_reload().await {
      pump_reload.notify(&agents);
    }
    tracing::warn!("script watcher ended; hot reload is disabled from here on");
  });
  Ok(Some(reload))
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
    let bus = Arc::new(MessageBus::with_tunables(cfg.tunables));
    let (endpoint_registry, endpoint_task) =
      if let Some(endpoint) = &cfg.endpoint {
        let entry = crate::endpoint::build(&endpoint.kind, &endpoint.params)
          .with_context(|| {
            format!("failed to build endpoint {:?}", endpoint.kind)
          })?;
        let registry = Arc::new(EndpointRegistry::with_tunables(
          Arc::clone(&bus),
          cfg.tunables,
        ));
        let serve_bus = Arc::clone(&bus);
        let serve_registry = Arc::clone(&registry);
        let task = tokio::spawn(async move {
          if let Err(error) =
            entry.endpoint.serve(serve_bus, serve_registry).await
          {
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
mod abort_tests {
  use super::*;

  #[test]
  fn classify_finished_maps_flag_state_not_error_text() -> anyhow::Result<()> {
    let shared = Arc::new(MessageBus::new());
    let ctx = AgentContext::new(
      "test-agent".to_string(),
      PathBuf::from("unused.rhai"),
      HashMap::new(),
      HashMap::new(),
      shared,
      Arc::new(StreamRegistry::new()),
      Arc::new(CancelRegistry::new()),
      Arc::new(CancelRegistry::new()),
      Arc::new(CancelRegistry::new()),
      None,
    )?;
    // Clean Ok with no flag: done.
    assert!(matches!(
      classify_finished(Ok(RunOutcome::Completed), &ctx),
      RunEnd::Done(RunOutcome::Completed)
    ));
    // Clean Ok with a reload flag: still an abort (cooperative exit).
    ctx.request_reload();
    assert!(matches!(
      classify_finished(Ok(RunOutcome::Completed), &ctx),
      RunEnd::Aborted(Abort::Reload)
    ));
    ctx.clear_reload();
    // Err mentioning reload without a flag: still a failure.
    assert!(matches!(
      classify_finished(Err(anyhow::anyhow!("agent reloaded: boom")), &ctx),
      RunEnd::Failed(_)
    ));
    // Shutdown flag wins over reload text.
    ctx.request_shutdown();
    assert!(matches!(
      classify_finished(Ok(RunOutcome::Completed), &ctx),
      RunEnd::Aborted(Abort::Shutdown)
    ));
    Ok(())
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
