//! `omw` argument-parsing and exit-code integration tests.

mod common;

use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn help_prints_usage() {
  common::omw()
    .arg("--help")
    .assert()
    .success()
    .stdout(predicate::str::contains("Usage"));
}

#[test]
fn no_subcommand_is_a_usage_error() {
  common::omw()
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains("Usage"));
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
  common::omw().arg("bogus").assert().failure().code(2);
}

#[test]
fn schema_requires_an_output() {
  common::omw().arg("schema").assert().failure().code(2);
}

#[test]
fn invalid_format_value_is_a_usage_error() {
  common::omw()
    .args(["run", "--format", "bogus"])
    .assert()
    .failure()
    .code(2);
}

#[test]
fn invalid_log_format_value_is_a_usage_error() {
  common::omw()
    .args(["run", "--log-format", "bogus"])
    .assert()
    .failure()
    .code(2);
}

#[test]
fn forced_tty_falls_back_to_pipe_when_not_a_terminal() {
  let dir = tempdir().unwrap();
  let config = dir.path().join("omw.toml");
  std::fs::write(&config, "").unwrap();
  let assert = common::omw()
    .args(["run", "--config"])
    .arg(&config)
    .args(["--log-format", "tty"])
    .assert()
    .success();
  let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
  assert!(
    stderr.contains("live view unavailable"),
    "should fall back to pipe, got: {stderr}"
  );
  assert!(
    stderr.contains("\"level\":\"INFO\""),
    "logs should be JSONL after the fallback, got: {stderr}"
  );
}
