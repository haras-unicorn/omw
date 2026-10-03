//! `omw-test` argument-parsing and exit-code integration tests.

mod common;

use predicates::prelude::*;

#[test]
fn help_prints_usage() {
  common::omw_test()
    .arg("--help")
    .assert()
    .success()
    .stdout(predicate::str::contains("Usage"));
}

#[test]
fn no_subcommand_is_a_usage_error() {
  common::omw_test()
    .assert()
    .failure()
    .code(2)
    .stderr(predicate::str::contains("Usage"));
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
  common::omw_test().arg("bogus").assert().failure().code(2);
}

#[test]
fn invalid_format_value_is_a_usage_error() {
  common::omw_test()
    .args(["run", "--format", "bogus"])
    .assert()
    .failure()
    .code(2);
}

#[test]
fn invalid_log_format_value_is_a_usage_error() {
  common::omw_test()
    .args(["run", "--log-format", "bogus"])
    .assert()
    .failure()
    .code(2);
}
