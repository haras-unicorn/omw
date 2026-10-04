//! `omw-test` entry point: discover test configs, run each through the
//! traced path, and check its assertions.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::IsTerminal as _;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use omw::config::Format;
use omw::host::trace::TraceEvent;
use omw::runtime::RunOutcome;
use omw::testing::Snapshots;
use omw::watch::{RecursiveMode, Watcher};

#[cfg(feature = "compile-wasm")]
use crate::cli::CompileWasmArgs;
use crate::cli::{Cli, Command, RunArgs};
use crate::collect::{self, Test};

/// Run the `omw-test` binary.
pub async fn run() -> ExitCode {
  let cli = Cli::load();
  let live = crate::log::init(&cli.command, cli.log_format);
  if let Some(live) = &live {
    live.set_view(omw_output::View::Tests);
    live.set_command(omw_output::command_line());
  }
  crate::tls::init();

  tracing::info!(command = ?cli.command, "omw-test starting");

  let RunResult { summary, result } = match cli.command {
    Command::Run { args } => run_tests(args, live.as_ref()).await,
    Command::Schema { output } => RunResult::plain(generate_schema(&output)),
    #[cfg(feature = "compile-wasm")]
    Command::CompileWasm { args } => RunResult::plain(compile_wasm(&args)),
  };

  if let Err(error) = &result {
    omw_output::report_error(error);
  }
  drop(live);

  // Durable tally: when the live view owned stdout, replay the verdict lines
  // and the tally there, so an interactive run leaves its result in scrollback
  // instead of only a flash on the alternate screen.
  if let Some(summary) = summary
    && summary.suppressed
  {
    for (label, passed) in &summary.verdicts {
      println!("{} {label}", if *passed { "PASS" } else { "FAIL" });
    }
    println!("{} passed, {} failed", summary.passed, summary.failed);
  }

  match result {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("omw-test: {error:#}");
      ExitCode::FAILURE
    }
  }
}

/// The result of a whole `run` invocation: an optional summary for the durable
/// tally and the process-level result.
struct RunResult {
  summary: Option<PassSummary>,
  result: Result<()>,
}

impl RunResult {
  fn plain(result: Result<()>) -> Self {
    Self {
      summary: None,
      result,
    }
  }

  fn err(error: anyhow::Error) -> Self {
    Self {
      summary: None,
      result: Err(error),
    }
  }
}

/// Generate the JSON schema for the test configuration and write it to `path`.
fn generate_schema(path: &Path) -> Result<()> {
  let contents = omw::config::Config::schema_json()?;
  crate::stdio::write(path, &contents)?;
  tracing::info!("wrote configuration schema to {}", path.display());
  Ok(())
}

async fn run_tests(
  args: RunArgs,
  live: Option<&omw_output::Live>,
) -> RunResult {
  // The live view is process-wide and starts before any test config is
  // loaded, so only the `OMW_TEST__` env overlay can drive it. Best-effort:
  // an unreadable overlay keeps the crate-local defaults (the watch branch
  // below still fails hard on it).
  let tunables = collect::env_tunables();
  if let Some(live) = live
    && let Ok(tunables) = &tunables
  {
    live.configure(tunables.tui_tick_ms, tunables.tui_tab_capacity);
  }
  if !args.watch {
    let tests = match collect::discover(&args.path)
      .and_then(|tests| collect::filter(tests, &args.include, &args.exclude))
    {
      Ok(tests) => tests,
      Err(error) => return RunResult::err(error),
    };
    let summary = match run_pass(&tests, &args, live).await {
      Ok(summary) => summary,
      Err(error) => return RunResult::err(error),
    };
    let result = if summary.failed == 0 {
      Ok(())
    } else {
      Err(anyhow::anyhow!("{}", summary.failure_summary()))
    };
    return RunResult {
      summary: Some(summary),
      result,
    };
  }
  // Hold one watcher across reruns so a change between passes is not missed.
  let debounce = match tunables {
    Ok(tunables) => tunables.watch_debounce(),
    Err(error) => return RunResult::err(error),
  };
  let mut watcher = match Watcher::watch(
    collect::root(&args.path),
    RecursiveMode::Recursive,
    debounce,
  ) {
    Ok(watcher) => watcher,
    Err(error) => return RunResult::err(error),
  };
  loop {
    let tests = match collect::discover(&args.path)
      .and_then(|tests| collect::filter(tests, &args.include, &args.exclude))
    {
      Ok(tests) => tests,
      Err(error) => {
        tracing::error!(error = %error, "test discovery failed; watching for changes");
        if watcher.next_change().await.is_none() {
          return RunResult::err(anyhow::anyhow!("watch channel closed"));
        }
        continue;
      }
    };
    if let Some(live) = live {
      live.reset();
    }
    match run_pass(&tests, &args, live).await {
      Ok(summary) if summary.failed == 0 => {}
      Ok(summary) => {
        tracing::error!(
          error = %summary.failure_summary(),
          "test pass failed; watching for changes"
        );
      }
      Err(error) => {
        tracing::error!(error = %error, "test pass failed; watching for changes");
      }
    }
    if watcher.next_change().await.is_none() {
      return RunResult::err(anyhow::anyhow!("watch channel closed"));
    }
  }
}

