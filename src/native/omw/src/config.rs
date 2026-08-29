//! TOML configuration: global provider/tooling/runtime implementations plus
//! per-agent wiring.
//!
//! Config is deliberately impl-agnostic: each provider/tooling/runtime entry
//! is a `kind` string plus an opaque params blob. The kind is validated and
//! the params are deserialized into an impl-specific struct only when the impl
//! is constructed (see the `build` factories in `provider`, `tooling` and
//! `runtime`).

use std::{
  collections::HashMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A single configured implementation: which kind plus opaque params.
#[derive(Debug, Deserialize, Clone, Serialize, JsonSchema)]
pub struct ImplConfig {
  /// Which implementation this is.
  pub kind: String,
  /// Impl-specific options, validated at construction time.
  #[serde(flatten)]
  pub params: serde_json::Value,
}

/// OMW configuration.
#[derive(Debug, Deserialize, Clone, Serialize, JsonSchema)]
pub struct Config {
  /// Named provider implementations.
  #[serde(default)]
  pub providers: HashMap<String, ImplConfig>,
  /// Named tooling implementations.
  #[serde(default)]
  pub tooling: HashMap<String, ImplConfig>,
  /// Named runtime implementations.
  #[serde(default)]
  pub runtime: HashMap<String, ImplConfig>,
  #[serde(default)]
  pub agents: Vec<AgentConfig>,
}

/// A single agent wiring itself to the globals above.
#[derive(Debug, Deserialize, Clone, Serialize, JsonSchema)]
pub struct AgentConfig {
  pub name: String,
  /// Which named runtime implementation this agent's brain uses.
  pub runtime: String,
  /// The agent's brain script.
  pub script: String,
}

#[derive(Parser, Debug)]
#[command(name = "omw", about = "OMW = OpenAI + MCP + WASM")]
pub struct Cli {
  #[command(subcommand)]
  pub command: Command,

  /// Path to the config file (defaults to `omw.toml` in the current directory)
  #[arg(long, global = true)]
  pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
  /// Run all configured agents once
  Run,
  /// Loop all configured agents
  Loop,
  /// Generate the JSON schema for the configuration
  Schema {
    /// Output path
    #[arg(long)]
    output: PathBuf,
  },
}

impl Cli {
  pub fn resolve_config_path(&self) -> PathBuf {
    self
      .config
      .clone()
      .unwrap_or_else(|| PathBuf::from("omw.toml"))
  }

  /// Load the configuration from `omw.toml` (optional) overlaid with
  /// `OMW_*` environment variables.
  pub fn load_config(&self) -> Result<Config> {
    let path = self.resolve_config_path();
    let env = ::config::Environment::with_prefix("OMW").separator("__");
    // The `config` crate cannot read `/dev/stdin` (an extension-less stream,
    // unlike a regular file path), so read stdin explicitly and inject it
    // via `File::from_str` when the config arrives through a stream.
    let raw: ::config::Config = if path == Path::new("/dev/stdin") {
      let contents = std::io::read_to_string(std::io::stdin())
        .context("failed to read configuration from stdin")?;
      ::config::Config::builder()
        .add_source(
          ::config::File::from_str(&contents, ::config::FileFormat::Toml)
            .required(false),
        )
        .add_source(env)
        .build()
    } else {
      ::config::Config::builder()
        .add_source(::config::File::from(path.as_path()).required(false))
        .add_source(env)
        .build()
    }
    .context("failed to build configuration")?;
    let config: Config = raw
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
    tracing::debug!(
      config = %redacted_config(&config),
      "configuration details"
    );
    Ok(config)
  }

  pub fn load() -> anyhow::Result<Self> {
    Ok(Self::try_parse()?)
  }
}

/// Redact known secret keys (`api_key`, `auth_token`) from a JSON value,
/// descending into nested objects and arrays.
fn redact(value: &mut serde_json::Value) {
  match value {
    serde_json::Value::Object(map) => {
      for (key, child) in map {
        if matches!(key.as_str(), "api_key" | "auth_token") {
          *child = serde_json::Value::String("<redacted>".to_string());
        } else {
          redact(child);
        }
      }
    }
    serde_json::Value::Array(items) => {
      for item in items {
        redact(item);
      }
    }
    _ => {}
  }
}

/// A debug render of a [`Config`] with known secret params redacted, so the
/// full shape can be logged without leaking `api_key` / `auth_token`.
fn redacted_config(cfg: &Config) -> String {
  let mut json = match serde_json::to_value(cfg) {
    Ok(json) => json,
    Err(_) => return "<unserializable>".to_string(),
  };
  redact(&mut json);
  serde_json::to_string(&json)
    .unwrap_or_else(|_| "<unserializable>".to_string())
}

/// Generate the JSON schema for the configuration and write it to `path`.
pub fn generate_schema(path: &Path) -> Result<()> {
  let schema = schemars::schema_for!(Config);
  let json = serde_json::to_string_pretty(&schema)
    .context("failed to serialize schema")?;
  if let Some(parent) = path.parent()
    && !parent.as_os_str().is_empty()
  {
    std::fs::create_dir_all(parent).with_context(|| {
      format!("failed to create directory {}", parent.display())
    })?;
  }
  let contents = format!("{json}\n");
  std::fs::write(path, contents)
    .with_context(|| format!("failed to write schema to {}", path.display()))?;
  tracing::info!("wrote configuration schema to {}", path.display());
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use serial_test::serial;
  use std::path::PathBuf;
  use tempfile::tempdir;

  fn cli(path: PathBuf) -> Cli {
    Cli {
      command: Command::Run,
      config: Some(path),
    }
  }

  /// Removes the named variables on drop, so a failing test never leaks
  /// process-global env into sibling tests.
  struct EnvSet {
    value: HashMap<String, String>,
  }

  impl EnvSet {
    pub fn new(value: HashMap<String, String>) -> Self {
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
  fn toml_config_deserializes_with_flattened_params() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(
      &path,
      r#"
        [providers.openai]
        kind = "openai"
        base_url = "https://example.com/v1"
        api_key = "sk-test"
        model = "gpt-test"

        [tooling.mcp]
        kind = "mcp"
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

    let cfg = cli(path).load_config()?;

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
  fn resolve_config_path_defaults_to_omw_toml() {
    let cli = Cli {
      command: Command::Run,
      config: None,
    };
    assert_eq!(cli.resolve_config_path(), PathBuf::from("omw.toml"));
  }

  #[test]
  fn resolve_config_path_honors_override() {
    let cli = cli(PathBuf::from("custom.toml"));
    assert_eq!(cli.resolve_config_path(), PathBuf::from("custom.toml"));
  }

  #[test]
  #[serial(env)]
  fn unknown_kind_is_preserved_until_factory_time() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(
      &path,
      r#"
        [providers.custom]
        kind = "custom-thing"
        foo = "bar"
      "#,
    )?;
    let cfg = cli(path).load_config()?;
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
  fn env_overlay_merges_over_file() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(
      &path,
      r#"
        [providers.openai]
        kind = "openai"
        api_key = "from-file"
      "#,
    )?;

    let mut vars = HashMap::new();
    vars.insert(
      "OMW__PROVIDERS__OPENAI__API_KEY".to_owned(),
      "from-env".to_owned(),
    );
    let _vars = EnvSet::new(vars);

    let cfg = cli(path).load_config()?;
    let provider = cfg
      .providers
      .get("openai")
      .ok_or_else(|| anyhow::anyhow!("missing openai provider"))?;
    assert_eq!(provider.kind, "openai");
    assert_eq!(provider.params["api_key"], "from-env");
    Ok(())
  }
}
