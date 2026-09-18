//! Brain examples harness: drives the real on-disk `examples/<name>/`
//! brains through `run_agents` once, using the same in-process doubles
//! as `tests/agents.rs` — a wiremock OpenAI provider plus an in-process
//! rmcp streamable-HTTP echo server.
//!
//! The four `examples/<name>/` directories do not exist yet, so every
//! case currently passes after asserting their absence. As each brain
//! example lands (all three variants plus TOMLs and a README in one
//! step), its cases start driving the on-disk `brain.rhai` / `brain.js`
//! / compiled rust `brain.wasm` through `run_agents` and assert the
//! terminal outcome.
//!
//! Only the rust `.wasm` variant cases are heavy (nested
//! `cargo build --target wasm32-wasip2` + `wasm-tools`, like `build.rs`
//! does), so only those skip behind the existing
//! `OMW_TEST_WASM_RUNTIME_NON_NATIVE` var. `dev brain example
//! <example> <flavor>` runs one case (e.g.
//! `dev brain example 01-hello rhai`).

use std::path::PathBuf;

/// The repo root, three levels above this file's crate manifest dir.
fn repo_root() -> PathBuf {
  // `CARGO_MANIFEST_DIR` is `src/lib/omw` during `cargo test -p omw`.
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("..")
    .join("..")
    .join("..")
}

fn brain_dir(example: &str) -> PathBuf {
  repo_root().join("examples").join(example)
}

/// Only the rust `.wasm` cases are heavy (nested `wasm32-wasip2`
/// builds): skip them unless the non-native gate is set, the same gate
/// the engine and `rhai.rs` / `js.rs` unit tests already use.
fn enabled_non_native() -> bool {
  std::env::var_os("OMW_TEST_WASM_RUNTIME_NON_NATIVE")
    .is_some_and(|value| value != "0")
}

/// Placeholder until the brain example lands: assert the directory is
/// still absent, so a landed-but-unwired example fails loudly instead
/// of silently passing.
fn assert_not_yet_present(example: &str) -> anyhow::Result<()> {
  let dir = brain_dir(example);
  assert!(
    !dir.exists(),
    "brain example {example:?} landed at {dir:?} but its harness case is still a placeholder",
  );
  Ok(())
}

macro_rules! brain_case {
  ($example:literal, $test_name:ident) => {
    #[tokio::test]
    async fn $test_name() -> anyhow::Result<()> {
      assert_not_yet_present($example)
    }
  };
}

macro_rules! wasm_brain_case {
  ($example:literal, $test_name:ident) => {
    #[tokio::test]
    async fn $test_name() -> anyhow::Result<()> {
      if !enabled_non_native() {
        eprintln!("skipping: OMW_TEST_WASM_RUNTIME_NON_NATIVE not set");
        return Ok(());
      }
      assert_not_yet_present($example)
    }
  };
}

brain_case!("01-hello", example_01_hello_rhai);
brain_case!("01-hello", example_01_hello_js);
wasm_brain_case!("01-hello", example_01_hello_wasm);

brain_case!("02-tool-agent", example_02_tool_agent_rhai);
brain_case!("02-tool-agent", example_02_tool_agent_js);
wasm_brain_case!("02-tool-agent", example_02_tool_agent_wasm);

brain_case!("03-endpoint", example_03_endpoint_rhai);
brain_case!("03-endpoint", example_03_endpoint_js);
wasm_brain_case!("03-endpoint", example_03_endpoint_wasm);

brain_case!("04-ping-pong", example_04_ping_pong_rhai);
brain_case!("04-ping-pong", example_04_ping_pong_js);
wasm_brain_case!("04-ping-pong", example_04_ping_pong_wasm);
