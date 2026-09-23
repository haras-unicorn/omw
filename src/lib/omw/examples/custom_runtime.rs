//! Custom runtime: a pure-Rust inline `Runtime` that bypasses the
//! WASM engine entirely.
//!
//! Library-only teaching material and the escape hatch for brains
//! that do not need a guest script. The inline TOML wires
//! `[runtime.inline] kind = "inline"` plus one agent; the example
//! registers the back end with `register_runtimes!`, runs the agent
//! through `run_agents`, and asserts the run completed before
//! exiting 0.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use omw::prelude::*;

/// Set once the inline runtime's `run` executes.
static RAN: AtomicBool = AtomicBool::new(false);

/// A pure-Rust runtime with no guest script and no WASM engine.
struct InlineRuntime;

impl omw::runtime::Factory for InlineRuntime {
  fn build(
    _name: &str,
    _params: &serde_json::Value,
  ) -> anyhow::Result<Arc<Self>> {
    Ok(Arc::new(Self))
  }
}

#[async_trait::async_trait]
impl Runtime for InlineRuntime {
  fn kind() -> &'static str {
    "inline"
  }

  async fn run(&self, _ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
    RAN.store(true, Ordering::Relaxed);
    Ok(RunOutcome::Completed)
  }

  async fn validate(&self, _ctx: &AgentContext) -> anyhow::Result<()> {
    Ok(())
  }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let raw = r#"
[runtime.inline]
kind = "inline"

[[agents]]
name = "agent"
runtime = "inline"
script = "brain.txt"
"#;
  let cfg: Config = toml::from_str(raw)?;
  let mut registries = Registries::new();
  omw::register_runtimes!(registries.runtimes, InlineRuntime);
  run_agents(&cfg, false, &registries).await?;
  anyhow::ensure!(
    RAN.load(Ordering::Relaxed),
    "expected the inline runtime to run"
  );
  Ok(())
}
