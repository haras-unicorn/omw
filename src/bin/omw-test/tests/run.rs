//! `omw-test run` semantics: PASS/FAIL reporting, exit codes, discovery,
//! filtering, and a config read from stdin.
#![cfg(feature = "runtime-rhai")]

mod common;

use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn passing_case_reports_pass_and_exits_zero() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "case", "chat");
  common::omw_test()
    .args(["run"])
    .arg(dir.path())
    .assert()
    .success()
    .stdout(predicate::str::contains("PASS"));
}

#[test]
fn verdicts_go_to_stdout_and_logs_to_stderr() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "case", "chat");
  let assert = common::omw_test()
    .args(["run"])
    .arg(dir.path())
    .assert()
    .success();
  let output = assert.get_output();
  let stdout = String::from_utf8(output.stdout.clone()).unwrap();
  let stderr = String::from_utf8(output.stderr.clone()).unwrap();
  assert!(stdout.contains("PASS"), "got: {stdout}");
  assert!(stdout.contains("passed"), "got: {stdout}");
  assert!(
    stderr.contains("\"level\":\"INFO\""),
    "logs should be JSONL on stderr, got: {stderr}"
  );
}

#[test]
fn failing_assertion_reports_fail_and_exits_nonzero() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "case", "nope");
  common::omw_test()
    .args(["run"])
    .arg(dir.path())
    .assert()
    .failure()
    .stdout(predicate::str::contains("FAIL"))
    .stderr(predicate::str::contains("failed"));
}

#[test]
fn missing_path_errors() {
  let dir = tempdir().unwrap();
  let missing = dir.path().join("nope");
  common::omw_test()
    .args(["run"])
    .arg(&missing)
    .assert()
    .failure()
    .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn empty_root_warns_but_succeeds() {
  let dir = tempdir().unwrap();
  common::omw_test()
    .args(["run"])
    .arg(dir.path())
    .assert()
    .success()
    .stderr(predicate::str::contains("no tests found"));
}

#[test]
fn include_selects_a_subset() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "a", "chat");
  common::write_case(dir.path(), "b", "chat");
  common::omw_test()
    .args(["run"])
    .arg(dir.path())
    .args(["--include", "a/**"])
    .assert()
    .success()
    .stdout(predicate::str::contains("a/omw.test.toml"))
    .stdout(predicate::str::contains("b/omw.test.toml").not());
}

#[test]
fn run_reads_a_config_from_stdin() {
  let dir = tempdir().unwrap();
  let config = common::write_case(dir.path(), "case", "chat");
  let contents = std::fs::read_to_string(&config).unwrap();

  common::omw_test()
    .args(["run", "-", "--format", "toml"])
    .current_dir(dir.path().join("case"))
    .write_stdin(contents)
    .assert()
    .success()
    .stdout(predicate::str::contains("PASS"));
}

#[test]
fn base_config_layers_under_discovered_tests() {
  let dir = tempdir().unwrap();
  let config = common::write_case(dir.path(), "case", "chat");
  // Shrink the test config to the variant-local wiring: the runtime kind and
  // the agent's script. Everything else comes from the base config.
  std::fs::write(
    &config,
    r#"
[runtime.runtime]
kind = "rhai"

[agents.alice]
script = "brain.rhai"
"#,
  )
  .unwrap();
  // The base config sits at the discovery root, so it reaches the case below.
  let base = dir.path().join("omw.test.base.toml");
  std::fs::write(
    &base,
    r#"
[providers.openai]
kind = "mock"
turns = [{ content = "hello, world" }]

[agents.alice]
runtime = "runtime"

[assertions.alice]
outcome = "completed"
events = [{ kind = "call", op = "chat" }]
"#,
  )
  .unwrap();

  common::omw_test()
    .args(["run"])
    .arg(dir.path())
    .assert()
    .success()
    .stdout(predicate::str::contains("PASS"));
}

#[test]
fn forced_tty_falls_back_to_pipe_when_stderr_is_not_a_terminal() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "case", "chat");
  let assert = common::omw_test()
    .args(["run", "--log-format", "tty"])
    .arg(dir.path())
    .assert()
    .success();
  let output = assert.get_output();
  let stderr = String::from_utf8(output.stderr.clone()).unwrap();
  assert!(
    stderr.contains("live view unavailable"),
    "should fall back to pipe, got: {stderr}"
  );
  assert!(
    stderr.contains("\"level\":\"INFO\""),
    "logs should be JSONL after the fallback, got: {stderr}"
  );
  let stdout = String::from_utf8(output.stdout.clone()).unwrap();
  assert!(stdout.contains("PASS"), "got: {stdout}");
}

#[test]
fn fail_fast_stops_at_the_first_failure_while_all_runs_every_test() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "a", "nope");
  common::write_case(dir.path(), "b", "nope");

  // `-j 1` is the sequential path: the first failure stops the pass.
  let assert = common::omw_test()
    .args(["run", "-j", "1"])
    .arg(dir.path())
    .assert()
    .failure();
  let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
  assert_eq!(
    stdout.matches("FAIL").count(),
    1,
    "fail-fast should run only the first test, got: {stdout}"
  );

  let assert = common::omw_test()
    .args(["run", "--all"])
    .arg(dir.path())
    .assert()
    .failure();
  let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
  assert_eq!(
    stdout.matches("FAIL").count(),
    2,
    "--all should run both tests, got: {stdout}"
  );
}

