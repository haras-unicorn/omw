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
//!   cross-compile its bundled guest for `wasm32-wasip1` (a core module, because
//!   Binaryen's `wasm-opt` cannot read a component) with the crate's requested
//!   profile (release only), size-optimize the core with `wasm-opt` (the
//!   requested level in release, the cheaper `-O1` in debug), wrap it into a
//!   WASM component with
//!   `wasm-tools component new --adapt` (the preview1 adapter pointed at by
//!   `OMW_WASM_PREVIEW1_ADAPTER`), shrink it with `wasm-tools strip`, and expose
//!   the **portable** component to the crate via `cargo:rustc-env`. The
//!   component is deliberately left as portable wasm (never AOT-compiled to a
//!   `cwasm`): the runtime compiles it in-process at startup, so the embedding
//!   binary stays portable across machines and CPU feature sets.
//!
//! - [`build_brains`], called by the `omw-test compile-wasm` dev helper to
//!   cross-build single-file Rust brains against the in-repo `omw-wasm-rust`
//!   SDK.
//!
//! For `build_component`, the guest source lives outside the component package
//! (`<root>/src/wasm/*`), so a published crate cannot cross-build it. To make
//! the published crate self-contained, setting `OMW_WASM_BUILD_VENDORED` makes
//! this helper also brotli-compress the produced component into
//! `<package>/wasm/<guest>.component.wasm.br`. A packaged crate - a registry
//! checkout, or a copy extracted under `<workspace>/target/package` while
//! `cargo publish` verifies it - has no guest source under its own
//! `<workspace>/src/lib`, so this helper decompresses the vendored blob instead
//! of cross-compiling. Only the release prebuild sets that variable, so normal
//! development never writes into the package. The blob is compressed because
//! cargo gzips the crate tarball and gzip cannot shrink this data, so a stronger
//! codec is what fits `omw-runtime-python` under crates.io's crate limit.
//!
//! The nested `cargo` builds use a dedicated `--target-dir` so that they do not
//! contend for the global build lock held by the outer `cargo` invocation (which
//! would otherwise deadlock).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

mod brain;

pub use brain::build_brains;

/// The optimization level a guest crate asks for: it selects both the Cargo
/// profile the guest graph is compiled with (release only) and the Binaryen
/// `wasm-opt` level. Debug builds always use the cheap `-O1` so local iteration
/// stays fast while still exercising the `wasm-opt` round-trip.
#[derive(Clone, Copy, Debug)]
pub enum OptLevel {
  /// Minimize size (`-Oz` plus the `wasm-size` Cargo profile). The Python
  /// interpreter uses this so its vendored component fits under crates.io's
  /// 10 MiB crate limit.
  Size,
  /// Optimize for speed (`-O3` on the `wasm-speed` profile).
  Speed,
}

impl OptLevel {
  fn flag(self) -> &'static str {
    match self {
      Self::Size => "-Oz",
      Self::Speed => "-O3",
    }
  }

  /// The Cargo profile a guest's graph is compiled with in release. `Size`
  /// selects the workspace's `wasm-size` profile (LLVM `opt-level = "z"`),
  /// while `Speed` uses the `wasm-speed` profile.
  fn profile(self) -> &'static str {
    match self {
      Self::Size => "wasm-size",
      Self::Speed => "wasm-speed",
    }
  }
}

