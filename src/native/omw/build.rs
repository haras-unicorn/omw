//! Build script: for the `rhai` and `mock` features, cross-compiles the bundled
//! guests for `wasm32-wasip2`, wraps the resulting core module into a WASM
//! component with `wasm-tools`, and embeds it into the `omw` binary.
//!
//! The guests are intentionally *not* a `[dependencies]` of `omw`: their `export!`
//! ABI (`#![no_main]` + `cabi_post_...` symbols) cannot link for the host
//! target. Instead we build it as part of `omw`'s own build, only when the
//! `rhai` (the embedded rhai interpreter) or `mock` (a test-only wasm mock
//! brain) feature is enabled. A featureless build runs no wasm tooling at all,
//! so `cargo publish` (the default crate) verifies without `wasm-tools` or a
//! `wasm32-wasip2` target.
//!
//! The nested `cargo` build uses a dedicated `--target-dir` (under `OUT_DIR`)
//! so that it does not contend for the global build lock held by the outer
//! `cargo` invocation (which would otherwise deadlock)。

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
  if env::var_os("CARGO_FEATURE_RHAI").is_some() {
    compile_guest("omw-rhai-wasm-interpreter");
  }
  if env::var_os("CARGO_FEATURE_MOCK").is_some() {
    compile_guest("omw-wasm-mock");
  }
}

fn compile_guest(guest: &str) {
  let env_prefix = guest.replace("-", "_").to_uppercase();
  let wat_env = format!("{env_prefix}_COMPONENT_WAT");
  let wasm_env = format!("{env_prefix}_COMPONENT_WASM");
  let native_env = format!("{env_prefix}_COMPONENT_NATIVE");

  let manifest_dir = PathBuf::from(
    env::var("CARGO_MANIFEST_DIR")
      .unwrap_or_else(|_| panic!("CARGO_MANIFEST_DIR not set")),
  );
  // Workspace root: <root>/src/native/omw -> <root>
  let workspace_root = manifest_dir
    .parent()
    .and_then(|p| p.parent())
    .and_then(|p| p.parent())
    .map(Path::to_path_buf)
    .unwrap_or_else(|| {
      panic!("could not find workspace root from {manifest_dir:?}")
    });

  let guest_name = guest;
  let guest_dir = workspace_root.join("src").join("wasm").join(guest_name);

  let out_dir = PathBuf::from(
    env::var("OUT_DIR").unwrap_or_else(|_| panic!("OUT_DIR not set")),
  );
  // Dedicated target dir to avoid the outer build's global lock.
  let wasm_target = out_dir.join("wasm-target");

  let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
  let release = profile == "release";

  let guest_wasm = format!("{}.wasm", guest.replace("-", "_"));
  let core_wasm = wasm_target
    .join("wasm32-wasip2")
    .join(profile)
    .join(guest_wasm);
  let component_wasm = out_dir.join(format!("{guest}.component.wasm"));
  let component_native = out_dir.join(format!("{guest}.component.cwasm"));
  let component_wat = out_dir.join(format!("{guest}.component.wat"));

  // Rebuild when the guest source or the WIT contract changes.
  println!(
    "cargo:rerun-if-changed={}",
    guest_dir.join("Cargo.toml").display()
  );
  println!("cargo:rerun-if-changed={}", guest_dir.join("src").display());
  println!(
    "cargo:rerun-if-changed={}",
    manifest_dir.join("wit").display()
  );

  // 1. Cross-compile the guest for wasm32-wasip2.
  let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
  let mut cmd = Command::new(&cargo);
  cmd
    .args([
      "build",
      "--lib",
      "--target",
      "wasm32-wasip2",
      "-p",
      guest_name,
      "--target-dir",
    ])
    .arg(&wasm_target);
  if release {
    cmd.arg("--release");
  }
  let status = cmd
    .status()
    .unwrap_or_else(|e| panic!("failed to run {cargo}: {e}"));
  assert!(
    status.success(),
    "cross-build of {guest_name} for wasm32-wasip2 failed \
     (is the 'wasm32-wasip2' target installed?)"
  );

  // 2. Ensure `core_wasm` is a WASM *component*.
  //
  // The `wasm32-wasip2` target emits a component directly (component model
  // version = 0x0d), but verify the magic so that environments producing a
  // bare core module (component version gap) still work: if it's a core
  // module (version = 0x01), wrap it with `wasm-tools component new`.
  let header = std::fs::read(&core_wasm)
    .unwrap_or_else(|e| panic!("failed to read {}: {e}", core_wasm.display()));
  let is_core_module = header.len() >= 8
    && header[..4] == [0, b'a', b's', b'm']
    && header[4..8] == [1, 0, 0, 0];
  if is_core_module {
    let status = Command::new("wasm-tools")
      .args(["component", "new"])
      .arg(&core_wasm)
      .args(["-o"])
      .arg(&component_wasm)
      .status()
      .unwrap_or_else(|e| panic!("failed to run wasm-tools: {e}"));
    assert!(
      status.success(),
      "wasm-tools component new failed (is 'wasm-tools' on PATH?)"
    );
  } else {
    std::fs::copy(&core_wasm, &component_wasm).unwrap_or_else(|e| {
      panic!("failed to copy component to {:?}: {e}", component_wasm)
    });
  }

  // 3. Convert component WASM to WAT.
  let status = Command::new("wasm-tools")
    .args(["print"])
    .arg(&component_wasm)
    .arg("-o")
    .arg(&component_wat)
    .status()
    .unwrap_or_else(|e| panic!("failed to run wasm-tools print: {e}"));
  assert!(status.success(), "wasm-tools print failed");

  // 4. Compile the component AOT.
  let engine = wasmtime::Engine::default();
  let component =
    wasmtime::component::Component::from_file(&engine, &component_wasm)
      .unwrap_or_else(|_| panic!("failed compiling {guest} guest component"));
  let native = component
    .serialize()
    .unwrap_or_else(|_| panic!("failed serializing {guest} guest component"));
  std::fs::write(&component_native, native)
    .unwrap_or_else(|_| panic!("failed writing {guest} guest"));

  // 5. Make the component path available to `include_bytes!` at compile time.
  println!("cargo:rustc-env={}={}", wat_env, component_wat.display());
  println!("cargo:rustc-env={}={}", wasm_env, component_wasm.display());
  println!(
    "cargo:rustc-env={}={}",
    native_env,
    component_native.display()
  );
}
