//! Cross-build one or more rust brains for `wasm32-wasip2` and write each
//! ready-to-load component next to its source as `<stem>.wasm`. Mirrors the
//! `omw` `build.rs`: a throwaway crate is scaffolded in the system temp dir
//! around each single `brain.rs` (with the in-repo `omw-wasm-rust` SDK), built
//! with a dedicated `--target-dir` (the outer `cargo` run may still hold the
//! workspace build lock), then wrapped with `wasm-tools component new` when
//! the output is a bare core module rather than a component already.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::cli::CompileWasmArgs;

/// The in-repo SDK, relative to this crate's manifest dir.
const SDK: &str = "../../lib/omw-wasm-rust";

/// Compile every rust brain under `args.path` and return the written
/// `<stem>.wasm` paths.
pub fn build(args: &CompileWasmArgs) -> Result<Vec<PathBuf>> {
  let sources = collect_sources(&args.path)?;
  let sdk = args.sdk.clone().unwrap_or_else(default_sdk);
  let mut outputs = Vec::with_capacity(sources.len());
  for source in sources {
    outputs.push(build_one(&source, &sdk, args)?);
  }
  Ok(outputs)
}

fn default_sdk() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SDK)
}

fn collect_sources(path: &Path) -> Result<Vec<PathBuf>> {
  if path.is_file() {
    return Ok(vec![path.to_path_buf()]);
  }
  if !path.is_dir() {
    anyhow::bail!("path {} does not exist", path.display());
  }
  let mut sources = Vec::new();
  walk(path, &mut sources)?;
  sources.sort();
  Ok(sources)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
  let entries = std::fs::read_dir(dir)
    .with_context(|| format!("failed to read {}", dir.display()))?;
  for entry in entries {
    let entry = entry?;
    let path = entry.path();
    if entry.file_type()?.is_dir() {
      if !is_hidden(&path) {
        walk(&path, out)?;
      }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
      out.push(path);
    }
  }
  Ok(())
}

fn is_hidden(path: &Path) -> bool {
  path
    .file_name()
    .and_then(|name| name.to_str())
    .is_some_and(|name| name.starts_with('.'))
}

fn build_one(
  source: &Path,
  sdk: &Path,
  args: &CompileWasmArgs,
) -> Result<PathBuf> {
  let crate_dir = crate_root(source);
  let _ = std::fs::remove_dir_all(&crate_dir);
  std::fs::create_dir_all(crate_dir.join("src"))
    .with_context(|| format!("failed to create {}", crate_dir.display()))?;
  std::fs::copy(source, crate_dir.join("src/lib.rs"))
    .with_context(|| format!("failed to stage {}", source.display()))?;
  let manifest = format!(
    r#"
      [package]
      name = "{}"
      version = "0.0.0"
      edition = "2024"

      [lib]
      crate-type = ["cdylib"]
      [dependencies]
      omw-wasm-rust = {{ path = "{}" }}
    "#,
    crate_name(source),
    sdk.display()
  );
  std::fs::write(crate_dir.join("Cargo.toml"), manifest)
    .with_context(|| format!("failed to write {}", crate_dir.display()))?;

  let target = crate_dir.join("target");
  let cargo = args.cargo.clone().unwrap_or_else(|| "cargo".to_owned());
  let mut cmd = std::process::Command::new(&cargo);
  cmd.env_remove("RUSTFLAGS");
  cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
  for (key, _) in std::env::vars_os() {
    let Some(key) = key.to_str() else {
      continue;
    };
    if key.starts_with("CARGO_TARGET_") && key.ends_with("_RUSTFLAGS") {
      cmd.env_remove(key);
    }
  }
  let output = cmd
    .args(["build", "--target", "wasm32-wasip2"])
    .arg("--manifest-path")
    .arg(crate_dir.join("Cargo.toml"))
    .arg("--target-dir")
    .arg(&target)
    .args(&args.args)
    .output()
    .with_context(|| format!("failed to run {cargo}"))?;
  if !output.status.success() {
    anyhow::bail!(
      "cross-build of {} for wasm32-wasip2 failed: {}",
      source.display(),
      String::from_utf8_lossy(&output.stderr)
    );
  }

  let profile = target.join("wasm32-wasip2").join("debug");
  let module = find_wasm(&profile).with_context(|| {
    format!("cross-build of {} produced no .wasm", source.display())
  })?;
  let component = wrap(&module, args.wasm_tools.as_deref())?;
  let destination = source.with_extension("wasm");
  std::fs::copy(&component, &destination)
    .with_context(|| format!("failed to write {}", destination.display()))?;
  Ok(destination)
}

