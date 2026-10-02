//! Per-command logging policy: the mechanism lives in `omw-output`.

use omw_output::LogFormat;

use crate::cli::Command;

/// Install the tracing subscriber for `command`, returning the live view when
/// the resolved format is `tty`.
pub fn init(
  command: &Command,
  requested: LogFormat,
) -> Option<omw_output::Live> {
  omw_output::init_logging(format(command, requested), "info")
}

/// The format for a command: data producers never use the live view.
fn format(command: &Command, requested: LogFormat) -> LogFormat {
  match command {
    Command::Run { .. } => requested,
    Command::Schema { .. } => omw_output::producer_format(requested),
    #[cfg(feature = "compile-wasm")]
    Command::CompileWasm { .. } => omw_output::producer_format(requested),
  }
}
