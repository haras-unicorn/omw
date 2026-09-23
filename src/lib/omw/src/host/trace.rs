//! The in-process trace channel: a broadcast stream of what agents observed
//! and did, for deterministic brain testing (`omw-test`) and any embedder that
//! wants a structured run report.
//!
//! The channel is optional and zero-overhead when unset: [`MessageBus`] and
//! [`AgentContext`] hold an `Option<TraceSender>`, and every tap is a
//! synchronous `broadcast::Sender::send`, so it is safe from the
//! `spawn_blocking` wasm thread with no async bridge.
//!
//! [`MessageBus`]: crate::host::bus::MessageBus
//! [`AgentContext`]: crate::host::ctx::AgentContext

use std::collections::BTreeMap;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::host::events::Event;
use crate::runtime::RunOutcome;

/// The default trace buffer. A whole small run fits without lag; a larger run
/// gets a loud [`RecvError::Lagged`] rather than a silent drop.
///
/// [`RecvError::Lagged`]: broadcast::error::RecvError::Lagged
pub const DEFAULT_TRACE_BUFFER: usize = 4096;

/// One trace observation.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceEvent {
  /// An event observed from an agent's inbox (`recv` / `try_recv` success).
  Inbound {
    agent: String,
    id: String,
    event: Event,
  },
  /// An outbound host call made by an agent.
  Call {
    agent: String,
    op: String,
    detail: Value,
  },
  /// An agent iteration's terminal outcome.
  Outcome { agent: String, outcome: RunOutcome },
}

impl TraceEvent {
  /// The agent this observation belongs to.
  pub fn agent(&self) -> &str {
    match self {
      Self::Inbound { agent, .. }
      | Self::Call { agent, .. }
      | Self::Outcome { agent, .. } => agent,
    }
  }
}

/// The broadcast sender half of the trace channel.
pub type TraceSender = broadcast::Sender<TraceEvent>;

/// One agent's slice of a traced run.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AgentTrace {
  /// The agent's terminal outcome, when it stopped cleanly.
  pub outcome: Option<RunOutcome>,
  /// The agent's ordered observations (inbound + calls).
  pub events: Vec<TraceEvent>,
}

/// Group a flat trace stream by agent, preserving per-agent order.
pub fn group(
  events: impl IntoIterator<Item = TraceEvent>,
) -> BTreeMap<String, AgentTrace> {
  let mut grouped: BTreeMap<String, AgentTrace> = BTreeMap::new();
  for event in events {
    if let TraceEvent::Outcome { agent, outcome } = &event {
      grouped.entry(agent.clone()).or_default().outcome = Some(outcome.clone());
      continue;
    }
    let agent = event.agent().to_owned();
    grouped.entry(agent).or_default().events.push(event);
  }
  grouped
}
