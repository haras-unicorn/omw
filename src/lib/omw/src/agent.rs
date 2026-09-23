//! Agent bootstrap. Builds the shared provider/tooling/bus registries plus the
//! optional endpoint HTTP server, then runs every configured agent for one
//! iteration (`run`) or loops it (`loop`, restarting on failure with exponential
//! backoff).
//!
//! With `--watch`, a [`Scripts`](crate::watch::Scripts) tracks each
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
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::future::join_all;
use tokio::sync::mpsc;

use crate::config::{AgentConfig, Config};
use crate::host::bus::{MessageBus, new_uuid};
use crate::host::ctx::AgentContext;
use crate::host::endpoint::EndpointRegistry;
use crate::host::events::Event;
use crate::host::streams::{CancelRegistry, StreamRegistry};
use crate::host::trace::{TraceEvent, TraceSender};
use crate::runtime::RunOutcome;
use crate::shutdown::{Shutdown, shutdown_signal};
use crate::watch::Scripts;

/// The back-end registries the supervisor builds entries from. Owned by
/// the caller and passed into [`run_agents`] / [`loop_agents`], so custom
/// back ends are registered before the supervisor runs.
pub struct Registries {
  pub providers: crate::provider::Registry,
  pub tooling: crate::tooling::Registry,
  pub runtimes: crate::runtime::Registry,
  pub endpoints: crate::endpoint::Registry,
}

impl Registries {
  /// An empty set of registries with no built-ins.
  pub fn new() -> Self {
    Self {
      providers: crate::provider::Registry::new(),
      tooling: crate::tooling::Registry::new(),
      runtimes: crate::runtime::Registry::new(),
      endpoints: crate::endpoint::Registry::new(),
    }
  }
}

impl Default for Registries {
  /// The feature-gated built-ins, one per family.
  fn default() -> Self {
    Self {
      providers: crate::provider::Registry::default(),
      tooling: crate::tooling::Registry::default(),
      runtimes: crate::runtime::Registry::default(),
      endpoints: crate::endpoint::Registry::default(),
    }
  }
}

/// Per-agent cooperative stop flags, shared between the testing harness (which
/// flips them) and the agent contexts (which poll them). Empty in normal runs,
/// so the flag is never set and behaviour is unchanged.
#[derive(Clone, Default)]
pub(crate) struct StopRegistry {
  flags: Arc<dashmap::DashMap<String, Arc<AtomicBool>>>,
}

impl StopRegistry {
  /// The stop flag for `agent`, created (unset) on first use.
  pub(crate) fn flag(&self, agent: &str) -> Arc<AtomicBool> {
    let entry = self
      .flags
      .entry(agent.to_string())
      .or_insert_with(|| Arc::new(AtomicBool::new(false)));
    Arc::clone(&entry)
  }

  /// Request that `agent` stop. Creates the flag if the agent has not started
  /// yet, so an early stop still lands.
  pub(crate) fn request_stop(&self, agent: &str) {
    self.flag(agent).store(true, Ordering::Relaxed);
  }
}

/// Run every configured agent once, then aggregate their results.
///
/// With `watch`, a reload of an agent's script ends its current run early and
/// starts it again immediately, so `run` keeps agents up until they complete
/// without a pending reload. A shutdown (SIGTERM/SIGINT) is terminal but
/// graceful: every in-flight iteration aborts through the same tiers as a
/// reload, then `run` returns `Ok` (exit 0). Only a genuine failure without a
/// shutdown request collects as an error. A reload that outlives `run`'s
/// patience restarts the agent in place (same as `loop` treating reload as
/// `continue`).
pub async fn run_agents(
  cfg: &Config,
  watch: bool,
  registries: &Registries,
) -> anyhow::Result<()> {
  run_once(cfg, watch, registries, None).await
}

