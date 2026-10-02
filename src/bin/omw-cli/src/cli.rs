//! Binary CLI: arg parsing, config loading, schema generation, and
//! the `run()` entrypoint. Owned by `main.rs` in the `omw-cli` crate; not
//! part of the `omw` library.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use omw::config::{Config, Format};
use omw_output::LogFormat;

#[derive(Parser, Debug)]
#[command(name = "omw", about = "OMW = OpenAI + MCP + WASM")]
pub struct Cli {
  /// Log format: `auto`, `tty`, `pipe` or `journald`
  #[arg(long, global = true, value_enum, default_value = "auto")]
  pub log_format: LogFormat,
  #[command(subcommand)]
  pub command: Command,
}

/// Shared `run` / `loop` flags: config path + format + watch.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct RunArgs {
  /// Path to the config file (defaults to the first of `omw.toml`,
  /// `omw.yaml`, `omw.yml`, `omw.json` in the current directory)
  #[arg(long)]
  pub config: Option<PathBuf>,
  /// Config format; inferred from the file extension when omitted
  #[arg(long, value_name = "FORMAT")]
  pub format: Option<Format>,
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
  /// Config format; inferred from the file extension when omitted
  #[arg(long, value_name = "FORMAT")]
  pub format: Option<Format>,
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
  /// Parse the process arguments. clap handles `--help` and usage errors
  /// itself (its own exit codes and streams), so they never surface as an
  /// `anyhow` error from `main`.
  pub fn load() -> Self {
    Self::parse()
  }
}

/// Candidate default config file names, in preference order.
const DEFAULT_CONFIG_NAMES: [&str; 4] =
  ["omw.toml", "omw.yaml", "omw.yml", "omw.json"];

/// The default config in `dir`: the first candidate that exists, else an error.
pub fn default_config_path(dir: &Path) -> Result<PathBuf> {
  for name in DEFAULT_CONFIG_NAMES {
    let candidate = dir.join(name);
    if candidate.is_file() {
      return Ok(candidate);
    }
  }
  anyhow::bail!(
    "no config found in {} (looked for {}); pass --config",
    dir.display(),
    DEFAULT_CONFIG_NAMES.join(", ")
  )
}

impl RunArgs {
  pub fn watch(&self) -> bool {
    self.watch
  }

  pub fn resolve_config_path(&self) -> Result<PathBuf> {
    match &self.config {
      Some(path) => Ok(path.clone()),
      None => default_config_path(Path::new(".")),
    }
  }

