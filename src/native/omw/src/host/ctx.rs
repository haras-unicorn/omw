//! Per-agent runtime context handed to a [`Runtime`](crate::runtime::Runtime).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;

use crate::host::bus::MessageBus;
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
  /// Registry of this agent's open chat streams, keyed by UUID.
  pub streams: Arc<StreamRegistry>,
  /// The tokio runtime used to bridge synchronous wasm host calls to the
  /// async provider/tooling implementations.
  rt: Option<Arc<tokio::runtime::Runtime>>,
}

impl AgentContext {
  pub fn new(
    name: String,
    script: PathBuf,
    providers: HashMap<String, ProviderEntry>,
    tooling: HashMap<String, ToolingEntry>,
    bus: Arc<MessageBus>,
    streams: Arc<StreamRegistry>,
  ) -> anyhow::Result<Self> {
    Ok(Self {
      name,
      script,
      providers,
      tooling,
      bus,
      streams,
      rt: Some(Arc::new(
        tokio::runtime::Builder::new_multi_thread()
          .enable_all()
          .build()
          .context("failed to build agent runtime")?,
      )),
    })
  }

  pub fn rt(&self) -> Arc<tokio::runtime::Runtime> {
    #[allow(clippy::unwrap_used, reason = "Always constructed as Some")]
    {
      Arc::clone(self.rt.as_ref().unwrap())
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
