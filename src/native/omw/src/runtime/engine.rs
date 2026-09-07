//! The core wasm engine: load a component, wire the host imports, call the
//! exported `runtime.run`. Deliberately generic — it has no knowledge of any
//! particular brain implementation.

use std::path::Path;

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

use crate::bindings::Omw;
use crate::bindings::omw::omw;
use crate::host::ctx::AgentContext;
use crate::host::imports::Host;

/// An engine + component pair, loaded once and reused across iterations.
#[derive(Clone)]
pub struct WasmEngine {
  engine: Engine,
  component: Component,
}

/// WASM file type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmFileType {
  Wat,
  Wasm,
  Native,
}

impl WasmEngine {
  /// Check which WASM type is at the provided path.
  pub fn wasm_file_type(path: &Path) -> Option<WasmFileType> {
    match path
      .extension()
      .map(|str| str.to_string_lossy().to_string())
      .as_deref()
    {
      Some("wat") => Some(WasmFileType::Wat),
      Some("wasm") => Some(WasmFileType::Wasm),
      Some("cwasm") => Some(WasmFileType::Native),
      _ => None,
    }
  }

  /// Load a component from one of the supported file types.
  pub fn from_path(path: &Path) -> anyhow::Result<Self> {
    match Self::wasm_file_type(path) {
      Some(WasmFileType::Wat) => Self::from_wat_path(path),
      Some(WasmFileType::Wasm) => Self::from_wasm_path(path),
      Some(WasmFileType::Native) => Self::from_native_path(path),
      None => anyhow::bail!("Unknown WASM file type at {path:?}"),
    }
  }

  /// Load a component from a WAT file path.
  pub fn from_wat_path(path: &Path) -> anyhow::Result<Self> {
    let wasm = wat::parse_file(path)?;
    Self::from_wasm_bytes(&wasm)
  }

  /// Load a component from a WASM file path.
  pub fn from_wasm_path(path: &Path) -> anyhow::Result<Self> {
    let (engine, component) =
      Self::load(|engine| Component::from_file(engine, path))?;
    Ok(Self { engine, component })
  }

  /// Load an AOT compiled component from a file path.
  pub fn from_native_path(path: &Path) -> anyhow::Result<Self> {
    let (engine, component) = Self::load(|engine| {
      #[allow(unsafe_code, reason = "need to load it somehow")]
      {
        unsafe { Component::deserialize_file(engine, path) }
      }
    })?;
    Ok(Self { engine, component })
  }

  /// Load a WASM component from an in-memory WAT byte slice.
  pub fn from_wat_bytes(bytes: &'static [u8]) -> anyhow::Result<Self> {
    let wasm = wat::parse_bytes(bytes)?;
    Self::from_wasm_bytes(&wasm)
  }

