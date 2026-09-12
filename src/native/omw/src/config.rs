//! TOML configuration: global provider/tooling/runtime implementations plus
//! per-agent wiring.
//!
//! Config is deliberately impl-agnostic: each provider/tooling/runtime/endpoint
//! entry is a `kind` string plus an opaque params blob. The kind is validated
//! and the params are deserialized into an impl-specific struct only when the
//! impl is constructed (see the `build` factories in `provider`, `tooling`,
//! `runtime` and `endpoint`).

use std::{
  collections::HashMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A single configured implementation: which kind plus opaque params.
#[derive(Deserialize, Clone, Serialize, JsonSchema)]
pub struct ImplConfig {
  /// Which implementation this is.
  pub kind: String,
  /// Impl-specific options, validated at construction time.
  #[serde(flatten)]
  pub params: serde_json::Value,
}

impl std::fmt::Debug for ImplConfig {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self.params.as_object() {
      Some(map) => {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        f.debug_struct("ImplConfig")
          .field("kind", &self.kind)
          .field("params_keys", &keys)
          .finish()
      }
      None => f
        .debug_struct("ImplConfig")
        .field("kind", &self.kind)
        .field("params", &"<non-object>")
        .finish(),
    }
  }
}

/// Runtime tunables: channel sizes, timeouts, and backoffs. All optional;
/// omitted values fall back to the built-in defaults.
#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema,
)]
#[serde(default)]
pub struct Tunables {
  /// How many events a single agent inbox buffers before sends fail.
  #[serde(default = "default_inbox_bound")]
  pub inbox_bound: usize,
  /// How long `recv_while` parks between early-abort checks, in ms.
  #[serde(default = "default_recv_slice_ms")]
  pub recv_slice_ms: u64,
  /// How long a blocking `recv` waits before timing out, in seconds.
  #[serde(default = "default_recv_timeout_secs")]
  pub recv_timeout_secs: u64,
  /// How long the blocking-call helper waits between reload checks, in ms.
  #[serde(default = "default_reload_poll_ms")]
  pub reload_poll_ms: u64,
  /// Uninterrupted runtime execution allowed after grace expires, in ms.
  #[serde(default = "default_interrupt_budget_ms")]
  pub interrupt_budget_ms: u64,
  /// How long the supervisor waits for a cooperative exit, in seconds.
  #[serde(default = "default_reload_grace_secs")]
  pub reload_grace_secs: u64,
  /// Backoff start for `loop` restarts on failure, in ms.
  #[serde(default = "default_loop_backoff_start_ms")]
  pub loop_backoff_start_ms: u64,
  /// Backoff cap for `loop` restarts on failure, in seconds.
  #[serde(default = "default_loop_backoff_cap_secs")]
  pub loop_backoff_cap_secs: u64,
  /// How long to coalesce the burst of file events a single save produces,
  /// in ms.
  #[serde(default = "default_watch_debounce_ms")]
  pub watch_debounce_ms: u64,
  /// How many deltas a single endpoint session buffers before drops.
  #[serde(default = "default_session_buffer")]
  pub session_buffer: usize,
  /// Cancel open pumps (streams, timers, resources, tool calls) on reload.
  /// `false` keeps them across reload.
  #[serde(default = "default_cancel_pumps_on_reload")]
  pub cancel_pumps_on_reload: bool,
}

fn default_inbox_bound() -> usize {
  1024
}

fn default_recv_slice_ms() -> u64 {
  200
}

fn default_recv_timeout_secs() -> u64 {
  60
}

fn default_reload_poll_ms() -> u64 {
  200
}

fn default_interrupt_budget_ms() -> u64 {
  100
}

fn default_reload_grace_secs() -> u64 {
  5
}

fn default_loop_backoff_start_ms() -> u64 {
  100
}

fn default_loop_backoff_cap_secs() -> u64 {
  30
}

fn default_watch_debounce_ms() -> u64 {
  200
}

fn default_session_buffer() -> usize {
  8192
}

fn default_cancel_pumps_on_reload() -> bool {
  true
}

