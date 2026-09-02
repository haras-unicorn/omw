//! Runtime abstractions: how an agent brain is loaded and driven.

pub mod engine;
pub mod rhai;
pub mod wasm;

use std::sync::Arc;

use crate::config::ImplConfig;
use crate::host::ctx::AgentContext;

/// The terminal result of one agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
  /// The brain ran to completion.
  Completed,
  /// The brain terminated itself with a message.
  Exited(String),
}

/// A runtime loads an agent brain and drives it for one iteration.
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
  /// Which implementation this is; known statically, not bound to an instance.
  fn kind() -> &'static str
  where
    Self: Sized;

  async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome>;
}

/// Build a runtime from the agent's kind and the runtime's impl config (which
/// is optional — e.g. the rhai runtime falls back to the bundled interpreter).
pub fn build(
  kind: &str,
  impl_cfg: Option<&ImplConfig>,
) -> anyhow::Result<Arc<dyn Runtime>> {
  match kind {
    "wasm" => wasm::build(impl_cfg),
    "rhai" => rhai::build(impl_cfg),
    other => anyhow::bail!("unsupported runtime kind {other:?}"),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn factory_builds_known_kinds() -> anyhow::Result<()> {
    // `kind()` is static and not callable on a trait object, so assert only
    // that dispatch succeeds for each known kind.
    assert!(build("wasm", None).is_ok());
    assert!(build("rhai", None).is_ok());
    Ok(())
  }

  #[test]
  fn factory_rejects_unknown_kind() {
    assert!(build("nope", None).is_err());
  }
}
