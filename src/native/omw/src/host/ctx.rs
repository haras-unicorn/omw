//! Per-agent runtime context handed to a [`Runtime`](crate::runtime::Runtime).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;

use crate::config::Tunables;
use crate::host::bus::MessageBus;
use crate::host::endpoint::EndpointRegistry;
use crate::host::memory::Memory;
use crate::host::streams::CancelRegistry;
use crate::host::streams::StreamRegistry;
use crate::provider::ProviderEntry;
use crate::tooling::ToolingEntry;

/// Everything a runtime needs to execute one agent for one iteration.
///
/// It is cheap to clone (all fields are reference-counted or plain data) so
/// runtimes can move it across threads (e.g. into `spawn_blocking` for sync
/// wasm execution).
#[derive(Clone)]
pub struct AgentContext {
  pub name: String,
  /// The agent's brain file.
  pub script: PathBuf,
  /// Every configured provider, keyed by name.
  pub providers: HashMap<String, ProviderEntry>,
  /// Every configured tooling, keyed by name.
  pub tooling: HashMap<String, ToolingEntry>,
  pub bus: Arc<MessageBus>,
  /// Per-agent memory that survives hot reloads (same context is reused
  /// across reload iterations). Scoped to this agent only.
  pub memory: Arc<Memory>,
  /// Registry of this agent's open chat streams, keyed by UUID.
  pub streams: Arc<StreamRegistry>,
  /// Registry of this agent's pending timers, keyed by UUID.
  pub timers: Arc<CancelRegistry>,
  /// Registry of this agent's open resource subscriptions, keyed by UUID.
  pub resources: Arc<CancelRegistry>,
  /// Registry of this agent's in-flight tool calls, keyed by UUID.
  pub tool_calls: Arc<CancelRegistry>,
  /// The shared endpoint session registry, present when the endpoint HTTP
  /// server is configured. `None` when the agent cannot use the `endpoint-*`
  /// host imports (they error out).
  pub endpoint: Option<Arc<EndpointRegistry>>,
  /// The tokio runtime used to bridge synchronous wasm host calls to the
  /// async provider/tooling implementations.
  rt: Option<Arc<tokio::runtime::Runtime>>,
  tunables: Tunables,
  /// Cooperative reload flag, set by the file watcher. The blocking
  /// `host.recv` polls it every slice without draining the inbox, so a
  /// reload aborts the wait while queued events survive for the next run.
  reload: Arc<AtomicBool>,
  /// Cooperative shutdown flag, set on SIGTERM/SIGINT. Behaves like reload
  /// but is terminal: the supervisor does not restart the run.
  shutdown: Arc<AtomicBool>,
  /// The engine of the currently running `Store`, stashed by
  /// `WasmEngine::run` so the supervisor can trap unyielding wasm loops with
  /// `increment_epoch` once the grace expires.
  engine: Arc<Mutex<Option<wasmtime::Engine>>>,
}