  /// Load a WASM component from an in-memory byte slice.
  pub fn from_wasm_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
    let (engine, component) =
      Self::load(|engine| Component::new(engine, bytes))?;
    Ok(Self { engine, component })
  }

  /// Load an AOT compiled WASM component from an in-memory byte slice.
  pub fn from_native_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
    let (engine, component) = Self::load(|engine| {
      #[allow(unsafe_code, reason = "need to load it somehow")]
      {
        unsafe { Component::deserialize(engine, bytes) }
      }
    })?;
    Ok(Self { engine, component })
  }

  fn load(
    f: impl FnOnce(&Engine) -> wasmtime::Result<Component>,
  ) -> anyhow::Result<(Engine, Component)> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let component = f(&engine)?;
    Ok((engine, component))
  }

  /// Instantiate the component and call the exported
  /// `runtime.check(script)`. Validates without running: no abort mapping, so
  /// any trap (including epoch) surfaces as a plain validation failure.
  /// Runs synchronously, so it must be called from a non-async thread (see
  /// the runtimes).
  pub fn check(&self, ctx: AgentContext, script: String) -> anyhow::Result<()> {
    let span = tracing::info_span!("engine.check", agent = %ctx.name);
    let _entered = span.enter();
    tracing::debug!(script, "instantiating the component for validation");
    // Deliberately not `ctx.set_engine`: validation runs while the old run
    // keeps executing, and stashing would clobber the live run's epoch
    // handle that `abort_grace` traps through.
    let mut store = Store::new(
      &self.engine,
      Host {
        ctx: ctx.clone(),
        table: Default::default(),
        wasi: <wasmtime_wasi::WasiCtxBuilder as Default>::default().build(),
      },
    );
    store.set_epoch_deadline(1);
    let mut linker: Linker<Host> = Linker::new(&self.engine);
    // The guest (wasm32-wasip2) implicitly imports `wasi:cli/*`; satisfy it.
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    omw::provider::add_to_linker::<_, HasSelf<_>>(&mut linker, |h| h)?;
    omw::tooling::add_to_linker::<_, HasSelf<_>>(&mut linker, |h| h)?;
    omw::host::add_to_linker::<_, HasSelf<_>>(&mut linker, |h| h)?;

    let instance = Omw::instantiate(&mut store, &self.component, &linker)
      .inspect_err(|e| {
        tracing::error!(error = %e, "failed to instantiate the component");
      })?;
    instance
      .omw_omw_runtime()
      .call_check(&mut store, &script)
      .map_err(|e| {
        tracing::error!(error = %e, "component runtime.check failed");
        anyhow::anyhow!(e)
      })?
      .map_err(|e| {
        tracing::error!(error = %e, "component runtime.check returned an error");
        anyhow::anyhow!(e)
      })
  }

  /// Instantiate the component and call the exported `runtime.run(script)`.
  /// Runs synchronously, so it must be called from a non-async thread (see the
  /// runtimes). Returns the terminal message the brain chose to exit with, if
  /// any.
  pub fn run(
    &self,
    ctx: AgentContext,
    script: String,
  ) -> anyhow::Result<Option<String>> {
    let span = tracing::info_span!("engine.run", agent = %ctx.name);
    let _entered = span.enter();
    tracing::debug!(script, "instantiating the component");
    ctx.set_engine(self.engine.clone());
    let mut store = Store::new(
      &self.engine,
      Host {
        ctx: ctx.clone(),
        table: Default::default(),
        wasi: <wasmtime_wasi::WasiCtxBuilder as Default>::default().build(),
      },
    );
    store.set_epoch_deadline(1);
    let mut linker: Linker<Host> = Linker::new(&self.engine);
    // The guest (wasm32-wasip2) implicitly imports `wasi:cli/*`; satisfy it.
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    omw::provider::add_to_linker::<_, HasSelf<_>>(&mut linker, |h| h)?;
    omw::tooling::add_to_linker::<_, HasSelf<_>>(&mut linker, |h| h)?;
    omw::host::add_to_linker::<_, HasSelf<_>>(&mut linker, |h| h)?;

    let instance = Omw::instantiate(&mut store, &self.component, &linker)
      .inspect_err(|e| {
        tracing::error!(error = %e, "failed to instantiate the component");
      })?;
    let host_ctx = store.data().ctx.clone();
    let result: Option<String> = instance
      .omw_omw_runtime()
      .call_run(&mut store, &script)
      .map_err(|e| map_trap(&host_ctx, e.into()))
      .map_err(|e| {
        tracing::error!(error = %e, "component runtime.run failed");
        e
      })?
      .map_err(|e| {
        tracing::error!(error = %e, "component runtime.run returned an error");
        anyhow::anyhow!(e)
      })?;
    tracing::debug!(?result, "brain run returned");
    Ok(result)
  }
}

/// Map an epoch-interruption trap to a reload/shutdown abort when one was
/// requested, so unyielding wasm loops surface as a restart instead of a
/// failure. Any other trap (or an interrupt without a pending request) is a
/// real failure.
fn map_trap(ctx: &AgentContext, error: anyhow::Error) -> anyhow::Error {
  let trap = error
    .chain()
    .any(|cause| cause.to_string().contains("epoch deadline exceeded"));
  if !trap {
    return error;
  }
  if ctx.shutdown_requested() {
    return anyhow::anyhow!("agent shutting down");
  }
  if ctx.reload_requested() {
    return anyhow::anyhow!("agent reloaded");
  }
  error
}