/// Like [`run_agents`] but with an active trace channel: every agent's inbox
/// observations and outbound host calls are pushed onto `tx`, and the function
/// returns the collected [`TraceEvent`] stream once every agent has stopped.
///
/// A receiver-drain task subscribes to `tx` and buffers events concurrently,
/// so a long run does not overflow the broadcast buffer; a lagged receiver is
/// reported as an error rather than silently dropping observations.
pub async fn run_agents_traced(
  cfg: &Config,
  watch: bool,
  registries: &Registries,
  tx: TraceSender,
) -> anyhow::Result<Vec<TraceEvent>> {
  let tx_for_run = tx.clone();
  let rx = tx.subscribe();
  let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
  let (collected_tx, mut collected_rx) =
    tokio::sync::mpsc::unbounded_channel::<TraceEvent>();
  let lagged = Arc::new(std::sync::atomic::AtomicBool::new(false));
  let collector_lagged = Arc::clone(&lagged);
  let collector =
    tokio::spawn(collect_trace(rx, stop_rx, collected_tx, collector_lagged));
  let run = run_once(cfg, watch, registries, Some(tx_for_run)).await;
  let _ = stop_tx.send(());
  let collected_tx = collector.await?;
  run?;
  if lagged.load(std::sync::atomic::Ordering::Relaxed) {
    anyhow::bail!(
      "trace receiver lagged: more than the broadcast buffer of events \
       accumulated; raise the buffer size"
    );
  }
  let mut events = Vec::new();
  while let Ok(event) = collected_rx.try_recv() {
    events.push(event);
  }
  drop(collected_tx);
  Ok(events)
}

/// Run every configured agent once with an optional trace tap. The traced
/// entry points funnel through here so `run` and `run_agents_traced` behave
/// identically except for the tap.
async fn run_once(
  cfg: &Config,
  watch: bool,
  registries: &Registries,
  trace: Option<TraceSender>,
) -> anyhow::Result<()> {
  let shutdown = Shutdown::new();
  let signal = spawn_signal(&shutdown);
  let result = run_agents_inner(
    cfg,
    watch,
    registries,
    trace,
    shutdown,
    StopRegistry::default(),
  )
  .await;
  signal.abort();
  result
}

/// The testing harness's controlled entry point: like [`run_once`] but with a
/// caller-owned [`Shutdown`] (so the harness can force stragglers down) and a
/// [`StopRegistry`] (so it can stop an `asserted` agent once its assertions
/// settle). No OS signal subscription is installed.
pub(crate) async fn run_agents_controlled(
  cfg: &Config,
  registries: &Registries,
  trace: TraceSender,
  shutdown: Shutdown,
  stops: StopRegistry,
) -> anyhow::Result<()> {
  run_agents_inner(cfg, false, registries, Some(trace), shutdown, stops).await
}

/// Shared body of `run`: build the process registries, spawn one task per
/// agent (each with its stop flag), collect results, and force the run down
/// when `shutdown` is requested.
async fn run_agents_inner(
  cfg: &Config,
  watch: bool,
  registries: &Registries,
  trace: Option<TraceSender>,
  shutdown: Shutdown,
  stops: StopRegistry,
) -> anyhow::Result<()> {
  let shared = Arc::new(
    Shared::build(cfg, registries, shutdown.clone(), trace.clone()).await?,
  );
  let (watch_tx, watcher) = start_watcher(cfg, watch)?;
  let mut handles = Vec::new();
  for agent in &cfg.agents {
    let config = cfg.clone();
    let agent = agent.clone();
    let name = agent.name.clone();
    let shared = Arc::clone(&shared);
    let watch_tx = watch_tx.clone();
    let stop = stops.flag(&name);
    let handle = tokio::spawn(async move {
      run_agent(&config, &agent, &shared, watch_tx.clone(), stop).await
    });
    handles.push((name, handle));
  }
  let mut results = Vec::with_capacity(handles.len());
  for (name, handle) in handles {
    let result = match handle.await {
      Ok(Ok(AgentStop::Completed(outcome))) => {
        if let Some(tx) = trace.as_ref() {
          let _ = tx.send(TraceEvent::Outcome {
            agent: name,
            outcome,
          });
        }
        Ok(())
      }
      // Shutdown already maps to `Ok` below; keep the message for logs.
      Ok(Ok(AgentStop::Shutdown)) => {
        Err(anyhow::anyhow!("agent shutting down"))
      }
      // A stop is terminal and not a failure.
      Ok(Ok(AgentStop::Stopped)) => Ok(()),
      Ok(Err(error)) => Err(error),
      Err(join_error) => Err(anyhow::Error::from(join_error)),
    };
    results.push(result);
  }
  if let Some(watcher) = watcher {
    watcher.abort();
  }
  if shutdown.is_requested() {
    tracing::info!("shutdown requested, exiting gracefully");
    return Ok(());
  }
  collect_agent_results(results)
}

