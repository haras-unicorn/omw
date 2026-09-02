//! The wasm brain runtime: loads the agent's `.wasm` component (which
//! implements the exported `runtime` interface) and runs it.

use std::sync::Arc;

use anyhow::Context as _;

use crate::config::ImplConfig;
use crate::host::ctx::AgentContext;
use crate::runtime::engine::WasmEngine;
use crate::runtime::{RunOutcome, Runtime};

/// Loads the agent's wasm brain from `ctx.script` and runs it.
#[derive(Clone, Default)]
pub struct WasmRuntime;

pub fn build(_cfg: Option<&ImplConfig>) -> anyhow::Result<Arc<dyn Runtime>> {
  Ok(Arc::new(WasmRuntime))
}

#[async_trait::async_trait]
impl Runtime for WasmRuntime {
  fn kind() -> &'static str {
    "wasm"
  }

  async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
    tracing::debug!(agent = %ctx.name, script = %ctx.script.display(), "loading the wasm brain");
    let engine = WasmEngine::from_path(&ctx.script)
      .with_context(|| format!("failed to load wasm brain {:?}", ctx.script))?;
    let ctx = ctx.clone();

    // The wasm engine here is synchronous; push it off the tokio worker so
    // the host imports (which use `Runtime::block_on`) run on a thread that
    // is not itself inside a tokio runtime. The script argument is unused by
    // wasm brains; their brain is the component itself.
    let outcome =
      tokio::task::spawn_blocking(move || engine.run(ctx, String::new()))
        .await
        .context("wasm brain task failed")??;

    Ok(outcome.map_or(RunOutcome::Completed, RunOutcome::Exited))
  }
}

#[cfg(test)]
mod tests {
  use serial_test::serial;
  use std::collections::HashMap;
  use std::path::PathBuf;
  use std::sync::Arc;

  use tempfile::tempdir;

  use super::*;
  use crate::host::bus::MessageBus;
  use crate::runtime::engine::{
    WASM_MOCK_COMPONENT_WASM, WASM_MOCK_COMPONENT_WAT,
  };

  fn test_script(script: PathBuf) -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let ctx = AgentContext::new(
      "test-agent".to_string(),
      script,
      HashMap::new(),
      HashMap::new(),
      bus,
      Arc::new(crate::host::streams::StreamRegistry::new()),
    )?;

    let runtime = WasmRuntime;
    let rt = tokio::runtime::Builder::new_multi_thread()
      .enable_all()
      .build()?;

    let outcome = rt.block_on(runtime.run(&ctx))?;
    assert_eq!(outcome, RunOutcome::Completed);
    Ok(())
  }

  fn enabled_non_native() -> bool {
    std::env::var_os("OMW_TEST_WASM_RUNTIME_NON_NATIVE")
      .is_some_and(|value| value != "0")
  }

  #[test]
  #[serial(non_native)]
  fn runs_mock_component_wasm() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_RUNTIME_NON_NATIVE not set");
      return Ok(());
    }

    let dir = tempdir()?;
    let wasm = dir.path().join("brain.wasm");
    std::fs::write(&wasm, WASM_MOCK_COMPONENT_WASM)?;
    test_script(wasm)?;
    Ok(())
  }

  #[test]
  #[serial(non_native)]
  fn runs_mock_component_wat() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_RUNTIME_NON_NATIVE not set");
      return Ok(());
    }

    let dir = tempdir()?;
    let wat = dir.path().join("brain.wat");
    std::fs::write(&wat, WASM_MOCK_COMPONENT_WAT)?;
    test_script(wat)?;
    Ok(())
  }
}
