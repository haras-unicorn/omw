//! Recursive `omw.test.toml` discovery, root-relative include/exclude
//! filtering, and config loading for `omw-test run`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use glob::Pattern;
use omw::config::Config;
use omw::testing::{Assertions, parse};

/// The exact file name `omw-test` discovers. `omw.test.template.toml` is
/// never collected.
const CONFIG_NAME: &str = "omw.test.toml";

/// One discovered test: its config file and its directory relative to the
/// discovery root (used by `--include` / `--exclude`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Test {
  pub config: PathBuf,
  pub relative_directory: String,
}

impl Test {
  /// The label printed by `run`: the root-relative directory.
  pub fn label(&self) -> &str {
    &self.relative_directory
  }
}

/// Discover tests under `path`: a file is a single test, a directory is a
/// recursive walk for `omw.test.toml` (hidden directories are skipped).
pub fn discover(path: &Path) -> Result<Vec<Test>> {
  if !path.exists() {
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
        relative_directory: relative_dir(&config, base),
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

/// Keep tests whose root-relative directory matches an `--include` glob (or
/// any include when none are given) and no `--exclude` glob.
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
      .filter(|test| keep(&test.relative_directory, &include, &exclude))
      .collect(),
  )
}

/// Read a test config and parse both the runnable [`Config`] (with the
/// `OMW_TEST__` environment overlay) and its `[assertions]` section.
pub fn load(path: &Path) -> Result<(Config, Assertions)> {
  let raw = std::fs::read_to_string(path)
    .with_context(|| format!("failed to read config {}", path.display()))?;
  let env = ::config::Environment::with_prefix("OMW_TEST").separator("__");
  let source: ::config::Config = ::config::Config::builder()
    .add_source(
      ::config::File::from_str(&raw, ::config::FileFormat::Toml)
        .required(false),
    )
    .add_source(env)
    .build()
    .context("failed to build configuration")?;
  let config: Config = source
    .try_deserialize()
    .context("failed to deserialize configuration")?;
  tracing::info!(
    path = %path.display(),
    providers = config.providers.len(),
    tooling = config.tooling.len(),
    runtime = config.runtime.len(),
    agents = config.agents.len(),
    "configuration loaded"
  );
  tracing::debug!(config = ?config, "configuration details");
  let assertions = parse(&raw)?;
  Ok((config, assertions))
}

/// The process-wide testing tunables, read from the `OMW_TEST__` environment
/// overlay (default when unset). Used by the `--watch` debounce, which is not
/// tied to any single test config.
pub fn env_tunables() -> Result<omw::config::Tunables> {
  let env = ::config::Environment::with_prefix("OMW_TEST").separator("__");
  let source = ::config::Config::builder()
    .add_source(env)
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
/// Absolute script paths are left untouched.
pub fn resolve_scripts(config: &mut Config, path: &Path) {
  let base = path
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
    .unwrap_or(Path::new("."));
  for agent in &mut config.agents {
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
    } else if path.file_name().is_some_and(|name| name == CONFIG_NAME) {
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

fn relative_dir(config: &Path, root: &Path) -> String {
  let dir = config.parent().unwrap_or(Path::new("."));
  match dir.strip_prefix(root) {
    Ok(rel) if rel.as_os_str().is_empty() => ".".to_owned(),
    Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
    Err(_) => ".".to_owned(),
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

  fn write(dir: &Path, rel: &str, contents: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, contents)?;
    Ok(path)
  }

  fn relative_directories(tests: &[Test]) -> Vec<&str> {
    tests
      .iter()
      .map(|test| test.relative_directory.as_str())
      .collect()
  }

  #[test]
  fn discover_walks_a_directory_tree_in_sorted_order() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), "02-tool-agent/rhai/omw.test.toml", "")?;
    write(dir.path(), "01-hello/wasm/omw.test.toml", "")?;
    write(dir.path(), "01-hello/rhai/omw.test.toml", "")?;
    // A template and a differently-named config are never collected.
    write(dir.path(), "01-hello/omw.test.template.toml", "")?;
    write(dir.path(), "01-hello/omw.toml", "")?;

    let tests = discover(dir.path())?;
    assert_eq!(
      relative_directories(&tests),
      vec!["01-hello/rhai", "01-hello/wasm", "02-tool-agent/rhai"]
    );
    Ok(())
  }

  #[test]
  fn discover_skips_hidden_directories() -> anyhow::Result<()> {
    let dir = tempdir()?;
    write(dir.path(), ".git/omw.test.toml", "")?;
    write(dir.path(), "case/omw.test.toml", "")?;
    let tests = discover(dir.path())?;
    assert_eq!(relative_directories(&tests), vec!["case"]);
    Ok(())
  }

  #[test]
  fn discover_treats_a_file_as_a_single_test() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let config = write(dir.path(), "nested/omw.test.toml", "")?;
    let tests = discover(&config)?;
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].config, config);
    assert_eq!(tests[0].relative_directory, ".");
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
      &["**/rhai".to_owned()],
      &["01-hello/**".to_owned()],
    )?;
    assert_eq!(relative_directories(&selected), vec!["02-tool-agent/rhai"]);

    let selected = filter(tests, &[], &["**/wasm".to_owned()])?;
    assert_eq!(
      relative_directories(&selected),
      vec!["01-hello/js", "01-hello/rhai", "02-tool-agent/rhai"]
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

        [[agents]]
        name = "alice"
        runtime = "rhai"
        script = "brain.rhai"
      "#,
    )?;

    let (cfg, _) = load(&path)?;
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
    assert_eq!(cfg.agents[0].name, "alice");
    assert_eq!(cfg.agents[0].runtime, "rhai");
    assert_eq!(cfg.agents[0].script, "brain.rhai");
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
        listen = "127.0.0.1:8080"
      "#,
    )?;
    let (cfg, _) = load(&path)?;
    let endpoint = cfg
      .endpoint
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("missing endpoint"))?;
    assert_eq!(endpoint.kind, "openai");
    assert_eq!(endpoint.params["listen"], "127.0.0.1:8080");
    assert!(endpoint.params.get("kind").is_none());
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn endpoint_defaults_to_none() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = write(dir.path(), "omw.test.toml", "")?;
    let (cfg, _) = load(&path)?;
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
    let (cfg, _) = load(&path)?;
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
    let (cfg, _) = load(&path)?;
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
    let (cfg, _) = load(&path)?;
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

        [[agents]]
        name = "alice"
        runtime = "rhai"
        script = "brain.rhai"
      "#,
    )?;
    let (mut config, _) = load(&path)?;
    resolve_scripts(&mut config, &path);
    let expected = path
      .parent()
      .ok_or_else(|| anyhow::anyhow!("config has no parent"))?
      .join("brain.rhai");
    assert_eq!(config.agents[0].script, expected.to_string_lossy());
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

        [[agents]]
        name = "alice"
        runtime = "rhai"
        script = "/absolute/brain.rhai"
      "#,
    )?;
    let (mut config, _) = load(&path)?;
    resolve_scripts(&mut config, &path);
    assert_eq!(config.agents[0].script, "/absolute/brain.rhai");
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

    let (cfg, _) = load(&path)?;
    let provider = cfg
      .providers
      .get("openai")
      .ok_or_else(|| anyhow::anyhow!("missing openai provider"))?;
    assert_eq!(provider.kind, "openai");
    assert_eq!(provider.params["api_key"], "from-env");
    Ok(())
  }
}
