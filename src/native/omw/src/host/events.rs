//! The host-side `Event` type, mirroring the `event` WIT variant. This is the
//! single type every I/O source (inbox message, chat-stream delta, timer,
//! resource notification) eventually pushes into an agent's inbox.

use crate::provider::ChatDelta;
use crate::tooling::ResourceContent;

/// The result of a single tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
  pub name: String,
  pub arguments: String,
  pub result: String,
}

/// An inbound endpoint request routed to a subscribed agent. `session`
/// distinguishes the request from others in the same subscription, and
/// addresses the agent's streamed deltas back out through `endpoint-stream`.
#[derive(Debug, Clone, PartialEq)]
pub struct EndpointMessage {
  pub session: String,
  pub messages: Vec<crate::provider::ChatMessage>,
  pub tools: Vec<crate::tooling::Tool>,
}

/// An endpoint session ended: normally (`error` absent) when the agent's
/// reply completed, or abruptly (`error` present) when the session was
/// interrupted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointSessionEnd {
  pub session: String,
  pub error: Option<String>,
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
  /// An inbound endpoint request routed to a subscribed agent.
  EndpointMessage(EndpointMessage),
  /// An endpoint session ended: normal or abrupt.
  EndpointSessionEnd(EndpointSessionEnd),
}

/// An event tagged with the UUID of its subscribed source.
#[derive(Debug, Clone, PartialEq)]
pub struct EventEnvelope {
  /// UUID handle of the subscribed source this event came from.
  pub id: String,
  /// The payload.
  pub event: Event,
}