impl Default for Tunables {
  fn default() -> Self {
    Self {
      inbox_bound: default_inbox_bound(),
      recv_slice_ms: default_recv_slice_ms(),
      recv_timeout_secs: default_recv_timeout_secs(),
      reload_poll_ms: default_reload_poll_ms(),
      interrupt_budget_ms: default_interrupt_budget_ms(),
      reload_grace_secs: default_reload_grace_secs(),
      loop_backoff_start_ms: default_loop_backoff_start_ms(),
      loop_backoff_cap_secs: default_loop_backoff_cap_secs(),
      watch_debounce_ms: default_watch_debounce_ms(),
      session_buffer: default_session_buffer(),
      cancel_pumps_on_reload: default_cancel_pumps_on_reload(),
    }
  }
}

impl Tunables {
  pub fn recv_slice(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.recv_slice_ms)
  }

  pub fn recv_timeout(&self) -> std::time::Duration {
    std::time::Duration::from_secs(self.recv_timeout_secs)
  }

  pub fn reload_poll(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.reload_poll_ms)
  }

  pub fn interrupt_budget(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.interrupt_budget_ms)
  }

  pub fn reload_grace(&self) -> std::time::Duration {
    std::time::Duration::from_secs(self.reload_grace_secs)
  }

  pub fn loop_backoff_start(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.loop_backoff_start_ms)
  }

  pub fn loop_backoff_cap(&self) -> std::time::Duration {
    std::time::Duration::from_secs(self.loop_backoff_cap_secs)
  }

  pub fn watch_debounce(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.watch_debounce_ms)
  }
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

  /// Optional endpoint implementation: a `kind` string plus opaque params,
  /// like providers/tooling/runtime. When set, a server is started and the
  /// agents can subscribe to it as models.
  #[serde(default)]
  pub endpoint: Option<ImplConfig>,
  #[serde(default)]
  pub agents: Vec<AgentConfig>,
  /// Global runtime tunables; all optional with built-in defaults.
  #[serde(default)]
  pub tunables: Tunables,
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
}

/// Shared `run` / `loop` flags: config path + watch.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct RunArgs {
  /// Path to the config file (defaults to `omw.toml` in the current directory)
  #[arg(long)]
  pub config: Option<PathBuf>,
  /// Watch agent scripts and restart agents when their script changes
  #[arg(long)]
  pub watch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
  /// Run all configured agents once
  Run {
    #[command(flatten)]
    args: RunArgs,
  },
  /// Loop all configured agents
  Loop {
    #[command(flatten)]
    args: RunArgs,
  },
  /// Generate the JSON schema for the configuration
  Schema {
    /// Output path
    #[arg(long)]
    output: PathBuf,
  },
}

impl Cli {
  pub fn load() -> anyhow::Result<Self> {
    Ok(Self::try_parse()?)
  }
}

impl RunArgs {
  pub fn watch(&self) -> bool {
    self.watch
  }

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
    tracing::debug!(config = ?config, "configuration details");
    Ok(config)
  }
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

  fn cli(path: PathBuf) -> RunArgs {
    RunArgs {
      config: Some(path),
      watch: false,
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
  #[serial(env)]
  fn endpoint_deserializes_with_kind_and_opaque_params() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(
      &path,
      r#"
        [endpoint]
        kind = "openai"
        listen = "127.0.0.1:8080"
      "#,
    )?;
    let cfg = cli(path).load_config()?;
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
    let path = dir.path().join("omw.toml");
    std::fs::write(&path, "")?;
    let cfg = cli(path).load_config()?;
    assert!(cfg.endpoint.is_none());
    Ok(())
  }

  #[test]
  fn resolve_config_path_defaults_to_omw_toml() {
    let args = RunArgs {
      config: None,
      watch: false,
    };
    assert_eq!(args.resolve_config_path(), PathBuf::from("omw.toml"));
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
  fn tunables_default_and_override() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(&path, "[tunables]\nrecv_timeout_secs = 30\n")?;
    let cfg = cli(path).load_config()?;
    assert_eq!(cfg.tunables.recv_timeout_secs, 30);
    assert_eq!(
      cfg.tunables,
      Tunables {
        recv_timeout_secs: 30,
        ..Tunables::default()
      }
    );
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
