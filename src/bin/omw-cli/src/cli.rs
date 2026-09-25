//! Binary CLI: arg parsing, config loading, schema generation, and
//! the `run()` entrypoint. Owned by `main.rs` in the `omw-cli` crate; not
//! part of the `omw` library.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use omw::config::Config;

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
  /// Convert a config into a scaffolded `omw.test.toml`
  Scaffold {
    #[command(flatten)]
    args: ScaffoldArgs,
  },
  /// Generate the JSON schema for the configuration
  Schema {
    /// Output path
    #[arg(long)]
    output: PathBuf,
  },
}

/// `scaffold` flags: the source config, output path, and introspection knobs.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct ScaffoldArgs {
  /// Path to the config to convert
  pub config: PathBuf,
  /// Output path (defaults to `omw.test.toml` next to the config)
  #[arg(long)]
  pub output: Option<PathBuf>,
  /// Overwrite the output file if it already exists
  #[arg(long)]
  pub force: bool,
  /// Do not enumerate or read tooling resources
  #[arg(long)]
  pub no_resources: bool,
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
    load_config(&self.resolve_config_path())
  }
}

impl ScaffoldArgs {
  /// The output path: `--output`, or `omw.test.toml` next to the config.
  pub fn output_path(&self) -> PathBuf {
    self.output.clone().unwrap_or_else(|| {
      self
        .config
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("omw.test.toml")
    })
  }
}

/// Load a configuration file overlaid with `OMW_*` environment variables.
pub fn load_config(path: &Path) -> Result<Config> {
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
      .add_source(env_source())
      .build()
  } else {
    ::config::Config::builder()
      .add_source(::config::File::from(path).required(false))
      .add_source(env_source())
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

fn env_source() -> impl ::config::Source + Send + Sync + 'static {
  ::config::Environment::with_prefix("OMW").separator("__")
}

/// Convert a config into a scaffolded `omw.test.toml` at `args.output_path()`.
pub async fn scaffold(args: ScaffoldArgs) -> Result<()> {
  let config = load_config(&args.config)?;
  let registries = omw::agent::Registries::default();
  let rendered =
    omw::testing::scaffold(&config, &registries, !args.no_resources).await?;
  let output = args.output_path();
  if output.exists() && !args.force {
    anyhow::bail!(
      "{} already exists; pass --force to overwrite",
      output.display()
    );
  }
  if let Some(parent) = output.parent()
    && !parent.as_os_str().is_empty()
  {
    std::fs::create_dir_all(parent).with_context(|| {
      format!("failed to create directory {}", parent.display())
    })?;
  }
  std::fs::write(&output, rendered)
    .with_context(|| format!("failed to write {}", output.display()))?;
  tracing::info!(path = %output.display(), "scaffolded test config");
  println!("{}", output.display());
  Ok(())
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

pub async fn run() -> anyhow::Result<()> {
  crate::log::init();

  crate::tls::init();

  let cli = Cli::load()?;
  tracing::info!(command = ?cli.command, "omw starting");

  let result = match cli.command {
    Command::Run { args } => {
      let config = args.load_config()?;
      let registries = omw::agent::Registries::default();
      omw::agent::run_agents(&config, args.watch(), &registries).await
    }
    Command::Loop { args } => {
      let config = args.load_config()?;
      let registries = omw::agent::Registries::default();
      omw::agent::loop_agents(&config, args.watch(), &registries).await
    }
    Command::Schema { output } => generate_schema(&output),
    Command::Scaffold { args } => scaffold(args).await,
  };

  if let Err(error) = &result {
    tracing::error!(error = %error, "omw terminated with an error");
  }

  result
}

#[cfg(test)]
mod tests {
  use super::*;
  use serial_test::serial;
  use std::collections::HashMap;
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
  fn scaffold_defaults_output_next_to_the_config() {
    let args = ScaffoldArgs {
      config: PathBuf::from("cases/a/omw.toml"),
      output: None,
      force: false,
      no_resources: false,
    };
    assert_eq!(args.output_path(), PathBuf::from("cases/a/omw.test.toml"));
  }

  #[test]
  fn scaffold_honors_an_explicit_output() {
    let args = ScaffoldArgs {
      config: PathBuf::from("omw.toml"),
      output: Some(PathBuf::from("out/omw.test.toml")),
      force: true,
      no_resources: true,
    };
    assert_eq!(args.output_path(), PathBuf::from("out/omw.test.toml"));
  }

  #[test]
  fn scaffold_parses_with_flags() {
    let cli = Cli::try_parse_from([
      "omw",
      "scaffold",
      "omw.toml",
      "--force",
      "--no-resources",
    ])
    .expect("scaffold args should parse");
    match cli.command {
      Command::Scaffold { args } => {
        assert_eq!(args.config, PathBuf::from("omw.toml"));
        assert!(args.force);
        assert!(args.no_resources);
      }
      other => panic!("expected scaffold, got {other:?}"),
    }
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
      omw::config::Tunables {
        recv_timeout_secs: 30,
        ..omw::config::Tunables::default()
      }
    );
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn trace_buffer_tunable_default_and_override() -> anyhow::Result<()> {
    let defaults = omw::config::Tunables::default();
    assert_eq!(
      defaults.trace_buffer,
      omw::host::trace::DEFAULT_TRACE_BUFFER
    );
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(&path, "[tunables]\ntrace_buffer = 16\n")?;
    let cfg = cli(path).load_config()?;
    assert_eq!(cfg.tunables.trace_buffer, 16);
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn tooling_connect_backoff_tunables_default_and_override()
  -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(
      &path,
      "[tunables]\ntooling_connect_backoff_start_ms = 50\ntooling_connect_backoff_cap_secs = 5\n",
    )?;
    let cfg = cli(path).load_config()?;
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
