//! Shared helpers for the `omw-test` integration tests.
#![allow(dead_code, reason = "each integration test binary uses a subset")]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::process::{Child, Command as StdCommand, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

/// The `omw-test` binary under test.
///
/// `JOURNAL_STREAM` and `RUST_LOG` are cleared so `auto` logging settles on
/// `pipe` and the default level, keeping stderr assertions deterministic even
/// when the test host itself is a systemd service.
pub fn omw_test() -> Command {
  let mut command = Command::new(env!("CARGO_BIN_EXE_omw-test"));
  command.env_remove("JOURNAL_STREAM").env_remove("RUST_LOG");
  command
}

/// The `omw-test` binary under test, as a raw process command.
#[cfg(unix)]
pub fn omw_test_raw() -> StdCommand {
  let mut command = StdCommand::new(env!("CARGO_BIN_EXE_omw-test"));
  command.env_remove("JOURNAL_STREAM").env_remove("RUST_LOG");
  command
}

/// Spawn `cmd` with stdout and stderr both appended to `log`, stdin null. The
/// long-running watch cases need a raw process they can poll and kill.
#[cfg(unix)]
pub fn spawn_capturing(cmd: &mut StdCommand, log: &Path) -> Child {
  let file = File::create(log).expect("failed to create the log file");
  let stdout = file.try_clone().expect("failed to clone the log file");
  cmd
    .stdin(Stdio::null())
    .stdout(Stdio::from(stdout))
    .stderr(Stdio::from(file))
    .spawn()
    .expect("failed to spawn the omw-test binary")
}

/// Wait until `marker` appears at least `count` times in `log`, up to `timeout`.
#[cfg(unix)]
pub fn wait_for_count(
  log: &Path,
  marker: &str,
  count: usize,
  timeout: Duration,
) -> bool {
  let start = Instant::now();
  loop {
    let occurrences = std::fs::read_to_string(log)
      .map(|contents| contents.matches(marker).count())
      .unwrap_or(0);
    if occurrences >= count {
      return true;
    }
    if start.elapsed() >= timeout {
      return false;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
}

/// The 01-hello brain: one blocking chat against the mock provider.
const BRAIN: &str = r#"
let provider = omw::provider::get("openai");
let messages = [#{ role: "user", content: "say hi" }];
let reply = provider.chat("gpt-test", messages, []);
omw::host::log("info", reply.content);
"#;

/// A test config whose single assertion is a `call` with `op`.
const CONFIG: &str = r#"
[providers.openai]
kind = "mock"
turns = [{ content = "hello, world" }]

[runtime.runtime]
kind = "rhai"

[agents.alice]
runtime = "runtime"
script = "brain.rhai"

[assertions.alice]
outcome = "completed"
events = [
  { kind = "call", op = "__OP__" },
]
"#;

/// Write a case under `dir/name`: a brain next to an `omw.test.toml` whose
/// assertion expects a `call` with `op`. Returns the config path.
pub fn write_case(dir: &Path, name: &str, op: &str) -> PathBuf {
  let case = dir.join(name);
  std::fs::create_dir_all(&case).expect("failed to create the case directory");
  std::fs::write(case.join("brain.rhai"), BRAIN)
    .expect("failed to write the brain");
  let config = case.join("omw.test.toml");
  std::fs::write(&config, CONFIG.replace("__OP__", op))
    .expect("failed to write the config");
  config
}