impl AgentContext {
  #[allow(
    clippy::too_many_arguments,
    reason = "aggregating the per-agent registries into a struct is left to a ctx refactor"
  )]
  pub fn new(
    name: String,
    script: PathBuf,
    providers: HashMap<String, ProviderEntry>,
    tooling: HashMap<String, ToolingEntry>,
    bus: Arc<MessageBus>,
    streams: Arc<StreamRegistry>,
    timers: Arc<CancelRegistry>,
    resources: Arc<CancelRegistry>,
    tool_calls: Arc<CancelRegistry>,
    endpoint: Option<Arc<EndpointRegistry>>,
  ) -> anyhow::Result<Self> {
    Self::with_tunables(
      name,
      script,
      providers,
      tooling,
      bus,
      streams,
      timers,
      resources,
      tool_calls,
      endpoint,
      Tunables::default(),
    )
  }

  #[allow(
    clippy::too_many_arguments,
    reason = "aggregating the per-agent registries into a struct is left to a ctx refactor"
  )]
  pub fn with_tunables(
    name: String,
    script: PathBuf,
    providers: HashMap<String, ProviderEntry>,
    tooling: HashMap<String, ToolingEntry>,
    bus: Arc<MessageBus>,
    streams: Arc<StreamRegistry>,
    timers: Arc<CancelRegistry>,
    resources: Arc<CancelRegistry>,
    tool_calls: Arc<CancelRegistry>,
    endpoint: Option<Arc<EndpointRegistry>>,
    tunables: Tunables,
  ) -> anyhow::Result<Self> {
    Ok(Self {
      name,
      script,
      providers,
      tooling,
      bus,
      memory: Arc::new(Memory::new()),
      streams,
      timers,
      resources,
      tool_calls,
      endpoint,
      tunables,
      reload: Arc::new(AtomicBool::new(false)),
      shutdown: Arc::new(AtomicBool::new(false)),
      engine: Arc::new(Mutex::new(None)),
      rt: Some(Arc::new(
        tokio::runtime::Builder::new_multi_thread()
          .enable_all()
          .build()
          .context("failed to build agent runtime")?,
      )),
    })
  }

  pub fn tunables(&self) -> Tunables {
    self.tunables
  }

  /// Whether a reload has been requested (and not yet cleared).
  pub fn reload_requested(&self) -> bool {
    self.reload.load(Ordering::Relaxed)
  }

  /// Whether a shutdown has been requested (and not yet cleared).
  pub fn shutdown_requested(&self) -> bool {
    self.shutdown.load(Ordering::Relaxed)
  }

  /// Signal this agent's run to reload: the blocking `host.recv` aborts
  /// with a reload error, waking the brain out of its wait.
  pub fn request_reload(&self) {
    self.reload.store(true, Ordering::Relaxed);
  }

  /// Signal this agent's run to shut down terminally.
  pub fn request_shutdown(&self) {
    self.shutdown.store(true, Ordering::Relaxed);
  }

  /// Clear previously requested reload/shutdown flags, so the next iteration
  /// starts clean.
  pub fn clear_reload(&self) {
    self.reload.store(false, Ordering::Relaxed);
    self.shutdown.store(false, Ordering::Relaxed);
  }

  /// Stash the engine of the currently running `Store` for epoch trapping.
  pub fn set_engine(&self, engine: wasmtime::Engine) {
    if let Ok(mut slot) = self.engine.lock() {
      *slot = Some(engine);
    }
  }

  /// Trap the running wasm `Store` by incrementing the stashed engine epoch.
  /// Only fires after a reload/shutdown armed the deadline and the grace
  /// expired; a no-op when no run is active.
  pub fn increment_epoch(&self) {
    if let Ok(slot) = self.engine.lock()
      && let Some(engine) = slot.as_ref()
    {
      engine.increment_epoch();
    }
  }

  pub fn rt(&self) -> Arc<tokio::runtime::Runtime> {
    #[allow(clippy::unwrap_used, reason = "Always constructed as Some")]
    {
      Arc::clone(self.rt.as_ref().unwrap())
    }
  }

  /// Run `future` on the bridge runtime from the synchronous wasm thread,
  /// aborting early with `"agent reloaded"` / `"agent shutting down"` when
  /// requested. Upstream work may still run to completion; its result is
  /// dropped.
  pub fn block_on_reload<T: Send + 'static>(
    &self,
    future: impl std::future::Future<Output = T> + Send + 'static,
  ) -> Result<T, String> {
    let rt = self.rt();
    let mut handle = rt.spawn(future);
    loop {
      if self.shutdown_requested() {
        handle.abort();
        return Err("agent shutting down".to_string());
      }
      if self.reload_requested() {
        handle.abort();
        return Err("agent reloaded".to_string());
      }
      let poll = self.tunables.reload_poll();
      let settled = rt.block_on(async {
        tokio::select! {
          biased;
          done = &mut handle => Some(done),
          () = tokio::time::sleep(poll) => None,
        }
      });
      if let Some(done) = settled {
        return done.map_err(|_| {
          if self.shutdown_requested() {
            "agent shutting down".to_string()
          } else {
            "agent reloaded".to_string()
          }
        });
      }
    }
  }
}

impl std::fmt::Debug for AgentContext {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("AgentContext")
      .field("name", &self.name)
      .field("script", &self.script)
      .field("providers", &self.providers)
      .field("tooling", &self.tooling)
      .field("memory", &self.memory)
      .finish_non_exhaustive()
  }
}

impl Drop for AgentContext {
  fn drop(&mut self) {
    if let Some(rt) = self.rt.take() {
      if tokio::runtime::Handle::try_current().is_ok() {
        futures::executor::block_on(async move {
          if let Err(e) = tokio::task::spawn_blocking(move || drop(rt)).await {
            tracing::error!(error = %e, "failed to drop agent runtime");
          }
        });
      } else {
        drop(rt);
      }
    }
  }
}
