//! The wasm brain runtime: loads the agent's `.wasm` component (which
//! implements the exported `runtime` interface) and runs it.

use std::sync::Arc;
use std::sync::Mutex;

use crate::host::ctx::AgentContext;
use crate::runtime::engine::WasmEngine;
use crate::runtime::{RunOutcome, Runtime};
use anyhow::Context as _;
use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Config {}

/// Loads the agent's wasm brain from `ctx.script` and runs it.
#[derive(Clone, Default)]
pub struct WasmRuntime {
  pub name: String,
  pub config: Config,
  /// Last successfully validated engine, kept as a TOCTOU backstop: a save
  /// landing between validate and load still falls back to this instead of
  /// failing the run.
  last_good: Arc<Mutex<Option<WasmEngine>>>,
}

pub fn build(name: &str, params: &Value) -> anyhow::Result<Arc<dyn Runtime>> {
  Ok(Arc::new(WasmRuntime {
    name: name.to_owned(),
    config: Config::deserialize(params.into_deserializer())?,
    last_good: Arc::new(Mutex::new(None)),
  }))
}

#[async_trait::async_trait]
impl Runtime for WasmRuntime {
  fn kind() -> &'static str {
    "wasm"
  }

  async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
    tracing::debug!(agent = %ctx.name, script = %ctx.script.display(), "loading the wasm brain");
    let engine = match WasmEngine::from_path(&ctx.script)
      .with_context(|| format!("failed to load wasm brain {:?}", ctx.script))
    {
      Ok(engine) => engine,
      Err(error) => {
        if let Ok(slot) = self.last_good.lock()
          && let Some(cached) = slot.clone()
        {
          tracing::warn!(agent = %ctx.name, error = %error, "wasm brain changed underfoot, running the last-good component");
          cached
        } else {
          return Err(error);
        }
      }
    };
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

  async fn validate(&self, ctx: &AgentContext) -> anyhow::Result<()> {
    let engine = WasmEngine::from_path(&ctx.script)
      .with_context(|| format!("failed to load wasm brain {:?}", ctx.script))?;
    let ctx_clone = ctx.clone();
    tokio::task::spawn_blocking(move || engine.check(ctx_clone, String::new()))
      .await
      .context("wasm check task failed")??;
    if let Ok(fresh) = WasmEngine::from_path(&ctx.script)
      && let Ok(mut slot) = self.last_good.lock()
    {
      *slot = Some(fresh);
    }
    Ok(())
  }
}

#[cfg(test)]
#[cfg(feature = "mock")]
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
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      None,
    )?;

    let runtime = WasmRuntime::default();
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

  fn validate(script: PathBuf) -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let ctx = AgentContext::new(
      "test-agent".to_string(),
      script,
      HashMap::new(),
      HashMap::new(),
      bus,
      Arc::new(crate::host::streams::StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      None,
    )?;
    let runtime = WasmRuntime::default();
    let rt = tokio::runtime::Builder::new_multi_thread()
      .enable_all()
      .build()?;
    rt.block_on(runtime.validate(&ctx))
  }

  #[test]
  #[serial(non_native)]
  fn validate_ok_and_err() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_RUNTIME_NON_NATIVE not set");
      return Ok(());
    }
    let dir = tempdir()?;
    let wasm = dir.path().join("brain.wasm");
    std::fs::write(&wasm, WASM_MOCK_COMPONENT_WASM)?;
    validate(wasm)?;
    let missing = dir.path().join("missing.wasm");
    assert!(validate(missing).is_err());
    Ok(())
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
