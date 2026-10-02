//! `omw-test`: deterministic brain testing against scripted in-process
//! doubles. Mirrors the `omw-cli` binary crate: arg parsing, config loading,
//! and initialization for `tracing` and `rustls` — plus `[assertions]` in the
//! same `omw.test.toml` that the brain's run is checked against.
//!
//! `omw-test run [path]` discovers `omw.test.toml` configs under `path` and
//! runs each one. With the non-default `compile-wasm` feature,
//! `omw-test compile-wasm <path>` is an internal helper that cross-builds a
//! rust brain file (or a tree of them) for `wasm32-wasip2`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use omw::config::Format;
use omw_output::LogFormat;

#[derive(Parser, Debug)]
#[command(name = "omw-test", about = "Deterministic OMW brain testing")]
pub struct Cli {
  /// Log format: `auto`, `tty`, `pipe` or `journald`
  #[arg(long, global = true, value_enum, default_value = "auto")]
  pub log_format: LogFormat,
  #[command(subcommand)]
  pub command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
  /// Run every discovered `omw.test.toml` once and check its assertions
  Run {
    #[command(flatten)]
    args: RunArgs,
  },

  /// Generate the JSON schema for the test configuration
  Schema {
    /// Output path
    #[arg(long)]
    output: PathBuf,
  },

  /// Compile a Rust brain file or directory into WASM components
  #[cfg(feature = "compile-wasm")]
  #[command(hide = true)]
  CompileWasm {
    #[command(flatten)]
    args: CompileWasmArgs,
  },
}

/// `run` flags: the discovery root, include/exclude globs, format and watch.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct RunArgs {
  /// Config file or directory to search (defaults to `.`)
  #[arg(default_value = ".")]
  pub path: PathBuf,
  /// Only run tests whose root-relative directory matches this glob
  /// (`*` does not cross `/`, `**` does; repeatable)
  #[arg(long, value_name = "GLOB")]
  pub include: Vec<String>,
  /// Skip tests whose root-relative directory matches this glob
  #[arg(long, value_name = "GLOB")]
  pub exclude: Vec<String>,
  /// Config format; inferred from each file's extension when omitted
  #[arg(long, value_name = "FORMAT")]
  pub format: Option<Format>,
  /// Run every discovered test; without this, stop at the first failure
  #[arg(long)]
  pub all: bool,
  /// Write per-test traces and mock snapshots (`-` / `/dev/stdout` streams
  /// them)
  #[arg(long, value_name = "PATH")]
  pub dump: Option<PathBuf>,
  /// Format of the `--dump` output (`json`, `yaml` or `toml`)
  #[arg(long, value_name = "FORMAT", default_value = "json")]
  pub dump_format: Format,
  /// Re-run on change instead of exiting
  #[arg(long)]
  pub watch: bool,
}

/// `compile-wasm` flags: the source tree, the SDK path, and tool overrides.
#[cfg(feature = "compile-wasm")]
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct CompileWasmArgs {
  /// Rust brain file (`*.rs`) or a directory to recurse
  pub path: PathBuf,
  /// Path to the `omw-wasm-rust` SDK (defaults to the in-repo crate)
  #[arg(long)]
  pub sdk: Option<PathBuf>,
  /// `cargo` binary path
  #[arg(long)]
  pub cargo: Option<String>,
  /// Extra build arg for `cargo`
  #[arg(long, short, name = "arg")]
  pub args: Vec<String>,
  /// `wasm-tools` binary path
  #[arg(long)]
  pub wasm_tools: Option<String>,
}

impl Cli {
  /// Parse the process arguments. clap handles `--help` and usage errors
  /// itself (its own exit codes and streams), so they never surface as an
  /// `anyhow` error from `main`.
  pub fn load() -> Self {
    Self::parse()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).unwrap_or_else(|error| error.exit())
  }

  fn run_args(cli: Cli) -> Option<RunArgs> {
    match cli.command {
      Command::Run { args } => Some(args),
      Command::Schema { .. } => None,
      #[cfg(feature = "compile-wasm")]
      Command::CompileWasm { .. } => None,
    }
  }

  #[cfg(feature = "compile-wasm")]
  fn compile_wasm_args(cli: Cli) -> Option<CompileWasmArgs> {
    match cli.command {
      Command::CompileWasm { args } => Some(args),
      Command::Run { .. } | Command::Schema { .. } => None,
    }
  }

  #[test]
  fn run_defaults_to_the_current_directory() {
    let args = run_args(parse(&["omw-test", "run"]));
    assert_eq!(
      args,
      Some(RunArgs {
        path: PathBuf::from("."),
        include: Vec::new(),
        exclude: Vec::new(),
        format: None,
        all: false,
        dump: None,
        dump_format: Format::Json,
        watch: false,
      })
    );
  }

  #[test]
  fn run_accepts_a_path_globs_and_watch() {
    let args = run_args(parse(&[
      "omw-test",
      "run",
      "examples",
      "--include",
      "**/rhai",
      "--include",
      "**/js",
      "--exclude",
      "**/wasm",
      "--watch",
    ]));
    assert_eq!(
      args,
      Some(RunArgs {
        path: PathBuf::from("examples"),
        include: vec!["**/rhai".to_owned(), "**/js".to_owned()],
        exclude: vec!["**/wasm".to_owned()],
        format: None,
        all: false,
        dump: None,
        dump_format: Format::Json,
        watch: true,
      })
    );
  }

  #[test]
  fn run_accepts_a_format_override() {
    let args =
      run_args(parse(&["omw-test", "run", "examples", "--format", "json"]));
    assert_eq!(args.and_then(|args| args.format), Some(Format::Json));
  }

  #[cfg(feature = "compile-wasm")]
  #[test]
  fn compile_wasm_accepts_a_path_and_sdk() {
    let args = compile_wasm_args(parse(&[
      "omw-test",
      "compile-wasm",
      "brain.rs",
      "--sdk",
      "/sdk",
    ]));
    assert_eq!(args.and_then(|args| args.sdk), Some(PathBuf::from("/sdk")));
  }

  #[cfg(feature = "compile-wasm")]
  #[test]
  fn compile_wasm_defaults_sdk_to_none() {
    let args =
      compile_wasm_args(parse(&["omw-test", "compile-wasm", "examples"]));
    assert_eq!(args.and_then(|args| args.sdk), None);
  }
}
