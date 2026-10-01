//! `omw` environment-overlay integration tests: the `OMW__` prefix must reach
//! the loader of the real process.

mod common;

use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn env_overlay_reaches_the_loader() {
  let dir = tempdir().unwrap();
  let config = dir.path().join("omw.toml");
  std::fs::write(&config, "").unwrap();

  // A valid env-only override keeps the run successful.
  common::omw()
    .args(["run", "--config"])
    .arg(&config)
    .env("OMW__TUNABLES__INBOX_BOUND", "2048")
    .assert()
    .success();

  // An invalid env-only value fails the deserialize, proving the env is read.
  common::omw()
    .args(["run", "--config"])
    .arg(&config)
    .env("OMW__TUNABLES__RECV_TIMEOUT_SECS", "not-a-number")
    .assert()
    .failure()
    .stderr(predicate::str::contains("failed to deserialize"));
}