#[test]
fn jobs_flag_runs_tests_in_parallel() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "a", "chat");
  common::write_case(dir.path(), "b", "chat");
  common::write_case(dir.path(), "c", "chat");
  let assert = common::omw_test()
    .args(["run", "--all", "--jobs", "3"])
    .arg(dir.path())
    .assert()
    .success();
  let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
  assert_eq!(
    stdout.matches("PASS").count(),
    3,
    "every test should run, got: {stdout}"
  );
  assert!(
    stdout.contains("3 passed, 0 failed"),
    "tally should cover every test, got: {stdout}"
  );
}

#[test]
fn jobs_zero_is_auto_and_runs() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "a", "chat");
  common::omw_test()
    .args(["run", "-j", "0"])
    .arg(dir.path())
    .assert()
    .success()
    .stdout(predicate::str::contains("PASS"));
}

#[test]
fn dump_writes_per_test_traces_and_mock_snapshots() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "case", "chat");
  let dump = dir.path().join("dump.json");
  common::omw_test()
    .args(["run", "--dump"])
    .arg(&dump)
    .arg(dir.path())
    .assert()
    .success();
  let json = std::fs::read_to_string(&dump).unwrap();
  assert!(json.contains("\"tests\""), "{json}");
  assert!(json.contains("\"test\": \"case/omw.test.toml\""), "{json}");
  assert!(json.contains("\"passed\": true"), "{json}");
  assert!(json.contains("\"observed\""), "{json}");
  assert!(json.contains("\"turns_total\": 1"), "{json}");
  assert!(json.contains("\"turns_consumed\": 1"), "{json}");
  assert!(json.contains("\"deltas\""), "{json}");
  assert!(json.contains("hello, world"), "{json}");
}

#[test]
fn dump_writes_an_empty_artifact_when_no_tests_are_found() {
  let dir = tempdir().unwrap();
  let dump = dir.path().join("dump.json");
  common::omw_test()
    .args(["run", "--dump"])
    .arg(&dump)
    .arg(dir.path())
    .assert()
    .success();
  let json = std::fs::read_to_string(&dump).unwrap();
  assert!(json.contains("\"tests\": []"), "{json}");
}

#[test]
fn dump_format_yaml_and_toml_are_honored() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "case", "chat");

  let yaml = dir.path().join("dump.yaml");
  common::omw_test()
    .args(["run", "--dump"])
    .arg(&yaml)
    .args(["--dump-format", "yaml"])
    .arg(dir.path())
    .assert()
    .success();
  let text = std::fs::read_to_string(&yaml).unwrap();
  assert!(text.contains("tests:"), "{text}");
  assert!(text.contains("case/omw.test.toml"), "{text}");

  let toml = dir.path().join("dump.toml");
  common::omw_test()
    .args(["run", "--dump"])
    .arg(&toml)
    .args(["--dump-format", "toml"])
    .arg(dir.path())
    .assert()
    .success();
  let text = std::fs::read_to_string(&toml).unwrap();
  assert!(text.contains("[[tests]]"), "{text}");
  assert!(text.contains("case/omw.test.toml"), "{text}");
}

/// A `--watch` pass re-runs only the tests a change affects: editing one
/// test's brain re-runs that test and leaves its sibling's last verdict alone.
#[cfg(unix)]
#[test]
fn watch_reruns_only_the_affected_tests() {
  use std::time::Duration;

  let root = tempdir().unwrap();
  common::write_case(root.path(), "a", "chat");
  common::write_case(root.path(), "b", "chat");
  // The log lives outside the watched root, or its own writes would look like
  // changes and trigger endless re-runs.
  let logs = tempdir().unwrap();
  let log = logs.path().join("watch.log");

  let mut cmd = common::omw_test_raw();
  cmd
    .args(["run", "--watch", "-j", "1"])
    .arg(root.path())
    .env("OMW_TEST__TUNABLES__WATCH_DEBOUNCE_MS", "50");
  let mut child = common::spawn_capturing(&mut cmd, &log);

  assert!(
    common::wait_for_count(
      &log,
      "PASS b/omw.test.toml",
      1,
      Duration::from_secs(60)
    ),
    "the first pass should run both tests: {}",
    std::fs::read_to_string(&log).unwrap_or_default()
  );
  assert!(
    common::wait_for_count(
      &log,
      "2 passed, 0 failed",
      1,
      Duration::from_secs(60)
    ),
    "the first pass should tally both tests: {}",
    std::fs::read_to_string(&log).unwrap_or_default()
  );

  // Touch only `a`'s brain. The change keeps the same call, so `a` still
  // passes; the point is which tests re-ran.
  let brain = root.path().join("a/brain.rhai");
  let mut contents = std::fs::read_to_string(&brain).unwrap();
  contents.push_str("\n// touched\n");
  std::fs::write(&brain, contents).unwrap();

  assert!(
    common::wait_for_count(
      &log,
      "1 passed, 0 failed",
      1,
      Duration::from_secs(60)
    ),
    "the second pass should run only the affected test: {}",
    std::fs::read_to_string(&log).unwrap_or_default()
  );

  let contents = std::fs::read_to_string(&log).unwrap();
  assert_eq!(
    contents.matches("PASS a/omw.test.toml").count(),
    2,
    "`a` should have run in both passes: {contents}"
  );
  assert_eq!(
    contents.matches("PASS b/omw.test.toml").count(),
    1,
    "`b` should not have re-run: {contents}"
  );

  let _ = child.kill();
  let _ = child.wait();
}
