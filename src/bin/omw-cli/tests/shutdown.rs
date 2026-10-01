//! `omw` graceful-shutdown integration tests.
//!
//! The binary owns the SIGTERM/SIGINT subscription; these spawn the real
//! process, wait for a parking rhai brain to announce itself on stderr, signal
//! it, and assert it exits 0.
#![cfg(all(unix, feature = "runtime-rhai"))]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use nix::sys::signal::Signal;
use tempfile::tempdir;

/// Subscribe to lifecycle events, announce readiness, then park in `recv` until
/// the supervisor delivers a `shutdown` (or `reload`) event.
const BRAIN: &str = r#"
let lc = omw::host::subscribe_lifecycle();
omw::host::log("info", "omw-ready-marker");
loop {
  let e = omw::host::recv();
  if e.id == lc && e.kind == "shutdown" { break; }
  if e.id == lc && e.kind == "reload" { break; }
}
"#;

fn write_config(dir: &Path) -> PathBuf {
  let brain = dir.join("brain.rhai");
  std::fs::write(&brain, BRAIN).unwrap();
  let config = dir.join("omw.toml");
  std::fs::write(
    &config,
    format!(
      "[runtime.rhai]\nkind = \"rhai\"\n\n[agents.alice]\nruntime = \"rhai\"\nscript = \"{}\"\n",
      brain.display()
    ),
  )
  .unwrap();
  config
}

fn shutdown_gracefully(subcommand: &str, signal: Signal) {
  let dir = tempdir().unwrap();
  let config = write_config(dir.path());
  let log = dir.path().join("stderr.log");

  let mut cmd = common::omw_raw();
  cmd.arg(subcommand).args(["--config"]).arg(&config);
  let mut child = common::spawn_with_log(&mut cmd, &log);

  assert!(
    common::wait_for_log(&log, "omw-ready-marker", Duration::from_secs(60)),
    "the brain never became ready: {}",
    std::fs::read_to_string(&log).unwrap_or_default()
  );

  common::signal(&child, signal);
  let status = common::wait_for_exit(&mut child, Duration::from_secs(30));
  assert!(
    status.success(),
    "expected a graceful exit on {signal:?}, got {status}: {}",
    std::fs::read_to_string(&log).unwrap_or_default()
  );
}

#[test]
fn run_exits_gracefully_on_sigterm() {
  shutdown_gracefully("run", Signal::SIGTERM);
}

#[test]
fn run_exits_gracefully_on_sigint() {
  shutdown_gracefully("run", Signal::SIGINT);
}

#[test]
fn loop_exits_gracefully_on_sigterm() {
  shutdown_gracefully("loop", Signal::SIGTERM);
}
