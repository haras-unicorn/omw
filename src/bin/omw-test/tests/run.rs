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
