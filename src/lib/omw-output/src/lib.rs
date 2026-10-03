//! Shared output policy for the OMW binaries.
//!
//! # Not a library
//!
//! This crate is an implementation detail of the OMW binaries. It is published
//! only so `omw-cli` and `omw-test` can depend on it from the registry; it is
//! **not** a supported library API and may change without notice. Use the `omw`
//! crate instead.
//!
//! Every byte the binaries emit belongs to exactly one of three channels: the
//! command's product on stdout, diagnostics from `tracing` on stderr/journald,
//! and an interactive live view on a terminal. This crate owns that policy so
//! `omw-cli` and `omw-test` do not duplicate it.

#![deny(unsafe_code)]
#![deny(
  clippy::unwrap_used,
  clippy::expect_used,
  clippy::panic,
  clippy::unreachable
)]
#![deny(clippy::arithmetic_side_effects)]
#![deny(clippy::todo)]
#![deny(clippy::allow_attributes_without_reason)]

use std::io::IsTerminal as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::layer::TuiLayer;
use crate::live::{State, Wake};

mod layer;
mod live;
mod tui;

/// Re-export so the binaries depend on one crate for the subscriber stack.
pub use tracing_subscriber;

pub use live::{Live, View};

/// How logs are rendered. `Auto` guesses; every other value is explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum LogFormat {
  /// Guess from the environment (see [`LogFormat::resolve`]).
  #[default]
  Auto,
  /// A human-facing terminal view.
  Tty,
  /// JSONL to stderr.
  Pipe,
  /// Native journald fields.
  Journald,
}

/// The concrete format [`LogFormat::resolve`] settles on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
  /// Interactive terminal.
  Tty,
  /// JSONL to stderr.
  Pipe,
  /// Native journald fields.
  Journald,
}

impl LogFormat {
  /// Settle on a concrete format.
  ///
  /// `Auto` picks `tty` when both stdout and stderr are terminals, `journald`
  /// when systemd has connected stdio to the journal (`JOURNAL_STREAM` is
  /// set), and `pipe` otherwise. The terminal check comes first because a
  /// terminal server (or any systemd-managed session) can leak
  /// `JOURNAL_STREAM` into a process whose stderr is a real TTY.
  pub fn resolve(self) -> Resolved {
    match self {
      Self::Auto => detect(),
      Self::Tty => Resolved::Tty,
      Self::Pipe => Resolved::Pipe,
      Self::Journald => Resolved::Journald,
    }
  }
}

/// Guess the format for [`LogFormat::Auto`].
fn detect() -> Resolved {
  if std::io::stdout().is_terminal() && std::io::stderr().is_terminal() {
    Resolved::Tty
  } else if std::env::var_os("JOURNAL_STREAM").is_some() {
    Resolved::Journald
  } else {
    Resolved::Pipe
  }
}

/// The launched command line: the full `argv[0]` path plus the remaining
/// arguments joined with spaces (the same string `$0` would show), so the live
/// view can display exactly what was launched.
pub fn command_line() -> String {
  std::env::args().collect::<Vec<_>>().join(" ")
}

/// The format for a command that produces data on stdout and never shows the
/// interactive view: a resolved `tty` (or an `auto` that would pick it)
/// becomes `pipe`, while `journald` is preserved.
pub fn producer_format(format: LogFormat) -> LogFormat {
  match format.resolve() {
    Resolved::Tty | Resolved::Pipe => LogFormat::Pipe,
    Resolved::Journald => LogFormat::Journald,
  }
}

/// Install the process-wide `tracing` subscriber for `format`.
///
/// The level comes from `RUST_LOG` (through an [`EnvFilter`]) when it is set,
/// and from `default_level` otherwise.
///
/// For the `tty` format this starts the live view and returns its [`Live`]
/// handle; the other formats return `None`.
pub fn init_logging(format: LogFormat, default_level: &str) -> Option<Live> {
  let resolved = format.resolve();
  let live = match resolved {
    Resolved::Tty => init_tty(default_level),
    Resolved::Pipe => {
      init_pipe(default_level);
      None
    }
    Resolved::Journald => {
      init_journald(default_level);
      None
    }
  };
  tracing::debug!(?resolved, "resolved log format");
  live
}

/// The [`EnvFilter`] for `default_level`: `RUST_LOG` when set, else the level.
fn env_filter(default_level: &str) -> EnvFilter {
  EnvFilter::try_from_default_env()
    .unwrap_or_else(|_| EnvFilter::new(default_level))
}

/// JSONL diagnostics on stderr.
fn init_pipe(default_level: &str) {
  let layer = tracing_subscriber::fmt::layer()
    .with_writer(std::io::stderr)
    .json()
    .flatten_event(true)
    .with_current_span(false);
  tracing_subscriber::registry()
    .with(env_filter(default_level))
    .with(layer)
    .init();
}

/// The interactive live view, driven on stderr so a redirected stdout keeps
/// carrying data. Falls back to [`init_pipe`] when the terminal cannot be
/// driven (for example, stderr is not a terminal), returning `None`.
fn init_tty(default_level: &str) -> Option<Live> {
  // The view is rendered on stderr, so it can only work when stderr is a
  // terminal. Checking here (rather than relying on `enable_raw_mode` failing)
  // keeps the fallback deterministic: raw mode resolves the controlling
  // terminal, which may exist even when stderr is a pipe.
  if !std::io::stderr().is_terminal() {
    eprintln!(
      "omw: live view unavailable (stderr is not a terminal); \
       logging as pipe instead"
    );
    init_pipe(default_level);
    return None;
  }
  let color = std::env::var_os("NO_COLOR").is_none();
  let state = Arc::new(Mutex::new(State::new(color)));
  let (wake_tx, wake_rx) = mpsc::channel();
  let (quit_tx, quit_rx) = mpsc::channel();
  let stop = Arc::new(AtomicBool::new(false));
  let wake = Wake::new(wake_tx);

  match tui::start(Arc::clone(&state), wake_rx, quit_tx, Arc::clone(&stop)) {
    Ok(handle) => {
      let layer = TuiLayer::new(Arc::clone(&state), wake.clone());
      tracing_subscriber::registry()
        .with(env_filter(default_level))
        .with(layer)
        .init();
      Some(Live::new(state, wake, quit_rx, stop, handle))
    }
    Err(error) => {
      eprintln!(
        "omw: live view unavailable ({error}); logging as pipe instead"
      );
      init_pipe(default_level);
      None
    }
  }
}

/// Native journald fields, falling back to [`init_pipe`] when the journald
/// socket is unreachable.
fn init_journald(default_level: &str) {
  match tracing_journald::layer() {
    Ok(layer) => {
      tracing_subscriber::registry()
        .with(env_filter(default_level))
        .with(layer)
        .init();
    }
    Err(error) => {
      eprintln!(
        "omw: journald is unavailable ({error}); logging as pipe instead"
      );
      init_pipe(default_level);
    }
  }
}

/// Report `error` exactly once through the active subscriber.
///
/// The binaries call this from `main` instead of returning the `Err`, so Rust
/// never prints a second `Error: ...` line.
pub fn report_error(error: &anyhow::Error) {
  static REPORTED: AtomicBool = AtomicBool::new(false);
  if REPORTED.swap(true, Ordering::SeqCst) {
    return;
  }
  tracing::error!(error = %format!("{error:#}"), "terminated with an error");
}
