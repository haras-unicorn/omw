//! Brain examples harness: drives the real on-disk `examples/<name>/`
//! brains through `run_agents` once, using the same in-process doubles
//! as `tests/agents.rs` — a wiremock OpenAI provider plus an in-process
//! rmcp streamable-HTTP echo server.
//!
//! One test discovers its cases at runtime by listing `examples/*/`
//! (three flavors per example: `rhai`, `js`, `wasm`), so adding or
//! renaming an example needs no harness change. `OMW_EXAMPLE_FILTER`
//! selects a subset by substring match on `<example>/<flavor>`
//! (`dev brain example <example> <flavor>` sets it to exactly that;
//! `dev examples` loops every example one by one the same way).
//!
//! The four `examples/<name>/` directories do not exist yet, so the
//! test currently finds zero cases and passes. As each brain example
//! lands (all three variants plus TOMLs and a README in one step),
//! `run_case` starts driving the on-disk `brain.rhai` / `brain.js` /
//! compiled rust `brain.wasm` through `run_agents` and asserting the
//! terminal outcome.
//!
//! Only the rust `.wasm` flavor is heavy (nested
//! `cargo build --target wasm32-wasip2` + `wasm-tools`, like `build.rs`
//! does), so only those cases skip behind the existing
//! `OMW_TEST_WASM_RUNTIME_NON_NATIVE` var.

use std::path::PathBuf;

/// The repo root, three levels above this file's crate manifest dir.
fn repo_root() -> PathBuf {
  // `CARGO_MANIFEST_DIR` is `src/lib/omw` during `cargo test -p omw`.
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("..")
    .join("..")
    .join("..")
}

/// Substring selection on `<example>/<flavor>`; empty or unset runs all.
fn selected(example: &str, flavor: &str) -> bool {
  let Ok(filter) = std::env::var("OMW_EXAMPLE_FILTER") else {
    return true;
  };
  if filter.is_empty() {
    return true;
  }
  format!("{example}/{flavor}").contains(filter.as_str())
}

/// List every `<example>/<flavor>` case by scanning `examples/*/`; three
/// flavors per example directory, sorted for deterministic runs.
fn discover() -> anyhow::Result<Vec<(String, String)>> {
  let root = repo_root().join("examples");
  if !root.exists() {
    return Ok(Vec::new());
  }
  let mut examples: Vec<String> = Vec::new();
  for entry in std::fs::read_dir(&root)? {
    let entry = entry?;
    if !entry.file_type()?.is_dir() {
      continue;
    }
    let Ok(name) = entry.file_name().into_string() else {
      continue;
    };
    examples.push(name);
  }
  examples.sort_unstable();
  let mut cases = Vec::new();
  for example in examples {
    for flavor in ["rhai", "js", "wasm"] {
      cases.push((example.clone(), flavor.to_string()));
    }
  }
  Ok(cases)
}

/// Only the rust `.wasm` flavor is heavy (nested `wasm32-wasip2`
/// builds): skip it unless the non-native gate is set, the same gate
/// the engine and `rhai.rs` / `js.rs` unit tests already use.
fn enabled_non_native() -> bool {
  std::env::var_os("OMW_TEST_WASM_RUNTIME_NON_NATIVE")
    .is_some_and(|value| value != "0")
}

/// Placeholder until the brain example lands: a landed-but-unwired
/// example fails loudly instead of silently passing.
async fn run_case(example: &str, flavor: &str) -> anyhow::Result<()> {
  anyhow::bail!(
    "brain example {example:?} ({flavor}) landed but is not wired yet"
  )
}

#[tokio::test]
async fn brain_examples() -> anyhow::Result<()> {
  let mut cases = discover()?;
  cases.retain(|(example, flavor)| selected(example, flavor));
  if cases.is_empty() {
    eprintln!("brain examples: no cases selected");
    return Ok(());
  }
  let mut failures: Vec<String> = Vec::new();
  for (example, flavor) in &cases {
    if flavor == "wasm" && !enabled_non_native() {
      eprintln!("skipping: {example}/{flavor} (non-native gate not set)");
      continue;
    }
    eprintln!("brain example: {example}/{flavor}");
    if let Err(error) = run_case(example, flavor).await {
      failures.push(format!("{example}/{flavor}: {error:#}"));
    }
  }
  if failures.is_empty() {
    Ok(())
  } else {
    anyhow::bail!(
      "{} brain example(s) failed:\n{}",
      failures.len(),
      failures.join("\n")
    )
  }
}
