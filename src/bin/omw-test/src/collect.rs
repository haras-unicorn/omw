//! Recursive `omw.test.toml` discovery, root-relative include/exclude
//! filtering, and config loading for `omw-test run`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use glob::Pattern;
use omw::config::{Config, Format};
use omw::testing::Assertions;
use serde_json::Value;

/// A config file is collected when its stem is `omw.test` or ends with
/// `.omw.test`, and its extension is one of the supported formats, so several
/// test configs can live side by side in one directory. A shared base config
/// (`omw.test.base.<ext>`) never matches.
const CONFIG_STEM: &str = "omw.test";
const CONFIG_SUFFIX: &str = ".omw.test";
const CONFIG_EXTENSIONS: [&str; 4] = ["toml", "yaml", "yml", "json"];
const BASE_STEM: &str = "omw.test.base";

/// Whether `name` names a discoverable test config.
fn is_test_config(name: &str) -> bool {
  let Some((stem, extension)) = name.rsplit_once('.') else {
    return false;
  };
  if !CONFIG_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()) {
    return false;
  }
  stem == CONFIG_STEM || stem.ends_with(CONFIG_SUFFIX)
}

/// The base configs that apply to `config`, outermost ancestor first: for each
/// directory from the discovery `root` down to the config's own directory, the
/// `omw.test.base.<ext>` file it contains (if any). A base only reaches down
/// the tree, never above the path passed to `discover`.
fn base_configs(config: &Path, root: &Path) -> Vec<PathBuf> {
  let mut dirs = Vec::new();
  let mut current = config.parent();
  while let Some(dir) = current {
    let normalized = if dir.as_os_str().is_empty() {
      PathBuf::from(".")
    } else {
      dir.to_path_buf()
    };
    dirs.push(normalized.clone());
    if normalized == root {
      break;
    }
    current = dir.parent().filter(|parent| !parent.as_os_str().is_empty());
  }
  dirs.reverse();
  dirs.iter().filter_map(|dir| base_in(dir)).collect()
}

/// The base config in `dir`, trying each supported extension in order.
fn base_in(dir: &Path) -> Option<PathBuf> {
  CONFIG_EXTENSIONS
    .iter()
    .map(|extension| dir.join(format!("{BASE_STEM}.{extension}")))
    .find(|path| path.is_file())
}

/// One discovered test: its config file and its path relative to the discovery
/// root (used as the label and by `--include` / `--exclude`), plus the shared
/// base configs that apply to it (outermost ancestor first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Test {
  pub config: PathBuf,
  pub relative: String,
  pub bases: Vec<PathBuf>,
}

impl Test {
  /// The label printed by `run`: the root-relative path of the config file.
  pub fn label(&self) -> &str {
    &self.relative
  }
}

/// Discover tests under `path`: a file is a single test, a directory is a
/// recursive walk for `omw.test.toml` / `*.omw.test.toml` (hidden directories
/// are skipped). Each test carries the `omw.test.base.<ext>` configs in its
/// directory and its ancestors up to `path` (outermost first).
pub fn discover(path: &Path) -> Result<Vec<Test>> {
  if !crate::stdio::is_stdin(path) && !path.exists() {
    anyhow::bail!("path {} does not exist", path.display());
  }
  let base = root(path);
  let mut configs = Vec::new();
  if path.is_dir() {
    walk(path, &mut configs)?;
  } else {
    configs.push(path.to_path_buf());
  }
  configs.sort();
  Ok(
    configs
      .into_iter()
      .map(|config| Test {
        relative: relative_path(&config, base),
        bases: base_configs(&config, base),
        config,
      })
      .collect(),
  )
}

/// The discovery root: the directory itself, or a file's parent.
pub fn root(path: &Path) -> &Path {
  if path.is_dir() {
    path
  } else {
    path
      .parent()
      .filter(|parent| !parent.as_os_str().is_empty())
      .unwrap_or(Path::new("."))
  }
}

/// Keep tests whose root-relative path matches an `--include` glob (or any
/// include when none are given) and no `--exclude` glob.
pub fn filter(
  tests: Vec<Test>,
  include: &[String],
  exclude: &[String],
) -> Result<Vec<Test>> {
  let include = compile(include)?;
  let exclude = compile(exclude)?;
  Ok(
    tests
      .into_iter()
      .filter(|test| keep(&test.relative, &include, &exclude))
      .collect(),
  )
}