#[cfg(test)]
mod trap_tests {
  use super::*;
  use std::collections::HashMap;
  use std::path::PathBuf;
  use std::sync::Arc;

  use crate::host::bus::MessageBus;

  fn trap_ctx() -> anyhow::Result<AgentContext> {
    Ok(AgentContext::new(
      "test-agent".to_string(),
      PathBuf::from("unused.wasm"),
      HashMap::new(),
      HashMap::new(),
      Arc::new(MessageBus::new()),
      Arc::new(crate::host::streams::StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      None,
    )?)
  }

  #[test]
  fn epoch_trap_maps_to_reload_or_shutdown() -> anyhow::Result<()> {
    let ctx = trap_ctx()?;
    let error = anyhow::anyhow!("wasm trap: epoch deadline exceeded");
    assert!(
      map_trap(&ctx, anyhow::anyhow!("{error}"))
        .to_string()
        .contains("epoch")
    );
    ctx.request_reload();
    assert_eq!(
      map_trap(&ctx, anyhow::anyhow!("{error}")).to_string(),
      "agent reloaded"
    );
    ctx.clear_reload();
    ctx.request_shutdown();
    assert_eq!(
      map_trap(&ctx, anyhow::anyhow!("{error}")).to_string(),
      "agent shutting down"
    );
    Ok(())
  }
}

#[cfg(all(test, feature = "mock"))]
pub const WASM_MOCK_COMPONENT_WAT: &[u8] =
  include_bytes!(env!("OMW_WASM_MOCK_COMPONENT_WAT"));

#[cfg(all(test, feature = "mock"))]
pub const WASM_MOCK_COMPONENT_WASM: &[u8] =
  include_bytes!(env!("OMW_WASM_MOCK_COMPONENT_WASM"));

#[cfg(all(test, feature = "mock"))]
pub const WASM_MOCK_COMPONENT_NATIVE: &[u8] =
  include_bytes!(env!("OMW_WASM_MOCK_COMPONENT_NATIVE"));

#[cfg(test)]
mod tests {
  use serial_test::serial;
  use std::collections::HashMap;
  use std::path::PathBuf;
  use std::sync::Arc;

  use tempfile::tempdir;

  use super::*;
  use crate::host::bus::MessageBus;

  /// A minimal context; the mock brain calls no host imports, so `script` is
  /// unused by these tests.
  #[cfg(feature = "mock")]
  fn test_ctx() -> anyhow::Result<AgentContext> {
    let bus = Arc::new(MessageBus::new());
    Ok(AgentContext::new(
      "test-agent".to_string(),
      PathBuf::from("unused.wasm"),
      HashMap::new(),
      HashMap::new(),
      bus,
      Arc::new(crate::host::streams::StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      None,
    )?)
  }

  #[test]
  #[cfg(feature = "mock")]
  fn from_native_bytes_loads_mock_component() -> anyhow::Result<()> {
    let _engine = WasmEngine::from_native_bytes(WASM_MOCK_COMPONENT_NATIVE)?;
    Ok(())
  }

  #[test]
  #[cfg(feature = "mock")]
  fn from_native_path_loads_mock_component() -> anyhow::Result<()> {
    let dir = tempdir()?;

    let native = dir.path().join("brain.cwasm");
    std::fs::write(&native, WASM_MOCK_COMPONENT_NATIVE)?;
    let _engine = WasmEngine::from_native_path(&native)?;
    Ok(())
  }

  #[test]
  fn is_path_wat_detects_wat_files() {
    assert_eq!(
      WasmEngine::wasm_file_type(Path::new("brain.wat")),
      Some(WasmFileType::Wat)
    );
    assert_eq!(
      WasmEngine::wasm_file_type(Path::new("brain.wasm")),
      Some(WasmFileType::Wasm)
    );
    assert_eq!(
      WasmEngine::wasm_file_type(Path::new("brain.cwasm")),
      Some(WasmFileType::Native)
    );
    assert_eq!(WasmEngine::wasm_file_type(Path::new("brain")), None);
    assert_eq!(WasmEngine::wasm_file_type(Path::new("brain.js")), None);
  }

  #[test]
  fn from_path_with_missing_file_errors() {
    assert!(
      WasmEngine::from_wasm_path(Path::new("/nonexistent/brain.wasm")).is_err()
    );
    assert!(
      WasmEngine::from_wat_path(Path::new("/nonexistent/brain.wat")).is_err()
    );
    assert!(
      WasmEngine::from_native_path(Path::new("/nonexistent/brain.cwasm"))
        .is_err()
    );
  }

  #[test]
  #[cfg(feature = "mock")]
  fn from_path_dispatch_loads_native_component() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let native = dir.path().join("brain.cwasm");
    std::fs::write(&native, WASM_MOCK_COMPONENT_NATIVE)?;
    let _engine = WasmEngine::from_path(&native)?;
    Ok(())
  }

