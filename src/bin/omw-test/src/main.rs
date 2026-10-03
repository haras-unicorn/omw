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

mod cli;
mod collect;
mod log;
mod run;
mod stdio;
mod tls;
#[cfg(feature = "compile-wasm")]
mod wasm;

#[tokio::main]
async fn main() -> std::process::ExitCode {
  run::run().await
}