/// Concurrently drain `rx` into `collected_tx` until `stop_rx` fires, then
/// perform a final synchronous drain of whatever is still buffered. Returns
/// the sender half so the caller can keep the collection channel open until
/// it has emptied it.
async fn collect_trace(
  mut rx: tokio::sync::broadcast::Receiver<TraceEvent>,
  mut stop_rx: tokio::sync::oneshot::Receiver<()>,
  collected_tx: tokio::sync::mpsc::UnboundedSender<TraceEvent>,
  lagged: Arc<std::sync::atomic::AtomicBool>,
) -> tokio::sync::mpsc::UnboundedSender<TraceEvent> {
  use std::sync::atomic::Ordering;
  use tokio::sync::broadcast::error::{RecvError, TryRecvError};
  loop {
    tokio::select! {
      biased;
      _ = &mut stop_rx => {
        loop {
          match rx.try_recv() {
            Ok(event) => { let _ = collected_tx.send(event); }
            Err(TryRecvError::Lagged(_)) => {
              lagged.store(true, Ordering::Relaxed);
            }
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
          }
        }
        break;
      }
      received = rx.recv() => match received {
        Ok(event) => { let _ = collected_tx.send(event); }
        Err(RecvError::Lagged(_)) => {
          lagged.store(true, Ordering::Relaxed);
        }
        Err(RecvError::Closed) => break,
      },
    }
  }
  collected_tx
}

/// Run every configured agent in a loop forever, restarting immediately on
/// success and with exponential backoff (doubling up to a cap, see
/// `tunables.loop_backoff_*`) on
/// failure so a wedged agent does not spin the CPU.
///
/// With `watch`, a script change restarts the agent immediately (without
/// backoff) instead of waiting for the current iteration to finish.
pub async fn loop_agents(
  cfg: &Config,
  watch: bool,
  registries: &Registries,
) -> anyhow::Result<()> {
  loop_once(cfg, watch, registries, None).await
}

/// Like [`loop_agents`] but with an active trace channel. Every iteration's
/// inbox observations, host calls and (on success) outcome are pushed onto
/// `tx`; the function itself never returns while agents keep looping.
pub async fn loop_agents_traced(
  cfg: &Config,
  watch: bool,
  registries: &Registries,
  tx: TraceSender,
) -> anyhow::Result<()> {
  loop_once(cfg, watch, registries, Some(tx)).await
}