/// Read a test config and parse both the runnable [`Config`] (with the
/// `OMW_TEST__` environment overlay) and its `[assertions]` section.
///
/// `bases` are ordinary `omw.toml`-shaped files layered *under* the test config
/// in the order given (outermost ancestor first): later sources override earlier
/// ones, so the test config overrides the base configs it inherits and the
/// `OMW_TEST__` environment overrides them all. Merging is a deep table merge —
/// a test config's `[agents.<name>]` or `[assertions.<agent>]` overrides the
/// base config's keys for that entry while inheriting the rest. A base config
/// contributes both the `Config` and `[assertions]`.
///
/// Each file's format is `format` when given, else inferred from its extension.
/// A relative `script` in a base config resolves against that base config's
/// directory, so it always points where it was written.
pub fn load(
  path: &Path,
  format: Option<Format>,
  bases: &[PathBuf],
  registries: &omw::agent::Registries,
) -> Result<(Config, Assertions)> {
  let merged = merged_value(path, format, bases)?;
  let overlay = registries.env_overlay("OMW_TEST");
  let source: ::config::Config = ::config::Config::builder()
    .add_source(
      ::config::File::from_str(
        &serde_json::to_string(&merged)?,
        ::config::FileFormat::Json,
      )
      .required(false),
    )
    .add_source(
      ::config::File::from_str(
        &serde_json::to_string(&overlay)
          .context("failed to encode the environment overlay")?,
        ::config::FileFormat::Json,
      )
      .required(false),
    )
    .build()
    .context("failed to build configuration")?;
  let config: Config = source
    .try_deserialize()
    .context("failed to deserialize configuration")?;
  tracing::info!(
    path = %path.display(),
    format = %resolve_format(path, format)?,
    providers = config.providers.len(),
    tooling = config.tooling.len(),
    runtime = config.runtime.len(),
    agents = config.agents.len(),
    "configuration loaded"
  );
  tracing::debug!(config = ?config, "configuration details");
  let assertions: Assertions = serde_json::from_value(merged)
    .context("failed to deserialize [assertions]")?;
  Ok((config, assertions))
}

/// Merge the base configs and the test config into one JSON value, without the
/// `OMW_TEST__` environment overlay. Each base's relative `script` resolves
/// against that base's directory before it is merged, so a base always points
/// where it was written.
fn merged_value(
  path: &Path,
  format: Option<Format>,
  bases: &[PathBuf],
) -> Result<Value> {
  let mut merged = Value::Object(serde_json::Map::new());
  for base_path in bases {
    let raw = crate::stdio::read_to_string(base_path)?;
    let base_format = resolve_format(base_path, format)?;
    let mut value = parse_to_value(&raw, base_format).with_context(|| {
      format!("failed to parse base config {}", base_path.display())
    })?;
    resolve_source_scripts(&mut value, base_path)?;
    deep_merge(&mut merged, value);
  }
  let raw = crate::stdio::read_to_string(path)?;
  let format = resolve_format(path, format)?;
  let value = parse_to_value(&raw, format)
    .with_context(|| format!("failed to parse {}", path.display()))?;
  deep_merge(&mut merged, value);
  Ok(merged)
}

/// The paths whose change should re-run `test`: its own config file, each base
/// config, and every brain script its agents resolve to. A base's script is
/// already absolute by the time it is merged; the test config's relative
/// scripts resolve against the test config's directory.
///
/// Best-effort: a config that fails to parse still contributes its config and
/// bases, so an in-progress broken edit still re-runs the test (and reports the
/// parse error).
pub fn watched_paths(test: &Test, format: Option<Format>) -> Vec<PathBuf> {
  let mut paths = vec![canonical_path(&test.config)];
  for base in &test.bases {
    paths.push(canonical_path(base));
  }
  if let Ok(value) = merged_value(&test.config, format, &test.bases) {
    for script in agent_scripts(&value, &test.config) {
      paths.push(canonical_path(&script));
    }
  }
  paths.sort();
  paths.dedup();
  paths
}

/// Every agent's `script` in a merged config value, with a relative path
/// resolved against the test config's directory (base contributions are
/// already absolute).
fn agent_scripts(value: &Value, config: &Path) -> Vec<PathBuf> {
  let dir = config
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
    .unwrap_or(Path::new("."));
  let Some(agents) = value.get("agents").and_then(Value::as_object) else {
    return Vec::new();
  };
  agents
    .values()
    .filter_map(|agent| {
      let script = agent.get("script").and_then(Value::as_str)?;
      let path = Path::new(script);
      Some(if path.is_relative() {
        dir.join(path)
      } else {
        path.to_path_buf()
      })
    })
    .collect()
}

/// Canonical form of `path`, falling back to an absolute path when the file
/// does not exist (a not-yet-created or deleted target).
pub fn canonical_path(path: &Path) -> PathBuf {
  path.canonicalize().unwrap_or_else(|_| {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
  })
}

