//! `omw-test` environment-overlay integration tests.

mod common;

use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn env_overlay_reaches_the_loader() {
  let dir = tempdir().unwrap();
  common::write_case(dir.path(), "case", "chat");

  common::omw_test()
    .args(["run"])
    .arg(dir.path())
    .env("OMW_TEST__TUNABLES__RECV_TIMEOUT_SECS", "not-a-number")
    .assert()
    .failure()
    .stderr(predicate::str::contains("failed to deserialize"));
}