  /// Load the configuration (format inferred from the extension or forced with
  /// `--format`) overlaid with `OMW_*` environment variables.
  pub fn load_config(&self) -> Result<Config> {
    load_config(&self.resolve_config_path()?, self.format)
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

/// Load a configuration file overlaid with `OMW_*` environment variables. The
/// format is `format` when given, else inferred from the path's extension; a
/// standard stream (`-`, `/dev/stdin`, …) has no extension and needs
/// `--format`.
pub fn load_config(path: &Path, format: Option<Format>) -> Result<Config> {
  let contents = crate::stdio::read_to_string(path)?;
  let format =
    format
      .or_else(|| Format::from_path(path))
      .with_context(|| {
        format!(
          "cannot infer the config format of {}; pass --format",
          path.display()
        )
      })?;
  let raw: ::config::Config = ::config::Config::builder()
    .add_source(
      ::config::File::from_str(&contents, file_format(format)).required(false),
    )
    .add_source(env_source())
    .build()
    .context("failed to build configuration")?;
  let config: Config = raw
    .try_deserialize()
    .context("failed to deserialize configuration")?;
  tracing::info!(
    path = %path.display(),
    format = %format,
    providers = config.providers.len(),
    tooling = config.tooling.len(),
    runtime = config.runtime.len(),
    agents = config.agents.len(),
    "configuration loaded"
  );
  tracing::debug!(config = ?config, "configuration details");
  Ok(config)
}

/// Map an [`omw::config::Format`] onto the `config` crate's file format.
fn file_format(format: Format) -> ::config::FileFormat {
  match format {
    Format::Toml => ::config::FileFormat::Toml,
    Format::Yaml => ::config::FileFormat::Yaml,
    Format::Json => ::config::FileFormat::Json,
  }
}

fn env_source() -> impl ::config::Source + Send + Sync + 'static {
  ::config::Environment::with_prefix("OMW").separator("__")
}

/// The provenance banner `scaffold` prepends to the generated test config.
const SCAFFOLD_HEADER: &str = "\
# Generated by `omw scaffold`. Best-effort conversion of a deployment config
# into an `omw.test.toml`: provider/tooling/endpoint are the in-config mocks,
# pre-populated from the real back ends where possible. Fill in the provider
# `turns`, tooling `tool_calls`, and endpoint `requests` to script the run.";

/// Convert a config into a scaffolded `omw.test.toml` at `args.output_path()`.
pub async fn scaffold(args: ScaffoldArgs) -> Result<()> {
  let config = load_config(&args.config, args.format)?;
  let registries = omw::agent::Registries::default();
  let table =
    omw::testing::scaffold(&config, &registries, !args.no_resources).await?;
  let body = toml::to_string_pretty(&table)
    .context("failed to serialize the scaffolded config")?;
  let rendered = format!("{SCAFFOLD_HEADER}\n{body}");
  let output = args.output_path();
  // A stream target (`-`, `/dev/stdout`, …) is not a file to overwrite.
  let stream = crate::stdio::is_stdout(&output);
  if !stream && output.exists() && !args.force {
    anyhow::bail!(
      "{} already exists; pass --force to overwrite",
      output.display()
    );
  }
  crate::stdio::write(&output, &rendered)?;
  tracing::info!(path = %output.display(), "scaffolded test config");
  if !stream {
    println!("{}", output.display());
  }
  Ok(())
}

/// Generate the JSON schema for the configuration and write it to `path`.
pub fn generate_schema(path: &Path) -> Result<()> {
  let contents = Config::schema_json()?;
  crate::stdio::write(path, &contents)?;
  tracing::info!("wrote configuration schema to {}", path.display());
  Ok(())
}

pub async fn run() -> ExitCode {
  let cli = Cli::load();
  let live = crate::log::init(&cli.command, cli.log_format);
  if let Some(live) = &live {
    live.set_view(omw_output::View::Agents);
    live.set_command(omw_output::command_line());
  }

  crate::tls::init();

  tracing::info!(command = ?cli.command, "omw starting");

  // The binary owns the OS signal subscription; the library takes the latch.
  let shutdown = omw::shutdown::Shutdown::new();
  let signal = crate::shutdown::install(&shutdown);

  let result = dispatch(cli.command, shutdown.clone(), live.as_ref()).await;

  signal.abort();

  if let Err(error) = &result {
    omw_output::report_error(error);
  }
  drop(live);

  match result {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("omw: {error:#}");
      ExitCode::FAILURE
    }
  }
}

/// Dispatch a parsed command, returning the run's result for `run` to render.
async fn dispatch(
  command: Command,
  shutdown: omw::shutdown::Shutdown,
  live: Option<&omw_output::Live>,
) -> Result<()> {
  match command {
    Command::Run { args } => {
      let path = args.resolve_config_path()?;
      let config = args.load_config()?;
      let registries = omw::agent::Registries::default();
      announce(live, &config, &path);
      let run = omw::agent::run_agents(
        &config,
        args.watch(),
        &registries,
        shutdown.clone(),
      );
      drive(run, live, &shutdown).await
    }
    Command::Loop { args } => {
      let path = args.resolve_config_path()?;
      let config = args.load_config()?;
      let registries = omw::agent::Registries::default();
      announce(live, &config, &path);
      let run = omw::agent::loop_agents(
        &config,
        args.watch(),
        &registries,
        shutdown.clone(),
      );
      drive(run, live, &shutdown).await
    }
    Command::Schema { output } => generate_schema(&output),
    Command::Scaffold { args } => scaffold(args).await,
  }
}

/// Announce the run in the live view's info panel and status line.
fn announce(live: Option<&omw_output::Live>, config: &Config, path: &Path) {
  if let Some(live) = live {
    live.configure(
      config.tunables.tui_tick_ms,
      config.tunables.tui_tab_capacity,
    );
    let mut details = vec![
      ("config".to_owned(), path.display().to_string()),
      ("agents".to_owned(), config.agents.len().to_string()),
    ];
    if let Some(endpoint) = &config.endpoint
      && let Some(listen) =
        endpoint.params.get("listen").and_then(|v| v.as_str())
    {
      details.push(("endpoint".to_owned(), listen.to_owned()));
    }
    live.set_details(details);
    live.set_busy(true);
    live.set_status(format!("{} agent(s) running", config.agents.len()));
  }
}

