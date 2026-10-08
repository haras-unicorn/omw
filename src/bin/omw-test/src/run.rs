//! `omw-test` entry point: discover test configs, run each through the
//! traced path, and check its assertions.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use omw::config::Format;
use omw::host::trace::TraceEvent;
use omw::runtime::RunOutcome;
use omw::testing::Snapshots;
use omw::watch::{RecursiveMode, Watcher};
use tokio::task::JoinSet;
use tracing::Instrument as _;

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
    Command::Run { args } => {
      let registries = std::sync::Arc::new(omw::agent::Registries::default());
      run_tests(args, live.as_ref(), &registries).await
    }
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
  registries: &std::sync::Arc<omw::agent::Registries>,
) -> RunResult {
  // The live view is process-wide and starts before any test config is
  // loaded, so only the `OMW_TEST__` env overlay can drive it. Best-effort:
  // an unreadable overlay keeps the crate-local defaults (the watch branch
  // below still fails hard on it).
  let tunables = collect::env_tunables(registries);
  if let Some(live) = live
    && let Ok(tunables) = &tunables
  {
    live.configure(tunables.tui_tick_ms, tunables.tui_tab_capacity);
  }
  if !args.watch {
    let tests = match discover_and_filter(&args) {
      Ok(tests) => tests,
      Err(error) => return RunResult::err(error),
    };
    if let Some(live) = live {
      live
        .set_tests(tests.iter().map(|test| test.label().to_owned()).collect());
      set_pass_details(
        live,
        &args.path,
        tests.len(),
        tests.len(),
        resolve_jobs(args.jobs),
      );
    }
    let summary = match run_pass(&tests, &args, live, registries).await {
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

  // The first pass runs everything discovered.
  let mut tests = match discover_and_filter(&args) {
    Ok(tests) => tests,
    Err(error) => return RunResult::err(error),
  };
  if let Some(live) = live {
    live.reset();
    live.set_tests(tests.iter().map(|test| test.label().to_owned()).collect());
    set_pass_details(
      live,
      &args.path,
      tests.len(),
      tests.len(),
      resolve_jobs(args.jobs),
    );
  }
  report_pass(run_pass(&tests, &args, live, registries).await);
  let mut known: HashSet<PathBuf> = tests
    .iter()
    .map(|test| collect::canonical_path(&test.config))
    .collect();

  loop {
    let changed = match watcher.next_change().await {
      Some(changed) => changed,
      None => return RunResult::err(anyhow::anyhow!("watch channel closed")),
    };
    // Re-discover so new (and removed) test configs are picked up.
    tests = match discover_and_filter(&args) {
      Ok(tests) => tests,
      Err(error) => {
        tracing::error!(error = %error, "test discovery failed; watching for changes");
        continue;
      }
    };
    let affected = affected_tests(&tests, &changed, &known, args.format);
    known = tests
      .iter()
      .map(|test| collect::canonical_path(&test.config))
      .collect();
    if affected.is_empty() {
      tracing::info!(changed = ?changed, "no tests affected; watching for changes");
      continue;
    }
    if let Some(live) = live {
      // Keep every discovered test listed; only the affected ones change state.
      live
        .sync_tests(tests.iter().map(|test| test.label().to_owned()).collect());
      live.begin_pass();
      set_pass_details(
        live,
        &args.path,
        tests.len(),
        affected.len(),
        resolve_jobs(args.jobs),
      );
    }
    tracing::info!(
      affected = affected.len(),
      total = tests.len(),
      "re-running affected tests"
    );
    report_pass(run_pass(&affected, &args, live, registries).await);
  }
}

/// Discover and filter the tests under `args.path`.
fn discover_and_filter(args: &RunArgs) -> Result<Vec<Test>> {
  collect::discover(&args.path)
    .and_then(|tests| collect::filter(tests, &args.include, &args.exclude))
}

/// The tests a change batch should re-run: any test whose config, base configs
/// or resolved brain scripts include a changed path, plus any newly discovered
/// test (one whose config was not in `known`).
fn affected_tests(
  tests: &[Test],
  changed: &[PathBuf],
  known: &HashSet<PathBuf>,
  format: Option<Format>,
) -> Vec<Test> {
  let changed: Vec<PathBuf> = changed
    .iter()
    .map(|path| collect::canonical_path(path))
    .collect();
  tests
    .iter()
    .filter(|test| {
      if !known.contains(&collect::canonical_path(&test.config)) {
        return true;
      }
      let watched = collect::watched_paths(test, format);
      changed.iter().any(|path| watched.contains(path))
    })
    .cloned()
    .collect()
}

/// Update the live view's info panel for a pass: the discovery path, the total
/// discovered and (when a subset re-runs) the affected count, and the jobs.
fn set_pass_details(
  live: &omw_output::Live,
  path: &Path,
  total: usize,
  affected: usize,
  jobs: usize,
) {
  let mut details = vec![
    ("path".to_owned(), path.display().to_string()),
    ("tests".to_owned(), total.to_string()),
    ("jobs".to_owned(), jobs.to_string()),
  ];
  if affected != total {
    details.push(("affected".to_owned(), affected.to_string()));
  }
  live.set_details(details);
}

/// Log a watch pass's result; failures are not fatal while watching.
fn report_pass(result: Result<PassSummary>) {
  match result {
    Ok(summary) if summary.failed == 0 => {}
    Ok(summary) => tracing::error!(
      error = %summary.failure_summary(),
      "test pass failed; watching for changes"
    ),
    Err(error) => {
      tracing::error!(error = %error, "test pass failed; watching for changes");
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
  registries: &std::sync::Arc<omw::agent::Registries>,
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
  let jobs = resolve_jobs(args.jobs);
  if let Some(live) = live {
    live.set_progress(0, tests.len());
  }
  // Verdicts are the product: stdout when it is not the terminal the live view
  // owns (or when there is no live view at all).
  let to_stdout = live.is_none() || !std::io::stdout().is_terminal();
  let mut dumps: Vec<Option<TestDump>> =
    std::iter::repeat_with(|| None).take(tests.len()).collect();
  let mut next = 0usize;
  let mut in_flight = 0usize;
  let mut done = 0usize;
  let mut stop_launching = false;
  let mut set: JoinSet<(usize, TestDump)> = JoinSet::new();

  // Fill the initial window. Each test runs instrumented with its label, so a
  // concurrent run still routes its logs to the right test in the live view.
  let spawn_ready = |set: &mut JoinSet<(usize, TestDump)>,
                     next: &mut usize,
                     in_flight: &mut usize,
                     stop_launching: bool| {
    while !stop_launching && *next < tests.len() && *in_flight < jobs {
      let index = *next;
      let test = tests[index].clone();
      let format = args.format;
      let label = test.label().to_owned();
      let registries = std::sync::Arc::clone(registries);
      if let Some(live) = live {
        live.begin_test(label.clone());
      }
      set.spawn(async move {
        let dump = run_one(&test, format, &registries)
          .instrument(tracing::info_span!("test", test = %label))
          .await;
        (index, dump)
      });
      *next = next.saturating_add(1);
      *in_flight = in_flight.saturating_add(1);
    }
  };
  spawn_ready(&mut set, &mut next, &mut in_flight, stop_launching);

  while let Some(joined) = set.join_next().await {
    let (index, dump) = joined.map_err(|error| {
      anyhow::anyhow!("a test task failed to join: {error}")
    })?;
    in_flight = in_flight.saturating_sub(1);
    let label = tests[index].label().to_owned();
    let passed = dump.passed;
    let detail = dump.detail();
    if let Some(live) = live {
      live.verdict(label.clone(), passed, detail.clone());
    }
    if to_stdout {
      if passed {
        println!("PASS {label}");
      } else {
        println!("FAIL {label}");
        if let Some(detail) = &detail {
          eprintln!("{detail}");
        }
      }
    }
    done = done.saturating_add(1);
    if let Some(live) = live {
      live.set_progress(done, tests.len());
    }
    dumps[index] = Some(dump);
    // Fail-fast: a failure stops further launches, but already-running tests
    // finish and report (with `-j 1` this is exactly the sequential behavior).
    if !passed && !args.all {
      stop_launching = true;
    }
    spawn_ready(&mut set, &mut next, &mut in_flight, stop_launching);
  }

  let verdicts: Vec<(String, bool)> = dumps
    .iter()
    .flatten()
    .map(|dump| (dump.test.clone(), dump.passed))
    .collect();
  if let Some(path) = args.dump.as_deref() {
    let dumps: Vec<TestDump> = dumps.into_iter().flatten().collect();
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

/// How many tests to run at once: an explicit positive count, else the
/// machine's logical core count (`-j 0` and an omitted flag both mean auto).
/// Falls back to one if the parallelism cannot be determined.
fn resolve_jobs(jobs: Option<usize>) -> usize {
  match jobs {
    Some(jobs) if jobs > 0 => jobs,
    _ => std::thread::available_parallelism()
      .map(|parallelism| parallelism.get())
      .unwrap_or(1),
  }
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

async fn run_one(
  test: &Test,
  format: Option<Format>,
  registries: &std::sync::Arc<omw::agent::Registries>,
) -> TestDump {
  let label = test.label().to_owned();
  let (mut config, assertions) =
    match collect::load(&test.config, format, &test.bases, registries) {
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
  let report = omw::testing::Harness::new(&config, registries, &assertions)
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
  let wasms = omw_build::build_brains(
    &args.path,
    args.sdk.as_deref(),
    args.cargo.as_deref(),
    args.wasm_tools.as_deref(),
    &args.args,
  )?;
  for wasm in wasms {
    println!("{}", wasm.display());
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use std::collections::HashSet;
  use std::path::{Path, PathBuf};

  use super::{affected_tests, resolve_jobs};
  use crate::collect::{self, Test};

  #[test]
  fn an_explicit_positive_jobs_count_is_used_verbatim() {
    assert_eq!(resolve_jobs(Some(1)), 1);
    assert_eq!(resolve_jobs(Some(4)), 4);
  }

  #[test]
  fn an_omitted_or_zero_jobs_count_is_auto() {
    let auto = std::thread::available_parallelism()
      .map(|parallelism| parallelism.get())
      .unwrap_or(1);
    assert_eq!(resolve_jobs(None), auto);
    assert_eq!(resolve_jobs(Some(0)), auto);
  }

  fn write(dir: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent).expect("failed to create the directory");
    }
    std::fs::write(&path, contents).expect("failed to write the file");
    path
  }

  /// A minimal test config whose single agent runs `script`.
  fn case(dir: &Path, rel: &str, script: &str) -> PathBuf {
    write(
      dir,
      rel,
      &format!(
        "[runtime.runtime]\nkind = \"rhai\"\n\n[agents.alice]\nruntime = \"runtime\"\nscript = \"{script}\"\n"
      ),
    )
  }

  fn known(tests: &[Test]) -> HashSet<PathBuf> {
    tests
      .iter()
      .map(|test| collect::canonical_path(&test.config))
      .collect()
  }

  fn labels(tests: &[Test]) -> Vec<String> {
    tests.iter().map(|test| test.relative.clone()).collect()
  }

  #[test]
  fn a_brain_change_affects_only_the_test_running_it() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    case(dir.path(), "a/omw.test.toml", "brain.rhai");
    case(dir.path(), "b/omw.test.toml", "brain.rhai");
    let tests = collect::discover(dir.path())?;
    let changed = vec![dir.path().join("a/brain.rhai")];

    let affected = affected_tests(&tests, &changed, &known(&tests), None);
    assert_eq!(labels(&affected), vec!["a/omw.test.toml"]);
    Ok(())
  }

  #[test]
  fn an_unrelated_change_affects_nothing() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    case(dir.path(), "a/omw.test.toml", "brain.rhai");
    case(dir.path(), "b/omw.test.toml", "brain.rhai");
    let tests = collect::discover(dir.path())?;
    let changed = vec![dir.path().join("README.md")];

    let affected = affected_tests(&tests, &changed, &known(&tests), None);
    assert!(affected.is_empty());
    Ok(())
  }

  #[test]
  fn a_base_config_change_affects_every_test_inheriting_it()
  -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let base = write(
      dir.path(),
      "omw.test.base.toml",
      "[agents.bob]\nruntime = \"runtime\"\nscript = \"shared/brain.rhai\"\n",
    );
    case(dir.path(), "a/omw.test.toml", "brain.rhai");
    case(dir.path(), "b/omw.test.toml", "brain.rhai");
    let tests = collect::discover(dir.path())?;

    let by_base = affected_tests(&tests, &[base.clone()], &known(&tests), None);
    assert_eq!(labels(&by_base), vec!["a/omw.test.toml", "b/omw.test.toml"]);

    let by_shared_script = affected_tests(
      &tests,
      &[dir.path().join("shared/brain.rhai")],
      &known(&tests),
      None,
    );
    assert_eq!(
      labels(&by_shared_script),
      vec!["a/omw.test.toml", "b/omw.test.toml"]
    );
    Ok(())
  }

  #[test]
  fn a_newly_discovered_test_is_affected() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    case(dir.path(), "a/omw.test.toml", "brain.rhai");
    case(dir.path(), "b/omw.test.toml", "brain.rhai");
    let tests = collect::discover(dir.path())?;

    let affected = affected_tests(
      &tests,
      &[dir.path().join("unrelated")],
      &HashSet::new(),
      None,
    );
    assert_eq!(
      labels(&affected),
      vec!["a/omw.test.toml", "b/omw.test.toml"]
    );
    Ok(())
  }
}
