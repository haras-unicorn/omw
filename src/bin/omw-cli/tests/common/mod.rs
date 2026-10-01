//! Shared helpers for the `omw` integration tests.
//!
//! The synchronous cases use [`omw`] (an `assert_cmd::Command`); the shutdown
//! cases need a raw process so they can watch stderr and send a signal.
#![allow(dead_code, reason = "each integration test binary uses a subset")]

use assert_cmd::Command;

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Child, Command as StdCommand, ExitStatus, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;

/// The `omw` binary under test, for `assert_cmd`-style assertions.
pub fn omw() -> Command {
  Command::new(env!("CARGO_BIN_EXE_omw"))
}

/// The `omw` binary under test, as a raw process command.
#[cfg(unix)]
pub fn omw_raw() -> StdCommand {
  StdCommand::new(env!("CARGO_BIN_EXE_omw"))
}

/// Spawn `cmd` with stdout and stderr redirected to `log`, stdin null.
#[cfg(unix)]
pub fn spawn_with_log(cmd: &mut StdCommand, log: &Path) -> Child {
  let file = File::create(log).expect("failed to create the log file");
  cmd
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::from(file))
    .spawn()
    .expect("failed to spawn the omw binary")
}

/// Wait until `log` contains `marker`, up to `timeout`.
#[cfg(unix)]
pub fn wait_for_log(log: &Path, marker: &str, timeout: Duration) -> bool {
  let start = Instant::now();
  loop {
    if std::fs::read_to_string(log)
      .is_ok_and(|contents| contents.contains(marker))
    {
      return true;
    }
    if start.elapsed() >= timeout {
      return false;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
}

/// Send `signal` to `child`.
#[cfg(unix)]
pub fn signal(child: &Child, signal: Signal) {
  let pid = i32::try_from(child.id()).expect("child pid fits in i32");
  kill(Pid::from_raw(pid), signal).expect("failed to signal the omw binary");
}

/// Wait for `child` to exit, killing it if it outlives `timeout`.
#[cfg(unix)]
pub fn wait_for_exit(child: &mut Child, timeout: Duration) -> ExitStatus {
  let start = Instant::now();
  loop {
    if let Some(status) = child.try_wait().expect("failed to poll the child") {
      return status;
    }
    if start.elapsed() >= timeout {
      let _ = child.kill();
      let _ = child.wait();
      panic!("the omw binary did not exit within {timeout:?}");
    }
    std::thread::sleep(Duration::from_millis(50));
  }
}