/// The aggregated verdicts of one pass, kept for the durable tally.
struct PassSummary {
  verdicts: Vec<(String, bool)>,
  passed: usize,
  failed: usize,
  /// Whether the live view owned stdout, so the lines were not printed during
  /// the run and must be replayed after it tears down.
  suppressed: bool,
}

impl PassSummary {
  fn failure_summary(&self) -> String {
    let failed: Vec<&str> = self
      .verdicts
      .iter()
      .filter(|(_, passed)| !passed)
      .map(|(label, _)| label.as_str())
      .collect();
    format!("{} test(s) failed: {}", self.failed, failed.join(", "))
  }
}

async fn run_pass(
  tests: &[Test],
  args: &RunArgs,
  live: Option<&omw_output::Live>,
) -> Result<PassSummary> {
  if tests.is_empty() {
    tracing::warn!(path = %args.path.display(), "no tests found");
    if let Some(path) = args.dump.as_deref() {
      let rendered =
        render_dump(args.dump_format, &Dump { tests: Vec::new() })?;
      crate::stdio::write(path, &rendered)?;
    }
    return Ok(PassSummary {
      verdicts: Vec::new(),
      passed: 0,
      failed: 0,
      suppressed: false,
    });
  }
  if let Some(live) = live {
    live.set_tests(tests.iter().map(|test| test.label().to_owned()).collect());
    live.set_details(vec![
      ("path".to_owned(), args.path.display().to_string()),
      ("tests".to_owned(), tests.len().to_string()),
    ]);
    live.set_progress(0, tests.len());
  }
  // Verdicts are the product: stdout when it is not the terminal the live view
  // owns (or when there is no live view at all).
  let to_stdout = live.is_none() || !std::io::stdout().is_terminal();
  let mut verdicts = Vec::new();
  let mut dumps = Vec::new();
  let mut done = 0usize;
  for test in tests {
    if let Some(live) = live {
      live.begin_test(test.label());
    }
    let dump = run_one(test, args.format).await;
    let passed = dump.passed;
    let detail = dump.detail();
    if let Some(live) = live {
      live.verdict(test.label(), passed, detail.clone());
    }
    if to_stdout {
      if passed {
        println!("PASS {}", test.label());
      } else {
        println!("FAIL {}", test.label());
        if let Some(detail) = &detail {
          eprintln!("{detail}");
        }
      }
    }
    verdicts.push((test.label().to_owned(), passed));
    dumps.push(dump);
    done = done.saturating_add(1);
    if let Some(live) = live {
      live.set_progress(done, tests.len());
    }
    if !passed && !args.all {
      break;
    }
  }
  if let Some(path) = args.dump.as_deref() {
    let rendered = render_dump(args.dump_format, &Dump { tests: dumps })?;
    crate::stdio::write(path, &rendered)?;
  }
  let failed = verdicts.iter().filter(|(_, passed)| !passed).count();
  let passed = verdicts.len().saturating_sub(failed);
  if to_stdout {
    println!("{passed} passed, {failed} failed");
  }
  Ok(PassSummary {
    verdicts,
    passed,
    failed,
    suppressed: !to_stdout,
  })
}