/// Resolve a config value's relative agent `script` paths against `source`'s
/// directory, so each file's scripts are relative to where the file lives. The
/// resolved path is made absolute, so the later [`resolve_scripts`] pass (which
/// joins the test config's directory) leaves it untouched instead of joining it
/// a second time.
fn resolve_source_scripts(value: &mut Value, source: &Path) -> Result<()> {
  let dir = source
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
    .unwrap_or(Path::new("."));
  let Some(agents) = value.get_mut("agents").and_then(Value::as_object_mut)
  else {
    return Ok(());
  };
  for agent in agents.values_mut() {
    let Some(script) = agent.get("script").and_then(Value::as_str) else {
      continue;
    };
    if Path::new(script).is_relative() {
      let resolved = std::path::absolute(dir.join(script))
        .with_context(|| format!("failed to resolve the script {script:?}"))?
        .to_string_lossy()
        .into_owned();
      if let Some(object) = agent.as_object_mut() {
        object.insert("script".to_owned(), Value::String(resolved));
      }
    }
  }
  Ok(())
}

/// The format for a config path: `format` when given, else inferred from the
/// path's extension, erroring when neither is available.
fn resolve_format(path: &Path, format: Option<Format>) -> Result<Format> {
  format.or_else(|| Format::from_path(path)).with_context(|| {
    format!(
      "cannot infer the config format of {}; pass --format",
      path.display()
    )
  })
}

/// Parse `source` into a JSON value, so sources of any format can be merged.
fn parse_to_value(source: &str, format: Format) -> Result<Value> {
  format.parse(source)
}

/// Deep-merge `overlay` into `base`: objects merge recursively (an overlay key
/// that is absent from `base` is inserted; a key present in both is merged or
/// replaced), while arrays and scalars replace wholesale.
fn deep_merge(base: &mut Value, overlay: Value) {
  match (base, overlay) {
    (Value::Object(base), Value::Object(overlay)) => {
      for (key, value) in overlay {
        match base.get_mut(&key) {
          Some(existing) => deep_merge(existing, value),
          None => {
            base.insert(key, value);
          }
        }
      }
    }
    (base, overlay) => *base = overlay,
  }
}

/// The process-wide testing tunables, read from the `OMW_TEST__` environment
/// overlay (default when unset). Used by the `--watch` debounce, which is not
/// tied to any single test config.
pub fn env_tunables(
  registries: &omw::agent::Registries,
) -> Result<omw::config::Tunables> {
  let overlay = registries.env_overlay("OMW_TEST");
  let source = ::config::Config::builder()
    .add_source(
      ::config::File::from_str(
        &serde_json::to_string(&overlay)
          .context("failed to encode the environment overlay")?,
        ::config::FileFormat::Json,
      )
      .required(false),
    )
    .build()
    .context("failed to build the testing tunables")?;
  match source.get::<omw::config::Tunables>("tunables") {
    Ok(tunables) => Ok(tunables),
    Err(::config::ConfigError::NotFound(_)) => {
      Ok(omw::config::Tunables::default())
    }
    Err(error) => {
      Err(error).context("failed to deserialize the testing tunables")
    }
  }
}