fn find_wasm(profile: &Path) -> Result<PathBuf> {
  std::fs::read_dir(profile)
    .with_context(|| format!("failed to list {}", profile.display()))?
    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
    .find(|path| path.extension().is_some_and(|ext| ext == "wasm"))
    .ok_or_else(|| anyhow::anyhow!("no .wasm in {}", profile.display()))
}

fn wrap(module: &Path, wasm_tools: Option<&str>) -> Result<PathBuf> {
  let component = module.with_extension("component.wasm");

  let header = std::fs::read(module)
    .with_context(|| format!("failed to read {}", module.display()))?;
  let is_module = header.len() >= 8
    && header[..4] == [0, b'a', b's', b'm']
    && header[4..8] == [1, 0, 0, 0];
  if !is_module {
    std::fs::copy(module, &component)
      .with_context(|| format!("failed to copy {}", component.display()))?;
    return Ok(component);
  }

  let wasm_tools = wasm_tools.unwrap_or("wasm-tools");
  let output = std::process::Command::new(wasm_tools)
    .args(["component", "new"])
    .arg(module)
    .args(["-o"])
    .arg(&component)
    .output()
    .context("failed to run wasm-tools")?;
  if !output.status.success() {
    anyhow::bail!(
      "wasm-tools component new failed for {}: {}",
      module.display(),
      String::from_utf8_lossy(&output.stderr)
    );
  }
  Ok(component)
}

fn crate_root(source: &Path) -> PathBuf {
  std::env::temp_dir().join("omw-test").join(slug(source))
}

fn crate_name(source: &Path) -> String {
  let stem = source
    .file_stem()
    .and_then(|stem| stem.to_str())
    .unwrap_or("brain");
  let mut name = String::from("omw-brain-");
  for character in stem.chars() {
    if character.is_ascii_alphanumeric() {
      name.push(character);
    } else {
      name.push('-');
    }
  }
  name
}

/// A stable, filesystem-safe directory name derived from the source path:
/// keep alphanumerics, turn everything else into `-`, and trim the edges so an
/// absolute path can never escape the temp base via `Path::join`.
fn slug(source: &Path) -> String {
  let slug: String = source
    .to_string_lossy()
    .chars()
    .map(|character| {
      if character.is_ascii_alphanumeric() {
        character
      } else {
        '-'
      }
    })
    .collect();
  slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::tempdir;

  #[test]
  fn crate_name_is_sanitized() {
    assert_eq!(crate_name(Path::new("brain.rs")), "omw-brain-brain");
    assert_eq!(
      crate_name(Path::new("examples/01-hello/wasm/brain.rs")),
      "omw-brain-brain"
    );
    assert_eq!(
      crate_name(Path::new("my-brain.v2.rs")),
      "omw-brain-my-brain-v2"
    );
  }

  #[test]
  fn slug_never_escapes_the_temp_base() {
    let slug = slug(Path::new("/abs/path/brain.rs"));
    assert!(!slug.contains('/'));
    assert!(!slug.starts_with('-'));
    assert!(!slug.ends_with('-'));
  }

  #[test]
  fn collect_sources_accepts_a_single_file() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let file = dir.path().join("brain.rs");
    std::fs::write(&file, "")?;
    assert_eq!(collect_sources(&file)?, vec![file]);
    Ok(())
  }

  #[test]
  fn collect_sources_recurses_a_directory_in_sorted_order() -> anyhow::Result<()>
  {
    let dir = tempdir()?;
    std::fs::create_dir_all(dir.path().join("b"))?;
    std::fs::create_dir_all(dir.path().join(".hidden"))?;
    std::fs::write(dir.path().join("b/brain.rs"), "")?;
    std::fs::write(dir.path().join("a.rs"), "")?;
    std::fs::write(dir.path().join(".hidden/c.rs"), "")?;
    std::fs::write(dir.path().join("notes.txt"), "")?;
    assert_eq!(
      collect_sources(dir.path())?,
      vec![dir.path().join("a.rs"), dir.path().join("b/brain.rs")]
    );
    Ok(())
  }

  #[test]
  fn collect_sources_errors_on_a_missing_path() {
    let error = collect_sources(Path::new("does-not-exist"))
      .expect_err("should error on a missing path");
    assert!(error.to_string().contains("does not exist"));
  }
}
