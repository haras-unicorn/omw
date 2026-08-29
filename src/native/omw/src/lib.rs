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

pub mod agent;
pub mod bindings;
pub mod config;
pub mod host;
pub mod log;
pub mod provider;
pub mod runtime;
pub mod tooling;
