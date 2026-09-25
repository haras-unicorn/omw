//! Deterministic brain testing.
//!
//! This module owns the pieces that are generally useful to any embedder that
//! wants to drive a config against the in-config `kind = "mock"` doubles and
//! check what happened: the [`assert`] assertion model, parser, and matcher.
//! `omw-test` is a thin CLI over it (discovery, filtering, printing, exit
//! codes).
//!
//! The live-run harness that consumes a traced run and stops agents on
//! assertion settle lives here too, alongside the assertion machinery it
//! shares its pattern vocabulary with.

pub mod assert;
pub mod harness;
pub mod scaffold;

pub use assert::{
  After, AgentAssertion, ArrayStep, Assertions, EventAssertion, Matcher,
  OutcomeAssertion, Pattern, check, collect, event_kind, parse,
};
pub use harness::{AgentReport, Harness, Report};
pub use scaffold::scaffold;

#[cfg(any(test, feature = "mock"))]
pub(crate) use assert::TraceLog;