/// One test's machine-readable outcome, dumped by `--dump`.
#[derive(serde::Serialize)]
struct TestDump {
  test: String,
  passed: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  error: Option<String>,
  agents: BTreeMap<String, AgentDump>,
  snapshots: Snapshots,
}

/// The machine-readable artifact written by `--dump`: every test that ran.
#[derive(serde::Serialize)]
struct Dump {
  tests: Vec<TestDump>,
}

/// Render the dump in `format`. TOML has no null, so null fields are dropped
/// (they carry no information a diagnostic reader needs).
fn render_dump(format: Format, dump: &Dump) -> Result<String> {
  let mut rendered = match format {
    Format::Json => serde_json::to_string_pretty(dump)?,
    Format::Yaml => serde_norway::to_string(dump)?,
    Format::Toml => {
      let mut value = serde_json::to_value(dump)?;
      strip_nulls(&mut value);
      toml::to_string_pretty(&value)?
    }
  };
  if !rendered.ends_with('\n') {
    rendered.push('\n');
  }
  Ok(rendered)
}

/// Drop null object fields and array elements, recursively, so a JSON-shaped
/// value can be rendered as TOML (which has no null).
fn strip_nulls(value: &mut serde_json::Value) {
  match value {
    serde_json::Value::Object(map) => {
      map.retain(|_, value| !value.is_null());
      for value in map.values_mut() {
        strip_nulls(value);
      }
    }
    serde_json::Value::Array(items) => {
      items.retain(|item| !item.is_null());
      for item in items.iter_mut() {
        strip_nulls(item);
      }
    }
    _ => {}
  }
}

/// One agent's verdict as written to `--dump`.
#[derive(serde::Serialize)]
struct AgentDump {
  passed: bool,
  asserted: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  outcome: Option<RunOutcome>,
  #[serde(skip_serializing_if = "Option::is_none")]
  diff: Option<String>,
  cursor: usize,
  assertions: usize,
  timed_out: bool,
  observed: Vec<TraceEvent>,
}

impl TestDump {
  /// The human-readable failure detail, or `None` when the test passed.
  fn detail(&self) -> Option<String> {
    if let Some(error) = &self.error {
      return Some(format!("run failed: {error}"));
    }
    let mut failed = String::new();
    for (name, agent) in &self.agents {
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
      None
    } else {
      Some(failed.trim_end().to_owned())
    }
  }
}

async fn run_one(test: &Test, format: Option<Format>) -> TestDump {
  let label = test.label().to_owned();
  let (mut config, assertions) =
    match collect::load(&test.config, format, &test.bases) {
      Ok(pair) => pair,
      Err(error) => {
        return TestDump {
          test: label,
          passed: false,
          error: Some(format!("{error:#}")),
          agents: BTreeMap::new(),
          snapshots: Snapshots::default(),
        };
      }
    };
  collect::resolve_scripts(&mut config, &test.config);
  let registries = omw::agent::Registries::default();
  let report = omw::testing::Harness::new(&config, &registries, &assertions)
    .run()
    .await;
  let passed = report.passed();
  let agents = report
    .agents
    .into_iter()
    .map(|(name, agent)| {
      let dump = AgentDump {
        passed: agent.passed,
        asserted: agent.asserted,
        outcome: agent.outcome,
        diff: agent.diff,
        cursor: agent.cursor,
        assertions: agent.assertions,
        timed_out: agent.timed_out,
        observed: agent.observed,
      };
      (name, dump)
    })
    .collect();
  TestDump {
    test: label,
    passed,
    error: report.error,
    agents,
    snapshots: report.snapshots,
  }
}

#[cfg(feature = "compile-wasm")]
fn compile_wasm(args: &CompileWasmArgs) -> Result<()> {
  for wasm in crate::wasm::build(args)? {
    println!("{}", wasm.display());
  }
  Ok(())
}
