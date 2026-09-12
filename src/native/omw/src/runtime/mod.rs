//! Runtime abstractions: how an agent brain is loaded and driven.

#[cfg(feature = "wasm")]
mod bindings;
#[cfg(feature = "wasm")]
pub mod engine;
#[cfg(feature = "wasm")]
mod host;
#[cfg(feature = "js")]
pub mod js;
#[cfg(feature = "rhai")]
pub mod rhai;
#[cfg(feature = "wasm")]
pub mod wasm;

use crate::host::ctx::AgentContext;
use serde_json::Value;
use std::sync::Arc;

/// The terminal result of one agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
  /// The brain ran to completion.
  Completed,
  /// The brain terminated itself with a message.
  Exited(String),
}

/// A configured runtime instance: the impl plus its config-derived name and
/// static kind.
#[derive(Clone)]
pub struct RuntimeEntry {
  pub name: String,
  pub kind: String,
  pub runtime: Arc<dyn Runtime>,
}

/// A runtime loads an agent brain and drives it for one iteration.
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
  /// Which implementation this is; known statically, not bound to an instance.
  fn kind() -> &'static str
  where
    Self: Sized;

  async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome>;

  /// Check the agent's current brain script for validity without running it.
  /// Used by hot reload (and startup) to validate an edit before aborting
  /// the live run.
  async fn validate(&self, ctx: &AgentContext) -> anyhow::Result<()>;
}

/// Build a runtime from the agent's kind and the runtime's impl config.
pub fn build(
  name: &str,
  kind: &str,
  params: &Value,
) -> anyhow::Result<RuntimeEntry> {
  let runtime = match kind {
    #[cfg(feature = "wasm")]
    "wasm" => wasm::build(name, params),
    #[cfg(feature = "rhai")]
    "rhai" => rhai::build(name, params),
    #[cfg(feature = "js")]
    "js" => js::build(name, params),
    other => anyhow::bail!("unsupported runtime kind {other:?}"),
  }?;

  Ok(RuntimeEntry {
    name: name.to_owned(),
    kind: kind.to_owned(),
    runtime,
  })
}

#[cfg(test)]
mod tests {
  use serde_json::Map;

  use super::*;

  #[test]
  fn factory_builds_known_kinds() -> anyhow::Result<()> {
    #[cfg(feature = "wasm")]
    assert!(build("wasm", "wasm", &Value::Object(Map::new())).is_ok());
    #[cfg(feature = "rhai")]
    assert!(build("rhai", "rhai", &Value::Object(Map::new())).is_ok());
    #[cfg(feature = "js")]
    assert!(build("js", "js", &Value::Object(Map::new())).is_ok());
    Ok(())
  }

  #[test]
  fn factory_rejects_unknown_kind() {
    assert!(build("nope", "nope", &Value::Object(Map::new())).is_err());
  }
}