  #[test]
  #[serial(non_native)]
  #[cfg(feature = "mock")]
  fn from_path_dispatch_loads_wasm_component() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_ENGINE_NON_NATIVE not set");
      return Ok(());
    }

    let dir = tempdir()?;
    let wasm = dir.path().join("brain.wasm");
    std::fs::write(&wasm, WASM_MOCK_COMPONENT_WASM)?;
    let _engine = WasmEngine::from_path(&wasm)?;
    Ok(())
  }

  #[cfg(feature = "mock")]
  fn enabled_non_native() -> bool {
    std::env::var_os("OMW_TEST_WASM_ENGINE_NON_NATIVE")
      .is_some_and(|value| value != "0")
  }

  #[test]
  #[serial(non_native)]
  #[cfg(feature = "mock")]
  fn from_wat_bytes_loads_mock_component() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_ENGINE_NON_NATIVE not set");
      return Ok(());
    }

    let _engine = WasmEngine::from_wat_bytes(WASM_MOCK_COMPONENT_WAT)?;
    Ok(())
  }

  #[test]
  #[serial(non_native)]
  #[cfg(feature = "mock")]
  fn from_wasm_bytes_loads_mock_component() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_ENGINE_NON_NATIVE not set");
      return Ok(());
    }

    let _engine = WasmEngine::from_wasm_bytes(WASM_MOCK_COMPONENT_WASM)?;
    Ok(())
  }

  #[test]
  #[serial(non_native)]
  #[cfg(feature = "mock")]
  fn from_wat_path_loads_mock_component() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_ENGINE_NON_NATIVE not set");
      return Ok(());
    }

    let dir = tempdir()?;

    let wat = dir.path().join("brain.wat");
    std::fs::write(&wat, WASM_MOCK_COMPONENT_WAT)?;
    let _engine = WasmEngine::from_wat_path(&wat)?;
    Ok(())
  }

  #[test]
  #[serial(non_native)]
  #[cfg(feature = "mock")]
  fn from_wasm_path_loads_mock_component() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_ENGINE_NON_NATIVE not set");
      return Ok(());
    }

    let dir = tempdir()?;

    let wasm = dir.path().join("brain.wasm");
    std::fs::write(&wasm, WASM_MOCK_COMPONENT_WASM)?;
    let _engine = WasmEngine::from_wasm_path(&wasm)?;
    Ok(())
  }

  #[test]
  #[serial(non_native)]
  #[cfg(feature = "mock")]
  fn run_returns_none_for_mock_component() -> anyhow::Result<()> {
    if !enabled_non_native() {
      eprintln!("skipping: OMW_TEST_WASM_ENGINE_NON_NATIVE not set");
      return Ok(());
    }

    // The mock's `run` returns `Ok(None)` (it just prints and completes).
    let ctx = test_ctx()?;
    let engine = WasmEngine::from_wasm_bytes(WASM_MOCK_COMPONENT_WASM)?;
    let result = engine.run(ctx, String::new())?;
    assert_eq!(result, None);
    Ok(())
  }
}