async fn loop_once(
  cfg: &Config,
  watch: bool,
  registries: &Registries,
  trace: Option<TraceSender>,
) -> anyhow::Result<()> {
  let shutdown = Shutdown::new();
  let shared = Arc::new(
    Shared::build(cfg, registries, shutdown.clone(), trace.clone()).await?,
  );
  let signal = spawn_signal(&shutdown);
  let (watch_tx, watcher) = start_watcher(cfg, watch)?;
  join_all(cfg.agents.iter().map(|agent| {
    let config = cfg.clone();
    let agent = agent.clone();
    let shared = Arc::clone(&shared);
    let watch_tx = watch_tx.clone();
    let trace = trace.clone();
    tokio::spawn(async move {
      let backoff_start = config.tunables.loop_backoff_start();
      let backoff_cap = config.tunables.loop_backoff_cap();
      let mut delay = backoff_start;
      // `loop` has no testing harness, so the stop flag is never set.
      let stop = Arc::new(AtomicBool::new(false));
      loop {
        if shared.shutdown.is_requested() {
          tracing::info!(agent = %agent.name, "agent shutting down");
          break;
        }
        match run_agent(&config, &agent, &shared, watch_tx.clone(), Arc::clone(&stop)).await {
          Ok(AgentStop::Completed(completed)) => {
            tracing::info!(agent = %agent.name, ?completed, "agent iteration completed");
            if let Some(tx) = &trace {
              let _ = tx.send(TraceEvent::Outcome {
                agent: agent.name.clone(),
                outcome: completed.clone(),
              });
            }
            delay = backoff_start;
          }
          Ok(AgentStop::Shutdown) => {
            tracing::info!(agent = %agent.name, "agent shutting down");
            break;
          }
          Ok(AgentStop::Stopped) => {
            tracing::info!(agent = %agent.name, "agent stopped");
            break;
          }
          Err(error) => {
            if shared.shutdown.is_requested() {
              tracing::info!(agent = %agent.name, "agent shutting down");
              break;
            }
            tracing::error!(agent = %agent.name, error = %error, "agent iteration failed");
            tracing::debug!(
              agent = %agent.name,
              delay_ms = delay.as_millis(),
              "backing off before retrying the agent"
            );
            tokio::select! {
              biased;
              () = shared.shutdown.wait() => {
                tracing::info!(agent = %agent.name, "agent shutting down");
                break;
              }
              () = tokio::time::sleep(delay) => {}
            }
            delay = delay.saturating_mul(2).min(backoff_cap);
          }
        }
      }
    })
  }))
  .await;
  signal.abort();
  if let Some(watcher) = watcher {
    watcher.abort();
  }
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
  Stop,
}

