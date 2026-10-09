//! Build helpers shared across the OMW crates.
//!
//! # Not a library
//!
//! This crate is an implementation detail of the OMW crates. It is published
//! only so the `omw-runtime-*` crates and `omw-test` can depend on it from the
//! registry; it is **not** a supported library API and may change without
//! notice. Use the `omw` crate instead.
//!
//! It backs two internal workflows:
//!
//! - [`build_component`], called from each `omw-runtime-*` crate's `build.rs` to
//!   cross-compile its bundled guest for `wasm32-wasip2`, wrap the resulting
//!   core module into a WASM component when necessary, and expose the
//!   **portable** component to the crate via `cargo:rustc-env`. The component is
//!   deliberately left as portable wasm (never AOT-compiled to a `cwasm`): the
//!   runtime compiles it in-process at startup, so the embedding binary stays
//!   portable across machines and CPU feature sets.
//!
//! - [`build_brains`], called by the `omw-test compile-wasm` dev helper to
//!   cross-build single-file Rust brains against the in-repo `omw-wasm-rust`
//!   SDK.
//!
//! For `build_component`, the guest source lives outside the component package
//! (`<root>/src/wasm/*`), so a published crate cannot cross-build it. To make
//! the published crate self-contained, setting `OMW_WASM_BUILD_VENDORED` makes
//! this helper also copy the produced component into `<package>/wasm/`. A
//! packaged crate - a registry checkout, or a copy extracted under
//! `<workspace>/target/package` while `cargo publish` verifies it - has no guest
//! source under its own `<workspace>/src/lib`, so this helper embeds the
//! vendored component instead of cross-compiling. Only the release prebuild sets
//! that variable, so normal development never writes into the package.
//!
//! The nested `cargo` builds use a dedicated `--target-dir` so that they do not
//! contend for the global build lock held by the outer `cargo` invocation (which
//! would otherwise deadlock).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

mod brain;

pub use brain::build_brains;

/// Builds the `guest` component for the calling crate's `build.rs`, exposing it
/// as a portable wasm component through `<PREFIX>_COMPONENT_WASM` (and its WAT
/// via `<PREFIX>_COMPONENT_WAT`), where `<PREFIX>` is the guest's upper-cased,
/// underscore-separated name.
pub fn build_component(guest: &str) {
  let env_prefix = guest.replace('-', "_").to_uppercase();
  let wasm_env = format!("{env_prefix}_COMPONENT_WASM");
  let wat_env = format!("{env_prefix}_COMPONENT_WAT");

  let manifest_dir = PathBuf::from(require_env("CARGO_MANIFEST_DIR"));
  let out_dir = PathBuf::from(require_env("OUT_DIR"));
  let release = env::var("PROFILE").is_ok_and(|profile| profile == "release");

  let component_wasm = out_dir.join(format!("{guest}.component.wasm"));
  let component_wat = out_dir.join(format!("{guest}.component.wat"));
  let vendored_dir = manifest_dir.join("wasm");
  let vendored_component = vendored_dir.join(format!("{guest}.component.wasm"));
  let vendoring = env::var_os("OMW_WASM_BUILD_VENDORED").is_some();

  match find_guest(&manifest_dir, guest) {
    Some(guest_dir) => {
      compile_from_source(
        guest,
        &guest_dir,
        &out_dir.join("wasm-target"),
        release,
        &component_wasm,
      );
      if vendoring {
        std::fs::create_dir_all(&vendored_dir).unwrap_or_else(|e| {
          panic!("failed to create {}: {e}", vendored_dir.display())
        });
        std::fs::copy(&component_wasm, &vendored_component).unwrap_or_else(
          |e| {
            panic!(
              "failed to vendor {} to {}: {e}",
              component_wasm.display(),
              vendored_component.display()
            )
          },
        );
      }
    }
    None => {
      assert!(
        vendored_component.is_file(),
        "guest sources are absent (published crate) and the vendored \
         component is missing at {}; set OMW_WASM_BUILD_VENDORED=1 during the \
         release prebuild",
        vendored_component.display()
      );
      println!("cargo:rerun-if-changed={}", vendored_component.display());
      std::fs::copy(&vendored_component, &component_wasm).unwrap_or_else(|e| {
        panic!(
          "failed to copy vendored component {} to {}: {e}",
          vendored_component.display(),
          component_wasm.display()
        )
      });
    }
  }

  let wat = wasmprinter::print_bytes(
    std::fs::read(&component_wasm).unwrap_or_else(|e| {
      panic!("failed to read {}: {e}", component_wasm.display())
    }),
  )
  .unwrap_or_else(|e| panic!("failed to print {guest} component to wat: {e}"));
  std::fs::write(&component_wat, wat).unwrap_or_else(|e| {
    panic!("failed to write {}: {e}", component_wat.display())
  });

  println!("cargo:rustc-env={wasm_env}={}", component_wasm.display());
  println!("cargo:rustc-env={wat_env}={}", component_wat.display());
}

