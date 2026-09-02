//! The host-side `Event` type, mirroring the `event` WIT variant. This is the
//! single type every I/O source (inbox message, chat-stream delta, timer,
//! resource notification) eventually pushes into an agent's inbox.

use crate::provider::ChatDelta;

/// A strongly-typed event delivered into an agent's inbox.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
  /// A message from a subscribed agent.
  Message(String),
  /// A failed I/O surfaced to the guest.
  Error(String),
  /// A timer (timestamp / duration / cron wait) fired.
  Timer,
  /// A chat-stream delta.
  Delta(ChatDelta),
  /// A chat stream finished.
  StreamEnd,
  /// A subscribed resource list changed.
  ResourceChanged,
  /// A subscribed resource updated in place.
  ResourceUpdated,
}

/// An event tagged with the UUID of its subscribed source.
#[derive(Debug, Clone, PartialEq)]
pub struct EventEnvelope {
  /// UUID handle of the subscribed source this event came from.
  pub id: String,
  /// The payload.
  pub event: Event,
}