/// Resolve each agent's relative `script` against the config's directory, so a
/// config can point at the brain next to it regardless of the process CWD.
/// Absolute script paths are left untouched. A config read from a standard
/// stream has no meaningful directory, so its scripts resolve against the CWD.
pub fn resolve_scripts(config: &mut Config, path: &Path) {
  let base = if crate::stdio::is_stdin(path) {
    Path::new(".")
  } else {
    path
      .parent()
      .filter(|parent| !parent.as_os_str().is_empty())
      .unwrap_or(Path::new("."))
  };
  for agent in config.agents.values_mut() {
    let script = Path::new(&agent.script);
    if script.is_relative() {
      agent.script = base.join(script).to_string_lossy().into_owned();
    }
  }
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
    } else if path
      .file_name()
      .and_then(|name| name.to_str())
      .is_some_and(is_test_config)
    {
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

fn relative_path(config: &Path, root: &Path) -> String {
  match config.strip_prefix(root) {
    Ok(rel) if rel.as_os_str().is_empty() => ".".to_owned(),
    Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
    Err(_) => config
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
      .unwrap_or_else(|| ".".to_owned()),
  }
}

fn compile(patterns: &[String]) -> Result<Vec<Pattern>> {
  patterns
    .iter()
    .map(|pattern| {
      Pattern::new(pattern).with_context(|| format!("invalid glob {pattern:?}"))
    })
    .collect()
}

fn keep(rel: &str, include: &[Pattern], exclude: &[Pattern]) -> bool {
  let included = include.is_empty() || include.iter().any(|p| p.matches(rel));
  let excluded = exclude.iter().any(|p| p.matches(rel));
  included && !excluded
}

#[cfg(test)]
mod tests {
  use super::*;
  use serial_test::serial;
  use std::collections::HashMap;
  use tempfile::tempdir;

  /// Load with the built-in registries; tests do not register custom back ends.
  fn load(
    path: &Path,
    format: Option<Format>,
    bases: &[PathBuf],
  ) -> anyhow::Result<(Config, Assertions)> {
    super::load(path, format, bases, &omw::agent::Registries::default())
  }

  /// Testing tunables with the built-in registries.
  fn env_tunables() -> anyhow::Result<omw::config::Tunables> {
    super::env_tunables(&omw::agent::Registries::default())
  }

  fn write(dir: &Path, rel: &str, contents: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, contents)?;
    Ok(path)
  }

  fn relative_paths(tests: &[Test]) -> Vec<&str> {
    tests.iter().map(|test| test.relative.as_str()).collect()
  }

  #[test]
  fn discover_walks_a_directory_tree_in_sorted_order() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), "02-tool-agent/rhai/omw.test.toml", "")?;
    write(dir.path(), "01-hello/wasm/omw.test.toml", "")?;
    write(dir.path(), "01-hello/rhai/omw.test.toml", "")?;
    // A base config and a differently-named config are never collected.
    write(dir.path(), "01-hello/omw.test.base.toml", "")?;
    write(dir.path(), "01-hello/omw.toml", "")?;

    let tests = discover(dir.path())?;
    assert_eq!(
      relative_paths(&tests),
      vec![
        "01-hello/rhai/omw.test.toml",
        "01-hello/wasm/omw.test.toml",
        "02-tool-agent/rhai/omw.test.toml",
      ]
    );
    Ok(())
  }

  #[test]
  fn discover_collects_yaml_and_json_configs() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), "a/omw.test.yaml", "")?;
    write(dir.path(), "b/omw.test.yml", "")?;
    write(dir.path(), "c/omw.test.json", "")?;
    write(dir.path(), "d/first.omw.test.json", "")?;
    // A base config in a non-TOML format is still never collected.
    write(dir.path(), "e/omw.test.base.yaml", "")?;
    write(dir.path(), "f/omw.yaml", "")?;

    let tests = discover(dir.path())?;
    assert_eq!(
      relative_paths(&tests),
      vec![
        "a/omw.test.yaml",
        "b/omw.test.yml",
        "c/omw.test.json",
        "d/first.omw.test.json",
      ]
    );
    Ok(())
  }

  #[test]
  fn discover_matches_any_name_ending_in_the_suffix() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), "omw.test.toml", "")?;
    write(dir.path(), "first.omw.test.toml", "")?;
    write(dir.path(), "second.omw.test.toml", "")?;
    // Not a test config: it ends in `base.toml`.
    write(dir.path(), "case.omw.test.base.toml", "")?;

    let tests = discover(dir.path())?;
    assert_eq!(
      relative_paths(&tests),
      vec![
        "first.omw.test.toml",
        "omw.test.toml",
        "second.omw.test.toml"
      ]
    );
    Ok(())
  }

  #[test]
  fn discover_skips_hidden_directories() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), ".git/omw.test.toml", "")?;
    write(dir.path(), "case/omw.test.toml", "")?;
    let tests = discover(dir.path())?;
    assert_eq!(relative_paths(&tests), vec!["case/omw.test.toml"]);
    Ok(())
  }

  #[test]
  fn discover_treats_a_file_as_a_single_test() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let config = write(dir.path(), "nested/omw.test.toml", "")?;
    let tests = discover(&config)?;
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].config, config);
    assert_eq!(tests[0].relative, "omw.test.toml");
    Ok(())
  }

  #[test]
  fn discover_errors_on_a_missing_path() {
    let error = discover(Path::new("does-not-exist"))
      .expect_err("should error on a missing path");
    assert!(error.to_string().contains("does not exist"));
  }

  #[test]
  fn filter_applies_include_then_exclude() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), "01-hello/rhai/omw.test.toml", "")?;
    write(dir.path(), "01-hello/js/omw.test.toml", "")?;
    write(dir.path(), "01-hello/wasm/omw.test.toml", "")?;
    write(dir.path(), "02-tool-agent/rhai/omw.test.toml", "")?;
    let tests = discover(dir.path())?;

    let selected = filter(
      tests.clone(),
      &["**/rhai/**".to_owned()],
      &["01-hello/**".to_owned()],
    )?;
    assert_eq!(
      relative_paths(&selected),
      vec!["02-tool-agent/rhai/omw.test.toml"]
    );

    let selected = filter(tests, &[], &["**/wasm/**".to_owned()])?;
    assert_eq!(
      relative_paths(&selected),
      vec![
        "01-hello/js/omw.test.toml",
        "01-hello/rhai/omw.test.toml",
        "02-tool-agent/rhai/omw.test.toml",
      ]
    );
    Ok(())
  }

  #[test]
  fn filter_with_no_tests_is_empty() -> anyhow::Result<()> {
    let dir = tempdir()?;
    assert!(discover(dir.path())?.is_empty());
    Ok(())
  }

  #[test]
  fn filter_rejects_an_invalid_glob() {
    let error = filter(Vec::new(), &["[".to_owned()], &[])
      .expect_err("should reject an invalid glob");
    assert!(error.to_string().contains("invalid glob"));
  }

  /// Removes the named variables on drop, so a failing test never leaks
  /// process-global env into sibling tests.
  struct EnvSet {
    value: HashMap<String, String>,
  }

  impl EnvSet {
    fn new(value: HashMap<String, String>) -> Self {
      for (key, value) in &value {
        #[allow(unsafe_code, reason = "serial_test ensures this is serial")]
        {
          unsafe { std::env::set_var(key, value) };
        }
      }
      Self { value }
    }
  }

  impl Drop for EnvSet {
    #[allow(
      unsafe_code,
      reason = "std::env::remove_var is unsafe in edition 2024"
    )]
    fn drop(&mut self) {
      for (key, _) in &self.value {
        #[allow(unsafe_code, reason = "serial_test ensures this is serial")]
        {
          unsafe { std::env::remove_var(key) };
        }
      }
    }
  }

  /// Changes the process working directory and restores it on drop, so a test
  /// can exercise relative config paths without leaking the change.
  struct CwdGuard {
    original: PathBuf,
  }

  impl CwdGuard {
    fn new(dir: &Path) -> anyhow::Result<Self> {
      let original = std::env::current_dir()?;
      std::env::set_current_dir(dir)?;
      Ok(Self { original })
    }
  }

  impl Drop for CwdGuard {
    fn drop(&mut self) {
      let _ = std::env::set_current_dir(&self.original);
    }
  }

  #[test]
  #[serial(env)]
  fn config_deserializes_with_flattened_params() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      r#"
        [providers.openai]
        kind = "openai"
        base_url = "https://example.com/v1"
        api_key = "sk-test"
        model = "gpt-test"

        [tooling.mcp]
        kind = "mcp"
        transport = "stdio"
        command = "npx"
        args = ["-y", "@modelcontextprotocol/server-everything"]

        [runtime.rhai]
        kind = "rhai"

        [agents.alice]
        runtime = "rhai"
        script = "brain.rhai"
      "#,
    )?;

    let (cfg, _) = load(&path, None, &[])?;
    let provider = cfg
      .providers
      .get("openai")
      .ok_or_else(|| anyhow::anyhow!("missing openai provider"))?;
    assert_eq!(provider.kind, "openai");
    assert_eq!(provider.params["base_url"], "https://example.com/v1");
    assert_eq!(provider.params["api_key"], "sk-test");
    assert_eq!(provider.params["model"], "gpt-test");
    // `kind` is consumed by the named field, not duplicated in the params.
    assert!(provider.params.get("kind").is_none());

    let mcp = cfg
      .tooling
      .get("mcp")
      .ok_or_else(|| anyhow::anyhow!("missing mcp tooling"))?;
    assert_eq!(mcp.kind, "mcp");
    assert_eq!(mcp.params["command"], "npx");

    let rhai = cfg
      .runtime
      .get("rhai")
      .ok_or_else(|| anyhow::anyhow!("missing rhai runtime"))?;
    assert_eq!(rhai.kind, "rhai");

    assert_eq!(cfg.agents.len(), 1);
    assert_eq!(cfg.agents["alice"].runtime, "rhai");
    assert_eq!(cfg.agents["alice"].script, "brain.rhai");
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn endpoint_deserializes_with_kind_and_opaque_params() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      r#"
        [endpoint]
        kind = "openai"
        listen = "127.0.0.1:37532"
      "#,
    )?;
    let (cfg, _) = load(&path, None, &[])?;
    let endpoint = cfg
      .endpoint
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("missing endpoint"))?;
    assert_eq!(endpoint.kind, "openai");
    assert_eq!(endpoint.params["listen"], "127.0.0.1:37532");
    assert!(endpoint.params.get("kind").is_none());
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn yaml_config_parses_config_and_assertions() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.yaml",
      r#"
        runtime:
          rhai:
            kind: rhai
        agents:
          alice:
            runtime: rhai
            script: brain.rhai
        assertions:
          alice:
            outcome: completed
            events:
              - kind: call
                op: chat
      "#,
    )?;
    let (cfg, assertions) = load(&path, None, &[])?;
    assert_eq!(cfg.runtime["rhai"].kind, "rhai");
    assert_eq!(cfg.agents["alice"].script, "brain.rhai");
    assert_eq!(assertions.assertions["alice"].events.len(), 1);
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn json_config_parses_config_and_assertions() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.json",
      r#"{
        "runtime": { "rhai": { "kind": "rhai" } },
        "agents": { "alice": { "runtime": "rhai", "script": "brain.rhai" } },
        "assertions": { "alice": { "outcome": "completed", "events": [] } }
      }"#,
    )?;
    let (cfg, assertions) = load(&path, None, &[])?;
    assert_eq!(cfg.agents["alice"].runtime, "rhai");
    assert!(assertions.assertions.contains_key("alice"));
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn format_override_parses_an_extensionless_file() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(dir.path(), "omw.test.conf", "{\"agents\":{}}")?;
    let (cfg, _) = load(&path, Some(Format::Json), &[])?;
    assert!(cfg.agents.is_empty());
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn endpoint_defaults_to_none() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(dir.path(), "omw.test.toml", "")?;
    let (cfg, _) = load(&path, None, &[])?;
    assert!(cfg.endpoint.is_none());
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn unknown_kind_is_preserved_until_factory_time() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      r#"
        [providers.custom]
        kind = "custom-thing"
        foo = "bar"
      "#,
    )?;
    let (cfg, _) = load(&path, None, &[])?;
    let provider = cfg
      .providers
      .get("custom")
      .ok_or_else(|| anyhow::anyhow!("missing custom provider"))?;
    assert_eq!(provider.kind, "custom-thing");
    assert_eq!(provider.params["foo"], "bar");
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn tunables_default_and_override() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      "[tunables]\nrecv_timeout_secs = 30\n",
    )?;
    let (cfg, _) = load(&path, None, &[])?;
    assert_eq!(cfg.tunables.recv_timeout_secs, 30);
    assert_eq!(
      cfg.tunables,
      omw::config::Tunables {
        recv_timeout_secs: 30,
        ..omw::config::Tunables::default()
      }
    );
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn env_tunables_default_when_unset() -> anyhow::Result<()> {
    assert_eq!(
      env_tunables()?,
      omw::config::Tunables::default(),
      "no OMW_TEST__ variables means the defaults"
    );
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn env_tunables_overlays_the_watch_debounce() -> anyhow::Result<()> {
    let _guard = EnvSet::new(HashMap::from([(
      "OMW_TEST__TUNABLES__WATCH_DEBOUNCE_MS".to_owned(),
      "7".to_owned(),
    )]));
    let tunables = env_tunables()?;
    assert_eq!(tunables.watch_debounce_ms, 7);
    assert_eq!(
      tunables.watch_debounce(),
      std::time::Duration::from_millis(7)
    );
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn tooling_connect_backoff_tunables_default_and_override()
  -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      "[tunables]\ntooling_connect_backoff_start_ms = 50\ntooling_connect_backoff_cap_secs = 5\n",
    )?;
    let (cfg, _) = load(&path, None, &[])?;
    assert_eq!(cfg.tunables.tooling_connect_backoff_start_ms, 50);
    assert_eq!(cfg.tunables.tooling_connect_backoff_cap_secs, 5);
    assert_eq!(
      cfg.tunables.tooling_connect_backoff_start(),
      std::time::Duration::from_millis(50)
    );
    assert_eq!(
      cfg.tunables.tooling_connect_backoff_cap(),
      std::time::Duration::from_secs(5)
    );
    let defaults = omw::config::Tunables::default();
    assert_eq!(defaults.tooling_connect_backoff_start_ms, 100);
    assert_eq!(defaults.tooling_connect_backoff_cap_secs, 30);
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn resolve_scripts_joins_the_config_directory() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "case/rhai/omw.test.toml",
      r#"
        [runtime.rhai]
        kind = "rhai"

        [agents.alice]
        runtime = "rhai"
        script = "brain.rhai"
      "#,
    )?;
    let (mut config, _) = load(&path, None, &[])?;
    resolve_scripts(&mut config, &path);
    let expected = path
      .parent()
      .ok_or_else(|| anyhow::anyhow!("config has no parent"))?
      .join("brain.rhai");
    assert_eq!(config.agents["alice"].script, expected.to_string_lossy());
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn resolve_scripts_leaves_absolute_paths() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "case/rhai/omw.test.toml",
      r#"
        [runtime.rhai]
        kind = "rhai"

        [agents.alice]
        runtime = "rhai"
        script = "/absolute/brain.rhai"
      "#,
    )?;
    let (mut config, _) = load(&path, None, &[])?;
    resolve_scripts(&mut config, &path);
    assert_eq!(config.agents["alice"].script, "/absolute/brain.rhai");
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn env_overlay_merges_over_file() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      r#"
        [providers.openai]
        kind = "openai"
        api_key = "from-file"
      "#,
    )?;

    let mut vars = HashMap::new();
    vars.insert(
      "OMW_TEST__PROVIDERS__OPENAI__API_KEY".to_owned(),
      "from-env".to_owned(),
    );
    let _vars = EnvSet::new(vars);

    let (cfg, _) = load(&path, None, &[])?;
    let provider = cfg
      .providers
      .get("openai")
      .ok_or_else(|| anyhow::anyhow!("missing openai provider"))?;
    assert_eq!(provider.kind, "openai");
    assert_eq!(provider.params["api_key"], "from-env");
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn env_overlay_keeps_free_form_map_keys() -> anyhow::Result<()> {
    // Only rely on back ends `omw-test` enables: it builds without
    // `tooling-mcp`, so MCP's `env` map is not a registered opaque path here.
    // The mock tooling's `initial_resource_contents` is always available.
    let dir = tempdir()?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      r#"
        [tooling.mock]
        kind = "mock"
      "#,
    )?;

    let mut vars = HashMap::new();
    vars.insert(
      "OMW_TEST__TOOLING__MOCK__INITIAL_RESOURCE_CONTENTS__SomeURI".to_owned(),
      "content".to_owned(),
    );
    let _vars = EnvSet::new(vars);

    let (cfg, _) = load(&path, None, &[])?;
    let mock = cfg
      .tooling
      .get("mock")
      .ok_or_else(|| anyhow::anyhow!("missing mock tooling"))?;
    assert_eq!(
      mock.params["initial_resource_contents"]["SomeURI"],
      "content"
    );
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn base_config_layers_under_the_test_config() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let base = write(
      dir.path(),
      "omw.test.base.toml",
      r#"
        [providers.openai]
        kind = "mock"
        turns = [{ content = "hello" }]

        [agents.alice]
        runtime = "runtime"

        [assertions.alice]
        outcome = "completed"
        events = [{ kind = "call", op = "chat" }]
      "#,
    )?;
    let path = write(
      dir.path(),
      "case/rhai/omw.test.toml",
      r#"
        [runtime.runtime]
        kind = "rhai"

        [agents.alice]
        script = "brain.rhai"
      "#,
    )?;

    let (cfg, assertions) = load(&path, None, &[base])?;
    // The provider comes from the base config...
    assert_eq!(cfg.providers["openai"].kind, "mock");
    // ...while the runtime kind and the agent's script come from the test
    // config, and the agent inherits its `runtime` from the base config.
    assert_eq!(cfg.runtime["runtime"].kind, "rhai");
    assert_eq!(cfg.agents["alice"].runtime, "runtime");
    assert_eq!(cfg.agents["alice"].script, "brain.rhai");
    // Assertions are layered too.
    assert_eq!(assertions.assertions["alice"].events.len(), 1);
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn a_test_config_overrides_base_assertion_keys() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let base = write(
      dir.path(),
      "omw.test.base.toml",
      r#"
        [assertions.alice]
        outcome = "completed"
        events = [{ kind = "call", op = "chat" }]
      "#,
    )?;
    let path = write(
      dir.path(),
      "omw.test.toml",
      r#"
        [assertions.alice]
        outcome = "asserted"
        events = [{ kind = "call", op = "list_tools" }]
      "#,
    )?;
    let (_, assertions) = load(&path, None, &[base])?;
    let alice = &assertions.assertions["alice"];
    assert_eq!(
      alice.outcome,
      Some(omw::testing::OutcomeAssertion::Asserted)
    );
    assert_eq!(alice.events.len(), 1);
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn later_base_configs_override_earlier_ones() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let base = write(
      dir.path(),
      "base.toml",
      "[tunables]\nrecv_timeout_secs = 10\n[providers.openai]\nkind = \"mock\"\nturns = [{ content = \"base\" }]\n",
    )?;
    let overlay = write(
      dir.path(),
      "overlay.toml",
      "[providers.openai]\nturns = [{ content = \"overlay\" }]\n",
    )?;
    let path = write(dir.path(), "omw.test.toml", "")?;
    let (cfg, _) = load(&path, None, &[base, overlay])?;
    assert_eq!(cfg.tunables.recv_timeout_secs, 10);
    assert_eq!(
      cfg.providers["openai"].params["turns"],
      serde_json::json!([{ "content": "overlay" }])
    );
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn base_config_resolves_scripts_against_its_own_directory()
  -> anyhow::Result<()> {
    let dir = tempdir()?;
    let base = write(
      dir.path(),
      "shared/omw.test.base.toml",
      r#"
        [runtime.runtime]
        kind = "rhai"

        [agents.alice]
        runtime = "runtime"
        script = "brain.rhai"
      "#,
    )?;
    let path = write(dir.path(), "case/omw.test.toml", "")?;
    let (cfg, _) = load(&path, None, &[base])?;
    let expected = dir.path().join("shared/brain.rhai");
    assert_eq!(cfg.agents["alice"].script, expected.to_string_lossy());
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn resolve_scripts_does_not_double_join_base_config_scripts()
  -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(
      dir.path(),
      "shared/omw.test.base.toml",
      r#"
        [runtime.runtime]
        kind = "rhai"

        [agents.alice]
        runtime = "runtime"
        script = "dist/index.js"
      "#,
    )?;
    write(dir.path(), "shared/omw.test.toml", "")?;

    let _cwd = CwdGuard::new(dir.path())?;
    let (mut cfg, _) = load(
      Path::new("shared/omw.test.toml"),
      None,
      &[PathBuf::from("shared/omw.test.base.toml")],
    )?;
    resolve_scripts(&mut cfg, Path::new("shared/omw.test.toml"));

    let expected = std::path::absolute("shared/dist/index.js")?;
    assert_eq!(cfg.agents["alice"].script, expected.to_string_lossy());
    Ok(())
  }

  #[test]
  fn discover_attaches_ancestor_base_configs() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let outer = write(dir.path(), "omw.test.base.toml", "")?;
    let inner = write(dir.path(), "case/omw.test.base.toml", "")?;
    write(dir.path(), "case/rhai/omw.test.toml", "")?;

    let tests = discover(dir.path())?;
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].bases, vec![outer, inner]);
    Ok(())
  }

  #[test]
  fn discover_base_config_is_never_collected() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), "case/omw.test.base.toml", "")?;
    write(dir.path(), "case/omw.test.toml", "")?;
    let tests = discover(dir.path())?;
    assert_eq!(relative_paths(&tests), vec!["case/omw.test.toml"]);
    Ok(())
  }

  #[test]
  fn watched_paths_include_config_bases_and_resolved_scripts()
  -> anyhow::Result<()> {
    let dir = tempdir()?;
    let base = write(
      dir.path(),
      "omw.test.base.toml",
      "[agents.bob]\nruntime = \"runtime\"\nscript = \"shared/brain.rhai\"\n",
    )?;
    let config = write(
      dir.path(),
      "case/rhai/omw.test.toml",
      "[agents.alice]\nruntime = \"runtime\"\nscript = \"brain.rhai\"\n",
    )?;
    let test = Test {
      config: config.clone(),
      relative: "case/rhai/omw.test.toml".to_owned(),
      bases: vec![base.clone()],
    };

    let paths = watched_paths(&test, None);
    assert!(paths.contains(&canonical_path(&config)));
    assert!(paths.contains(&canonical_path(&base)));
    // The test config's script resolves against the test config's directory...
    assert!(
      paths.contains(&canonical_path(&dir.path().join("case/rhai/brain.rhai")))
    );
    // ...while the base's script resolves against the base's directory.
    assert!(
      paths.contains(&canonical_path(&dir.path().join("shared/brain.rhai")))
    );
    Ok(())
  }

  #[test]
  fn watched_paths_fall_back_to_config_and_bases_on_a_parse_error()
  -> anyhow::Result<()> {
    let dir = tempdir()?;
    let base = write(dir.path(), "omw.test.base.toml", "[agents.bob]\n")?;
    let config = write(dir.path(), "case/omw.test.toml", "not = = toml")?;
    let test = Test {
      config: config.clone(),
      relative: "case/omw.test.toml".to_owned(),
      bases: vec![base.clone()],
    };

    let paths = watched_paths(&test, None);
    let mut expected = vec![canonical_path(&config), canonical_path(&base)];
    expected.sort();
    assert_eq!(paths, expected);
    Ok(())
  }
}