/// How `run_agent` stops: a terminal brain outcome or a terminal shutdown.
/// Reloads never escape: the same context retries in place with pumps
/// cancelled, so `run` keeps agents up and `loop` never sees a reload. A
/// `Stopped` stop is the testing harness's per-agent assertion stop: terminal,
/// never restarted, and not a failure.
#[derive(Debug)]
enum AgentStop {
  Completed(RunOutcome),
  Shutdown,
  Stopped,
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
  stop: Arc<AtomicBool>,
) -> anyhow::Result<AgentStop> {
  let runtime = shared.runtimes.get(&agent.name).ok_or_else(|| {
    anyhow::anyhow!("agent {:?} has no built runtime", agent.name)
  })?;
  let mut ctx = AgentContext::with_tunables(
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
    shared.trace.clone(),
    config.tunables,
  )?;
  ctx.set_stop_flag(stop);
  if let Some(seed) = config.memory.get(&agent.name) {
    ctx
      .memory()
      .seed(seed.iter().map(|(key, value)| (key.clone(), value.clone())));
  }
  // Startup gate: a broken script never produces a first iteration.
  // Without `--watch` this fails fast, same as today. With `--watch` the
  // watcher is already registered below, so an edit fixing the script
  // validates and starts the agent normally instead of failing the task.
  // A shutdown parked at the gate exits terminally instead of waiting for
  // a fixing edit that will never come.
  let (reload_tx, mut reload_rx) = mpsc::unbounded_channel::<()>();
  if let Some(watch_tx) = &watch_tx {
    watch_tx.register(&agent.name, reload_tx);
  }
  if let Err(error) = runtime.inner().validate(&ctx).await {
    if shared.shutdown.is_requested() {
      tracing::info!(agent = %agent.name, "agent shutting down");
      return Ok(AgentStop::Shutdown);
    }
    if watch_tx.is_none() {
      tracing::error!(agent = %agent.name, error = %error, "agent brain failed startup validation");
      return Err(error);
    }
    tracing::warn!(agent = %agent.name, error = %error, "agent brain invalid at startup, waiting for a fixing edit");
    loop {
      tokio::select! {
        biased;
        () = shared.shutdown.wait() => {
          tracing::info!(agent = %agent.name, "agent shutting down");
          return Ok(AgentStop::Shutdown);
        }
        reload = reload_rx.recv() => {
          if reload.is_none() {
            return Err(error);
          }
        }
      }
      drain_reload(&mut reload_rx);
      match runtime.inner().validate(&ctx).await {
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
    // does not spuriously exit. A shutdown that landed between iterations
    // exits terminally instead of starting another run.
    if shared.shutdown.is_requested() {
      tracing::info!(agent = %agent.name, "agent shutting down");
      return Ok(AgentStop::Shutdown);
    }
    ctx.clear_reload();
    if let Err(error) = shared.bus.drain_system(&agent.name) {
      tracing::warn!(agent = %agent.name, error = %error, "failed to drain system events");
    }
    tracing::info!(agent = %agent.name, runtime = %agent.runtime, "agent iteration starting");
    match run_with_reload(runtime.inner(), &ctx, shared, &mut reload_rx).await {
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
        // A shutdown abort owns the brain's pumps outright: no next
        // iteration will run, so cancel them now instead of leaving
        // ownerless pumps on the bridge runtime until drop.
        ctx.streams().cancel_all();
        ctx.timers().cancel_all();
        ctx.resources().cancel_all();
        ctx.tool_calls().cancel_all();
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
          ctx.streams().cancel_all();
          ctx.timers().cancel_all();
          ctx.resources().cancel_all();
          ctx.tool_calls().cancel_all();
        }
        drain_reload(&mut reload_rx);
        continue;
      }
      RunEnd::Aborted(Abort::Stop) => {
        tracing::info!(agent = %agent.name, "agent stopped by the testing harness");
        // Terminal, and no next iteration will run, so cancel this run's
        // ownerless pumps like a shutdown does.
        ctx.streams().cancel_all();
        ctx.timers().cancel_all();
        ctx.resources().cancel_all();
        ctx.tool_calls().cancel_all();
        return Ok(AgentStop::Stopped);
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
/// Structured as a loop over `select! { finished, shutdown, reload_rx }`:
/// the process-wide latch resolves immediately when already set, so
/// iterations starting after the signal still abort. On a reload signal,
/// `runtime.validate(ctx)` runs *while the old run keeps
/// executing*. Valid → the abort path (deliver `reload` + flag + grace +
/// epoch). Invalid → an `error` event tagged with the lifecycle UUID (if
/// subscribed; log-only otherwise), a host-side `warn!`, and back to waiting
/// on the *same* `finished` future: no flag is set, no grace starts, no epoch
/// fires, so the live run provably never exits on an invalid edit.
async fn run_with_reload(
  runtime: &Arc<dyn crate::runtime::Runtime>,
  ctx: &AgentContext,
  shared: &Shared,
  reload_rx: &mut mpsc::UnboundedReceiver<()>,
) -> RunEnd {
  // With no watcher there is no sender, so `recv` never resolves and the
  // run completes undisturbed. Shutdown always listens.
  let bus = &shared.bus;
  let mut finished = Box::pin(runtime.run(ctx));
  loop {
    if shared.shutdown.is_requested() {
      let id = bus.lifecycle_of(ctx.name()).unwrap_or_else(new_uuid);
      ctx.request_shutdown();
      bus.deliver(ctx.name(), &id, Event::Shutdown);
      tracing::info!(agent = %ctx.name(), "agent shutdown requested");
      return abort_grace(&mut finished, ctx, Abort::Shutdown).await;
    }
    tokio::select! {
      biased;
      outcome = &mut finished => return classify_finished(outcome, ctx),
      () = shared.shutdown.wait() => {
        let id = bus.lifecycle_of(ctx.name()).unwrap_or_else(new_uuid);
        ctx.request_shutdown();
        bus.deliver(ctx.name(), &id, Event::Shutdown);
        tracing::info!(agent = %ctx.name(), "agent shutdown requested");
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
            let id = bus.lifecycle_of(ctx.name()).unwrap_or_else(new_uuid);
            ctx.request_reload();
            bus.deliver(ctx.name(), &id, Event::Reload);
            tracing::info!(agent = %ctx.name(), "agent reload requested");
            return abort_grace(&mut finished, ctx, Abort::Reload).await;
          }
          Err(error) => {
            tracing::warn!(agent = %ctx.name(), error = %error, "agent reload rejected: invalid script, keeping the live run");
            if let Some(lifecycle) = bus.lifecycle_of(ctx.name()) {
              bus.deliver(
                ctx.name(),
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
  if ctx.stop_requested() {
    return RunEnd::Aborted(Abort::Stop);
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

/// Spawn the single OS signal subscription for this process: on the first
/// SIGTERM/SIGINT request the shared latch, which every agent iteration,
/// the endpoint server, and the `loop` backoff await.
fn spawn_signal(shutdown: &Shutdown) -> tokio::task::JoinHandle<()> {
  let shutdown = shutdown.clone();
  tokio::spawn(async move {
    shutdown_signal().await;
    tracing::info!("shutdown signal received");
    shutdown.request();
  })
}

/// Drop any reload signals that arrived while the run was unwinding, so one
/// save cannot restart the agent twice.
fn drain_reload(reload_rx: &mut mpsc::UnboundedReceiver<()>) {
  while reload_rx.try_recv().is_ok() {}
}

/// Agent-name -> reload-sender registry fed by the [`Scripts`] pump.
/// Held per agent task and cloned where the supervisor needs it.
#[derive(Clone)]
struct ReloadTx {
  senders: Arc<dashmap::DashMap<String, Vec<mpsc::UnboundedSender<()>>>>,
}

impl ReloadTx {
  fn register(&self, agent: &str, tx: mpsc::UnboundedSender<()>) {
    self.senders.entry(agent.to_string()).or_default().push(tx);
  }

  fn notify(&self, agents: &[String]) {
    for agent in agents {
      if let Some(mut txs) = self.senders.get_mut(agent) {
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
/// pump notifies plus the pump handle so the supervisor can abort it on
/// shutdown. Without `watch` there is no pump and no overhead.
fn start_watcher(
  cfg: &Config,
  watch: bool,
) -> anyhow::Result<(Option<ReloadTx>, Option<tokio::task::JoinHandle<()>>)> {
  if !watch {
    return Ok((None, None));
  }
  let reload = ReloadTx {
    senders: Arc::new(dashmap::DashMap::new()),
  };
  let pump_reload = reload.clone();
  let mut watcher = Scripts::with_tunables(&cfg.agents, cfg.tunables)?;
  if watcher.is_empty() {
    tracing::warn!("--watch is set but no agent script can be watched");
    return Ok((None, None));
  }
  let handle = tokio::spawn(async move {
    while let Some(agents) = watcher.next_reload().await {
      pump_reload.notify(&agents);
    }
    tracing::warn!("script watcher ended; hot reload is disabled from here on");
  });
  Ok((Some(reload), Some(handle)))
}

/// Aggregate the results of every agent task into one, erroring if any of
/// them failed.
fn collect_agent_results(
  results: Vec<anyhow::Result<()>>,
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
  runtimes: HashMap<String, crate::runtime::RuntimeEntry>,
  bus: Arc<MessageBus>,
  endpoint_registry: Option<Arc<EndpointRegistry>>,
  endpoint_task: Option<tokio::task::JoinHandle<()>>,
  shutdown: Shutdown,
  /// Optional trace tap shared with every agent context and the bus.
  trace: Option<TraceSender>,
}

impl Shared {
  async fn build(
    cfg: &Config,
    registries: &Registries,
    shutdown: Shutdown,
    trace: Option<TraceSender>,
  ) -> anyhow::Result<Self> {
    // Secrets lock with `mlock` at construction; permit unlocked secrets for
    // the whole bootstrap when the tunable opts in (e.g. inside containers
    // where the outer `RLIMIT_MEMLOCK` cannot be raised).
    let build = || -> anyhow::Result<_> {
      let providers = registries.providers.build_entries(cfg)?;
      let tooling = registries.tooling.build_entries(cfg)?;
      if let Some(tx) = &trace {
        for entry in tooling.values() {
          entry.inner().attach_trace(tx.clone());
        }
      }
      let runtimes = cfg
        .agents
        .iter()
        .map(|agent| {
          registries
            .runtimes
            .build_for_agent(cfg, agent)
            .map(|entry| (agent.name.clone(), entry))
        })
        .collect::<anyhow::Result<HashMap<_, _>>>()?;
      let bus = match &trace {
        Some(tx) => Arc::new(MessageBus::with_trace(cfg.tunables, tx.clone())),
        None => Arc::new(MessageBus::with_tunables(cfg.tunables)),
      };
      Ok((providers, tooling, runtimes, bus))
    };
    let (providers, tooling, runtimes, bus) =
      if cfg.tunables.allow_unlocked_secrets {
        crate::secret::allow_unlocked(build)
      } else {
        build()
      }?;
    let (endpoint_registry, endpoint_task) =
      if let Some(entry) = registries.endpoints.build_entry(cfg)? {
        let registry = Arc::new(EndpointRegistry::with_tunables(
          Arc::clone(&bus),
          cfg.tunables,
        ));
        let serve_bus = Arc::clone(&bus);
        let serve_registry = Arc::clone(&registry);
        let serve_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
          if let Err(error) = entry
            .inner()
            .serve(serve_bus, serve_registry, serve_shutdown)
            .await
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
      runtimes,
      bus,
      endpoint_registry,
      endpoint_task,
      shutdown,
      trace,
    })
  }
}

impl Drop for Shared {
  // Best-effort only: `drop` cannot await the axum graceful drain, so it
  // unblocks the HTTP handlers with error ends and aborts the serve task.
  // The normal path already resolved `shutdown.wait()` inside `serve` before
  // this runs.
  fn drop(&mut self) {
    if let Some(registry) = self.endpoint_registry.take() {
      // Unblock the HTTP handlers first so the axum graceful shutdown the
      // serve task awaits can drain; the task itself is joined below.
      registry.abort_all();
    }
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
    let results = vec![Ok(()), Ok(())];
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

  /// A provider/tooling-free runtime that exercises both trace taps: one
  /// outbound call and one observed inbox event.
  struct ProbeRuntime;

  #[async_trait::async_trait]
  impl crate::runtime::Runtime for ProbeRuntime {
    fn kind() -> &'static str {
      "probe"
    }

    async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
      ctx.trace_call("probe", serde_json::json!({ "n": 1 }));
      ctx
        .bus()
        .deliver(ctx.name(), "self", Event::Message("ping".to_string()));
      let _ = ctx
        .bus()
        .recv(ctx.name(), std::time::Duration::from_secs(1))?;
      Ok(RunOutcome::Completed)
    }

    async fn validate(&self, _ctx: &AgentContext) -> anyhow::Result<()> {
      Ok(())
    }
  }

  fn probe_config() -> Config {
    Config {
      agents: vec![AgentConfig {
        name: "alice".to_string(),
        runtime: "probe".to_string(),
        script: "unused".to_string(),
      }],
      providers: HashMap::new(),
      tooling: HashMap::new(),
      runtime: HashMap::from([(
        "probe".to_string(),
        crate::config::ImplConfig {
          kind: "probe".to_string(),
          params: serde_json::json!({}),
        },
      )]),
      endpoint: None,
      memory: std::collections::BTreeMap::new(),
      tunables: crate::config::Tunables::default(),
    }
  }

  #[tokio::test]
  async fn run_agents_traced_reports_inbound_calls_and_outcome()
  -> anyhow::Result<()> {
    let mut registries = Registries::new();
    registries.runtimes.register_factory("probe", |_, _| {
      Ok(Arc::new(ProbeRuntime) as Arc<dyn crate::runtime::Runtime>)
    })?;
    let (tx, _rx) =
      tokio::sync::broadcast::channel(crate::host::trace::DEFAULT_TRACE_BUFFER);
    let events =
      run_agents_traced(&probe_config(), false, &registries, tx).await?;
    let grouped = crate::host::trace::group(events);
    let alice = grouped
      .get("alice")
      .ok_or_else(|| anyhow::anyhow!("missing alice trace"))?;
    assert_eq!(alice.outcome, Some(RunOutcome::Completed));
    assert!(alice.events.iter().any(|event| matches!(
      event,
      TraceEvent::Call { op, .. } if op == "probe"
    )));
    assert!(alice.events.iter().any(|event| matches!(
      event,
      TraceEvent::Inbound { event: Event::Message(message), .. }
        if message == "ping"
    )));
    Ok(())
  }

  /// A runtime that exits with the value of the `handle` memory key, so a test
  /// can observe what the brain saw at startup.
  struct MemoryRuntime;

  #[async_trait::async_trait]
  impl crate::runtime::Runtime for MemoryRuntime {
    fn kind() -> &'static str {
      "memory"
    }

    async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
      Ok(RunOutcome::Exited(
        ctx.memory().get("handle").unwrap_or_default(),
      ))
    }

    async fn validate(&self, _ctx: &AgentContext) -> anyhow::Result<()> {
      Ok(())
    }
  }

  fn memory_config(
    memory: std::collections::BTreeMap<
      String,
      std::collections::BTreeMap<String, String>,
    >,
  ) -> Config {
    Config {
      agents: vec![
        AgentConfig {
          name: "alice".to_string(),
          runtime: "memory".to_string(),
          script: "unused".to_string(),
        },
        AgentConfig {
          name: "bob".to_string(),
          runtime: "memory".to_string(),
          script: "unused".to_string(),
        },
      ],
      providers: HashMap::new(),
      tooling: HashMap::new(),
      runtime: HashMap::from([(
        "memory".to_string(),
        crate::config::ImplConfig {
          kind: "memory".to_string(),
          params: serde_json::json!({}),
        },
      )]),
      endpoint: None,
      memory,
      tunables: crate::config::Tunables::default(),
    }
  }

  #[tokio::test]
  async fn seeded_memory_is_visible_to_the_brain_and_per_agent()
  -> anyhow::Result<()> {
    use std::collections::BTreeMap;

    let mut registries = Registries::new();
    registries.runtimes.register_factory("memory", |_, _| {
      Ok(Arc::new(MemoryRuntime) as Arc<dyn crate::runtime::Runtime>)
    })?;
    let config = memory_config(BTreeMap::from([
      (
        "alice".to_string(),
        BTreeMap::from([("handle".to_string(), "alice-uuid".to_string())]),
      ),
      (
        "bob".to_string(),
        BTreeMap::from([("handle".to_string(), "bob-uuid".to_string())]),
      ),
    ]));
    let (tx, _rx) =
      tokio::sync::broadcast::channel(crate::host::trace::DEFAULT_TRACE_BUFFER);
    let events = run_agents_traced(&config, false, &registries, tx).await?;
    let grouped = crate::host::trace::group(events);
    assert_eq!(
      grouped
        .get("alice")
        .ok_or_else(|| anyhow::anyhow!("missing alice trace"))?
        .outcome,
      Some(RunOutcome::Exited("alice-uuid".to_string()))
    );
    assert_eq!(
      grouped
        .get("bob")
        .ok_or_else(|| anyhow::anyhow!("missing bob trace"))?
        .outcome,
      Some(RunOutcome::Exited("bob-uuid".to_string()))
    );
    Ok(())
  }
}