fn require_env(key: &str) -> String {
  env::var(key).unwrap_or_else(|_| panic!("{key} not set"))
}

/// Finds `<workspace>/src/wasm/<guest>` by walking up from `manifest_dir`, so
/// the same helper works from any `src/lib/omw-runtime-*` crate. The candidate
/// only counts when `manifest_dir` is itself under that workspace's `src/lib`,
/// so a packaged copy - extracted under `<workspace>/target/package` during
/// `cargo publish` verification - never walks back into the live workspace
/// source. Returns `None` for a published crate whose guest source is absent.
fn find_guest(manifest_dir: &Path, guest: &str) -> Option<PathBuf> {
  let mut current = Some(manifest_dir);
  while let Some(dir) = current {
    if manifest_dir.starts_with(dir.join("src").join("lib")) {
      let candidate = dir.join("src").join("wasm").join(guest);
      if candidate.is_dir() {
        return Some(candidate);
      }
    }
    current = dir.parent();
  }
  None
}

/// Cross-builds `guest` from its in-repo source at `guest_dir` into the portable
/// component at `component_wasm`.
fn compile_from_source(
  guest: &str,
  guest_dir: &Path,
  wasm_target: &Path,
  release: bool,
  component_wasm: &Path,
) {
  println!(
    "cargo:rerun-if-changed={}",
    guest_dir.join("Cargo.toml").display()
  );
  println!("cargo:rerun-if-changed={}", guest_dir.join("src").display());
  println!("cargo:rerun-if-changed={}", guest_dir.join("wit").display());

  let profile = if release { "release" } else { "debug" };
  let guest_wasm = format!("{}.wasm", guest.replace('-', "_"));
  let core_wasm = wasm_target
    .join("wasm32-wasip2")
    .join(profile)
    .join(guest_wasm);

  let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
  let mut cmd = Command::new(&cargo);
  cmd.env_remove("RUSTFLAGS");
  cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
  for (key, _) in env::vars_os() {
    let Some(key) = key.to_str() else {
      continue;
    };
    if key.starts_with("CARGO_TARGET_") && key.ends_with("_RUSTFLAGS") {
      cmd.env_remove(key);
    }
  }
  cmd
    .args([
      "build",
      "--lib",
      "--target",
      "wasm32-wasip2",
      "-p",
      guest,
      "--target-dir",
    ])
    .arg(wasm_target);
  if release {
    cmd.arg("--release");
  }
  let status = cmd
    .status()
    .unwrap_or_else(|e| panic!("failed to run {cargo}: {e}"));
  assert!(
    status.success(),
    "cross-build of {guest} for wasm32-wasip2 failed \
     (is the 'wasm32-wasip2' target installed?)"
  );

  // The `wasm32-wasip2` target emits a component directly (component model
  // version = 0x0d), but verify the magic so that environments producing a bare
  // core module (component version gap) still work: if it is a core module
  // (version = 0x01), wrap it with `wasm-tools component new`.
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
      .arg(component_wasm)
      .status()
      .unwrap_or_else(|e| panic!("failed to run wasm-tools: {e}"));
    assert!(
      status.success(),
      "wasm-tools component new failed (is 'wasm-tools' on PATH?)"
    );
  } else {
    std::fs::copy(&core_wasm, component_wasm).unwrap_or_else(|e| {
      panic!("failed to copy component to {:?}: {e}", component_wasm)
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::tempdir;

  #[test]
  fn find_guest_scopes_to_the_calling_workspace() -> anyhow::Result<()> {
    let root = tempdir()?;
    let crate_dir = root.path().join("src/lib/omw-runtime-js");
    let guest_dir = root.path().join("src/wasm/omw-wasm-js-interpreter");
    std::fs::create_dir_all(&crate_dir)?;
    std::fs::create_dir_all(&guest_dir)?;
    assert_eq!(
      find_guest(&crate_dir, "omw-wasm-js-interpreter"),
      Some(guest_dir)
    );

    let packaged = root.path().join("target/package/omw-runtime-js-0.1.7");
    std::fs::create_dir_all(&packaged)?;
    assert_eq!(find_guest(&packaged, "omw-wasm-js-interpreter"), None);
    Ok(())
  }
}