/// Await `run`, requesting shutdown when the live view reports a quit.
///
/// Without a live view this is just `run.await`.
async fn drive(
  run: impl std::future::Future<Output = Result<()>>,
  live: Option<&omw_output::Live>,
  shutdown: &omw::shutdown::Shutdown,
) -> Result<()> {
  let Some(quit) = live.and_then(omw_output::Live::take_quit_receiver) else {
    return run.await;
  };
  tokio::pin!(run);
  tokio::select! {
    result = &mut run => result,
    _ = tokio::task::spawn_blocking(move || quit.recv()) => {
      tracing::info!("live view quit requested");
      shutdown.request();
      run.await
    }
  }
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
      format: None,
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

        [agents.alice]
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
    assert_eq!(cfg.agents["alice"].runtime, "rhai");
    assert_eq!(cfg.agents["alice"].script, "brain.rhai");
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
        listen = "127.0.0.1:37532"
      "#,
    )?;
    let cfg = cli(path).load_config()?;
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
  fn endpoint_defaults_to_none() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    std::fs::write(&path, "")?;
    let cfg = cli(path).load_config()?;
    assert!(cfg.endpoint.is_none());
    Ok(())
  }

  #[test]
  fn default_config_path_prefers_toml_then_yaml_then_json() -> anyhow::Result<()>
  {
    let dir = tempdir()?;
    std::fs::write(dir.path().join("omw.json"), "{}")?;
    std::fs::write(dir.path().join("omw.yaml"), "")?;
    std::fs::write(dir.path().join("omw.toml"), "")?;
    assert_eq!(
      default_config_path(dir.path())?,
      dir.path().join("omw.toml")
    );

    std::fs::remove_file(dir.path().join("omw.toml"))?;
    assert_eq!(
      default_config_path(dir.path())?,
      dir.path().join("omw.yaml")
    );

    std::fs::remove_file(dir.path().join("omw.yaml"))?;
    assert_eq!(
      default_config_path(dir.path())?,
      dir.path().join("omw.json")
    );
    Ok(())
  }

  #[test]
  fn default_config_path_errors_when_none_exists() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let error = default_config_path(dir.path())
      .expect_err("should error without a config");
    assert!(error.to_string().contains("no config found"));
    Ok(())
  }

  #[test]
  fn resolve_config_path_honors_override() -> anyhow::Result<()> {
    let cli = cli(PathBuf::from("custom.toml"));
    assert_eq!(cli.resolve_config_path()?, PathBuf::from("custom.toml"));
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn yaml_config_deserializes_by_extension() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.yaml");
    std::fs::write(
      &path,
      "providers:\n  openai:\n    kind: openai\n    api_key: sk-test\nagents:\n  alice:\n    runtime: rhai\n    script: brain.rhai\n",
    )?;
    let cfg = cli(path).load_config()?;
    assert_eq!(cfg.providers["openai"].kind, "openai");
    assert_eq!(cfg.providers["openai"].params["api_key"], "sk-test");
    assert_eq!(cfg.agents["alice"].script, "brain.rhai");
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn json_config_deserializes_by_extension() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.json");
    std::fs::write(
      &path,
      r#"{"providers":{"openai":{"kind":"openai","api_key":"sk-test"}}}"#,
    )?;
    let cfg = cli(path).load_config()?;
    assert_eq!(cfg.providers["openai"].kind, "openai");
    assert_eq!(cfg.providers["openai"].params["api_key"], "sk-test");
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn format_override_parses_an_extensionless_file() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.conf");
    std::fs::write(&path, "{\"agents\":{}}")?;
    let args = RunArgs {
      config: Some(path),
      format: Some(Format::Json),
      watch: false,
    };
    let cfg = args.load_config()?;
    assert!(cfg.agents.is_empty());
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn extensionless_file_without_format_errors() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.conf");
    std::fs::write(&path, "")?;
    let error = cli(path)
      .load_config()
      .expect_err("should require an explicit format");
    assert!(error.to_string().contains("cannot infer"), "{error:#}");
    Ok(())
  }

  #[test]
  #[serial(env)]
  fn missing_config_errors() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("omw.toml");
    let error = cli(path)
      .load_config()
      .expect_err("should error on a missing config");
    assert!(error.to_string().contains("failed to read"), "{error:#}");
    Ok(())
  }

  #[test]
  fn scaffold_defaults_output_next_to_the_config() {
    let args = ScaffoldArgs {
      config: PathBuf::from("cases/a/omw.toml"),
      format: None,
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
      format: None,
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
      "--format",
      "yaml",
      "--force",
      "--no-resources",
    ])
    .expect("scaffold args should parse");
    match cli.command {
      Command::Scaffold { args } => {
        assert_eq!(args.config, PathBuf::from("omw.toml"));
        assert_eq!(args.format, Some(Format::Yaml));
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
