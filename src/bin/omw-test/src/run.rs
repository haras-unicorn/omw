//! `omw-test` entry point: discover test configs, run each through the
//! traced path, and check its assertions.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use omw::watch::{RecursiveMode, Watcher, scope};

#[cfg(feature = "compile-wasm")]
use crate::cli::CompileWasmArgs;
use crate::cli::{Cli, Command, RunArgs};
use crate::collect::{self, Test};

/// Run the `omw-test` binary.
pub async fn run() -> Result<()> {
  crate::log::init();
  crate::tls::init();

  let cli = Cli::load()?;
  tracing::info!(command = ?cli.command, "omw-test starting");

  let result = match cli.command {
    Command::Run { args } => run_tests(args).await,
    #[cfg(feature = "compile-wasm")]
    Command::CompileWasm { args } => compile_wasm(&args),
  };

  if let Err(error) = &result {
    tracing::error!(error = %error, "omw-test terminated with an error");
  }
  result
}

async fn run_tests(args: RunArgs) -> Result<()> {
  if !args.watch {
    let tests = collect::filter(
      collect::discover(&args.path)?,
      &args.include,
      &args.exclude,
    )?;
    return run_pass(&tests, &args.path).await;
  }
  // Hold one watcher across reruns so a change between passes is not missed.
  let debounce = collect::env_tunables()?.watch_debounce();
  let mut watcher =
    Watcher::watch(&scope(&args.path), RecursiveMode::Recursive, debounce)?;
  loop {
    let tests = collect::filter(
      collect::discover(&args.path)?,
      &args.include,
      &args.exclude,
    )?;
    if let Err(error) = run_pass(&tests, &args.path).await {
      tracing::error!(error = %error, "test pass failed; watching for changes");
    }
    if watcher.next_change().await.is_none() {
      anyhow::bail!("watch channel closed");
    }
  }
}

async fn run_pass(tests: &[Test], path: &Path) -> Result<()> {
  if tests.is_empty() {
    tracing::warn!(path = %path.display(), "no tests found");
    return Ok(());
  }
  let mut failures = Vec::new();
  for test in tests {
    match run_one(test).await {
      Ok(()) => println!("PASS {}", test.label()),
      Err(error) => {
        println!("FAIL {}", test.label());
        eprintln!("{error:#}");
        failures.push(test.label().to_owned());
      }
    }
  }
  let passed = tests.len().saturating_sub(failures.len());
  println!("{passed} passed, {} failed", failures.len());
  if failures.is_empty() {
    Ok(())
  } else {
    anyhow::bail!("{} test(s) failed: {}", failures.len(), failures.join(", "))
  }
}

async fn run_one(test: &Test) -> Result<()> {
  let (mut config, assertions) = collect::load(&test.config)?;
  collect::resolve_scripts(&mut config, &test.config);
  let registries = omw::agent::Registries::default();
  let report = omw::testing::Harness::new(&config, &registries, &assertions)
    .run()
    .await;
  if let Some(error) = &report.error {
    anyhow::bail!("run failed: {error}");
  }
  let mut failed = String::new();
  for (name, agent) in &report.agents {
    if agent.passed {
      continue;
    }
    let _ = writeln!(failed, "agent {name:?} failed:");
    if let Some(diff) = &agent.diff {
      for line in diff.lines() {
        let _ = writeln!(failed, "  {line}");
      }
    }
  }
  if failed.is_empty() {
    Ok(())
  } else {
    anyhow::bail!("{}", failed.trim_end())
  }
}

#[cfg(feature = "compile-wasm")]
fn compile_wasm(args: &CompileWasmArgs) -> Result<()> {
  for wasm in crate::wasm::build(args)? {
    println!("{}", wasm.display());
  }
  Ok(())
}
