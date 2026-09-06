//! The host-side `Event` type, mirroring the `event` WIT variant. This is the
//! single type every I/O source (inbox message, chat-stream delta, timer,
//! resource notification) eventually pushes into an agent's inbox.

use crate::provider::ChatDelta;
use crate::tooling::ResourceContent;

/// The result of a single tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
  /// The tool's name, as passed to `call-tool`.
  pub name: String,
  /// The tool's arguments, as opaque JSON passed to `call-tool`.
  pub arguments: String,
  /// The tool's JSON result.
  pub result: String,
}

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
  ChatDelta(ChatDelta),
  /// A chat stream finished.
  StreamEnd,
  /// A queued tool invocation returned; carries the tool result.
  ToolResult(ToolResult),
  /// The subscribed resource list changed.
  ResourceListUpdated(Vec<crate::tooling::ResourceInfo>),
  /// A subscribed resource updated in place; carries the freshly read content.
  ResourceUpdated(ResourceContent),
}

/// An event tagged with the UUID of its subscribed source.
#[derive(Debug, Clone, PartialEq)]
pub struct EventEnvelope {
  /// UUID handle of the subscribed source this event came from.
  pub id: String,
  /// The payload.
  pub event: Event,
}