/// Builds the `guest` component for the calling crate's `build.rs`, exposing it
/// as a portable wasm component through `<PREFIX>_COMPONENT_WASM` (and its WAT
/// via `<PREFIX>_COMPONENT_WAT`), where `<PREFIX>` is the guest's upper-cased,
/// underscore-separated name. `opt_level` selects the release Binaryen level.
pub fn build_component(guest: &str, opt_level: OptLevel) {
  let env_prefix = guest.replace('-', "_").to_uppercase();
  let wasm_env = format!("{env_prefix}_COMPONENT_WASM");
  let wat_env = format!("{env_prefix}_COMPONENT_WAT");

  let manifest_dir = PathBuf::from(require_env("CARGO_MANIFEST_DIR"));
  let out_dir = PathBuf::from(require_env("OUT_DIR"));
  let release = env::var("PROFILE").is_ok_and(|profile| profile == "release");

  let component_wasm = out_dir.join(format!("{guest}.component.wasm"));
  let component_wat = out_dir.join(format!("{guest}.component.wat"));
  let vendored_dir = manifest_dir.join("wasm");
  // The vendored component ships brotli-compressed: cargo gzips the crate
  // tarball itself, and gzip cannot beat this data, so pre-compressing it with a
  // stronger codec is what keeps `omw-runtime-python` under crates.io's 10 MiB
  // crate cap. The raw component is restored from the blob on the publishing
  // side and on every consumer build.
  let vendored_component =
    vendored_dir.join(format!("{guest}.component.wasm.br"));
  let stale_component = vendored_dir.join(format!("{guest}.component.wasm"));
  let vendoring = env::var_os("OMW_WASM_BUILD_VENDORED").is_some();

  match find_guest(&manifest_dir, guest) {
    Some(guest_dir) => {
      compile_from_source(
        guest,
        &guest_dir,
        &out_dir.join("wasm-target"),
        release,
        opt_level,
        &component_wasm,
      );
      shrink_component(&component_wasm);
      if vendoring {
        std::fs::create_dir_all(&vendored_dir).unwrap_or_else(|e| {
          panic!("failed to create {}: {e}", vendored_dir.display())
        });
        vendor_component(
          &component_wasm,
          &vendored_component,
          &stale_component,
        );
      }
    }
    None => restore_vendored_component(&vendored_component, &component_wasm),
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

/// Shrinks a freshly built component in place with `wasm-tools strip`, dropping
/// custom sections (`producers`, `target_features`, DWARF, …) that are not
/// needed to execute it. `wasm-tools` is already required to build guests, and
/// the stripped component is what gets vendored and embedded.
fn shrink_component(component: &Path) {
  let stripped = component.with_extension("stripped.wasm");
  let status = Command::new("wasm-tools")
    .arg("strip")
    .arg(component)
    .arg("-o")
    .arg(&stripped)
    .status()
    .unwrap_or_else(|e| panic!("failed to run wasm-tools: {e}"));
  assert!(
    status.success(),
    "wasm-tools strip failed on {}",
    component.display()
  );
  std::fs::rename(&stripped, component).unwrap_or_else(|e| {
    panic!(
      "failed to replace {} with stripped component: {e}",
      component.display()
    )
  });
}

/// Brotli-compresses a built component into its vendored `.wasm.br` blob.
///
/// Cargo gzips the crate tarball, and this data is saturated under gzip, so the
/// compressed size produced here is what counts against crates.io's crate limit;
/// quality 11 with the full window buys enough margin for the Python
/// interpreter.
fn compress_component(component: &Path, compressed: &Path) {
  let mut input = std::fs::File::open(component)
    .unwrap_or_else(|e| panic!("failed to read {}: {e}", component.display()));
  let mut output = std::fs::File::create(compressed).unwrap_or_else(|e| {
    panic!("failed to create {}: {e}", compressed.display())
  });
  let params = brotli::enc::BrotliEncoderParams {
    quality: 11,
    lgwin: 24,
    ..Default::default()
  };
  brotli::BrotliCompress(&mut input, &mut output, &params).unwrap_or_else(
    |e| panic!("failed to compress {}: {e}", component.display()),
  );
}

/// Restores the raw component from its vendored brotli blob.
fn decompress_component(compressed: &Path, component: &Path) {
  let mut input = std::fs::File::open(compressed)
    .unwrap_or_else(|e| panic!("failed to read {}: {e}", compressed.display()));
  let mut output = std::fs::File::create(component).unwrap_or_else(|e| {
    panic!("failed to create {}: {e}", component.display())
  });
  brotli::BrotliDecompress(&mut input, &mut output).unwrap_or_else(|e| {
    panic!("failed to decompress {}: {e}", compressed.display())
  });
}

/// Vendors a freshly built component as its brotli blob, dropping any stale
/// uncompressed copy so it cannot be shipped and re-gzipped.
fn vendor_component(component: &Path, vendored: &Path, stale: &Path) {
  compress_component(component, vendored);
  if stale.is_file() {
    std::fs::remove_file(stale).unwrap_or_else(|e| {
      panic!("failed to remove stale {}: {e}", stale.display())
    });
  }
}

/// Restores a packaged crate's component from its vendored brotli blob.
fn restore_vendored_component(vendored: &Path, component_wasm: &Path) {
  assert!(
    vendored.is_file(),
    "guest sources are absent (published crate) and the vendored component is \
     missing at {}; set OMW_WASM_BUILD_VENDORED=1 during the release prebuild",
    vendored.display()
  );
  println!("cargo:rerun-if-changed={}", vendored.display());
  decompress_component(vendored, component_wasm);
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
///
/// The guest is compiled for `wasm32-wasip1`, with the `OptLevel`'s release
/// profile, so the result is a **core module** that Binaryen's `wasm-opt` can
/// size-optimize; it is then wrapped back into a component with the preview1
/// adapter.
fn compile_from_source(
  guest: &str,
  guest_dir: &Path,
  wasm_target: &Path,
  release: bool,
  opt_level: OptLevel,
  component_wasm: &Path,
) {
  println!(
    "cargo:rerun-if-changed={}",
    guest_dir.join("Cargo.toml").display()
  );
  println!("cargo:rerun-if-changed={}", guest_dir.join("src").display());
  println!("cargo:rerun-if-changed={}", guest_dir.join("wit").display());

  let profile = if release {
    opt_level.profile()
  } else {
    "debug"
  };
  let guest_wasm = format!("{}.wasm", guest.replace('-', "_"));
  let core_wasm = wasm_target
    .join("wasm32-wasip1")
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
      "wasm32-wasip1",
      "-p",
      guest,
      "--target-dir",
    ])
    .arg(wasm_target);
  if release {
    cmd.args(["--profile", opt_level.profile()]);
  }
  let status = cmd
    .status()
    .unwrap_or_else(|e| panic!("failed to run {cargo}: {e}"));
  assert!(
    status.success(),
    "cross-build of {guest} for wasm32-wasip1 failed \
     (is the 'wasm32-wasip1' target installed?)"
  );

  optimize_core(&core_wasm, release, opt_level);
  componentize(&core_wasm, component_wasm);
}

/// Size-optimizes a guest's core module in place with Binaryen's `wasm-opt`.
///
/// Guests are built for `wasm32-wasip1` precisely so this can run: Binaryen
/// cannot read a component, so the module is optimized before it is wrapped.
/// Release builds use the level the crate requested (`-Oz` for the size-focused
/// Python interpreter, `-O3` otherwise); debug builds use the much cheaper
/// `-O1`, which still round-trips the module (so the pipeline is exercised on
/// every build) without the cost of the full pipeline. Opaque custom sections
/// (notably the `component-type:*` metadata) are preserved so the later
/// `wasm-tools component new` still sees the WIT world. `wasm-opt` is provided
/// by the development shell.
fn optimize_core(core_wasm: &Path, release: bool, opt_level: OptLevel) {
  let level = if release { opt_level.flag() } else { "-O1" };
  let optimized = core_wasm.with_extension("opt.wasm");
  let mut cmd = Command::new("wasm-opt");
  cmd
    .args(["-all", level, "--strip-debug"])
    .arg(core_wasm)
    .arg("-o")
    .arg(&optimized);
  // Binaryen sizes its pass thread pool from `BINARYEN_CORES` (it has no
  // `--num-threads` flag). Leave a caller-provided value alone; otherwise use
  // Cargo's own job count so `-j`/`CARGO_BUILD_JOBS` is respected.
  if env::var_os("BINARYEN_CORES").is_none()
    && let Some(cores) = wasm_opt_cores()
  {
    cmd.env("BINARYEN_CORES", cores);
  }
  let status = cmd
    .status()
    .unwrap_or_else(|e| panic!("failed to run wasm-opt: {e}"));
  assert!(
    status.success(),
    "wasm-opt failed on {} (is 'wasm-opt' on PATH?)",
    core_wasm.display()
  );
  std::fs::rename(&optimized, core_wasm).unwrap_or_else(|e| {
    panic!(
      "failed to replace {} with its optimized module: {e}",
      core_wasm.display()
    )
  });
}

/// The thread count to hand Binaryen: Cargo's `NUM_JOBS` (which reflects `-j` /
/// `CARGO_BUILD_JOBS`), falling back to the machine's parallelism.
fn wasm_opt_cores() -> Option<String> {
  if let Ok(jobs) = env::var("NUM_JOBS")
    && jobs.parse::<usize>().is_ok_and(|jobs| jobs > 0)
  {
    return Some(jobs);
  }
  std::thread::available_parallelism()
    .ok()
    .map(|parallelism| parallelism.get().to_string())
}

/// Wraps a guest's core module into the `omw` component, adapting its
/// `wasi_snapshot_preview1` imports through the preview1 adapter the development
/// shell points at via `OMW_WASM_PREVIEW1_ADAPTER`.
fn componentize(core_wasm: &Path, component_wasm: &Path) {
  let adapter = env::var("OMW_WASM_PREVIEW1_ADAPTER").unwrap_or_else(|_| {
    panic!(
      "OMW_WASM_PREVIEW1_ADAPTER must point at \
       wasi_snapshot_preview1.reactor.wasm; the development shell sets it"
    )
  });
  let status = Command::new("wasm-tools")
    .args(["component", "new"])
    .arg(core_wasm)
    .arg("--adapt")
    .arg(format!("wasi_snapshot_preview1={adapter}"))
    .args(["-o"])
    .arg(component_wasm)
    .status()
    .unwrap_or_else(|e| panic!("failed to run wasm-tools: {e}"));
  assert!(
    status.success(),
    "wasm-tools component new failed (is 'wasm-tools' on PATH?)"
  );
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

  /// The exact bytes survive the `compress_component`/`decompress_component`
  /// round-trip.
  #[test]
  fn compression_round_trips() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let raw = dir.path().join("blob.bin");
    let br = dir.path().join("blob.bin.br");
    let restored = dir.path().join("restored.bin");
    let data = b"\0asm\x01\0\0\0 the quick brown fox".repeat(128);
    std::fs::write(&raw, &data)?;

    compress_component(&raw, &br);
    assert!(br.is_file());
    decompress_component(&br, &restored);

    assert_eq!(std::fs::read(&restored)?, data);
    Ok(())
  }

  /// Vendoring writes the brotli blob, drops a stale uncompressed copy, and the
  /// blob restores the freshly built bytes.
  #[test]
  fn vendor_writes_blob_and_drops_stale_raw() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let built = dir.path().join("built.wasm");
    let vendored_dir = dir.path().join("wasm");
    std::fs::create_dir_all(&vendored_dir)?;
    let vendored = vendored_dir.join("omw-wasm-mock.component.wasm.br");
    let stale = vendored_dir.join("omw-wasm-mock.component.wasm");
    std::fs::write(&built, b"freshly built component")?;
    std::fs::write(&stale, b"stale uncompressed component")?;

    vendor_component(&built, &vendored, &stale);

    assert!(vendored.is_file());
    assert!(!stale.exists());

    let restored = dir.path().join("restored.wasm");
    restore_vendored_component(&vendored, &restored);
    assert_eq!(std::fs::read(&restored)?, b"freshly built component");
    Ok(())
  }

  /// A packaged crate without a vendored blob fails loudly.
  #[test]
  #[should_panic(expected = "vendored component is missing")]
  fn restore_requires_the_vendored_blob() {
    let dir = tempdir().unwrap_or_else(|e| panic!("{e}"));
    let missing = dir.path().join("omw-wasm-mock.component.wasm.br");
    restore_vendored_component(&missing, &dir.path().join("out.wasm"));
  }
}
