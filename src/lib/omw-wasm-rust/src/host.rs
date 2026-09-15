//! Static host helpers: logging, time, timers, inbox, agents, memory,
//! endpoint, and small utilities.
//!
//! Thin wrappers over the generated `host` interface with friendlier
//! shapes (RAII guards, log macros) and no hidden behavior.

use crate::omw::omw::host as raw;
use crate::omw::omw::types::{ChatDelta, EventEnvelope};

/// Log at `trace` level.
pub fn trace(message: &str) {
  raw::log("trace", message);
}

/// Log at `debug` level.
pub fn debug(message: &str) {
  raw::log("debug", message);
}

/// Log at `info` level.
pub fn info(message: &str) {
  raw::log("info", message);
}

/// Log at `warn` level.
pub fn warn(message: &str) {
  raw::log("warn", message);
}

/// Log at `error` level.
pub fn error(message: &str) {
  raw::log("error", message);
}

/// Current wall clock (milliseconds since epoch).
pub fn now() -> u64 {
  raw::time_now()
}

/// Format a timestamp with a strftime-ish format string.
pub fn format_time(ts: u64, format: &str) -> String {
  raw::time_format(ts, format)
}

/// A pending timer; cancels on drop.
pub struct TimerGuard {
  uuid: String,
}

impl TimerGuard {
  /// The UUID handle tagging this timer's `timer` event.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Cancel the wait; no further `timer` event is delivered.
  pub fn cancel(&self) {
    raw::cancel_timer(&self.uuid);
  }
}

impl Drop for TimerGuard {
  fn drop(&mut self) {
    self.cancel();
  }
}

/// Wait until timestamp `ts` fires. Errors if `ts <= now`.
pub fn wait_until(ts: u64) -> Result<TimerGuard, String> {
  raw::wait_until(ts).map(|uuid| TimerGuard { uuid })
}

/// Wait for `ms` milliseconds.
pub fn wait_for(ms: u64) -> Result<TimerGuard, String> {
  raw::wait_for(ms).map(|uuid| TimerGuard { uuid })
}

/// Wait until the next fire of cron `spec`.
pub fn wait_cron(spec: &str) -> Result<TimerGuard, String> {
  raw::wait_cron(spec).map(|uuid| TimerGuard { uuid })
}

/// Blocking wait for `ms` milliseconds; no event is delivered.
pub fn sleep_for(ms: u64) {
  raw::sleep_for(ms);
}

/// Blocking wait until `ts` fires. Errors if `ts <= now`.
pub fn sleep_until(ts: u64) -> Result<(), String> {
  raw::sleep_until(ts)
}

/// Blocking wait until the next fire of cron `spec`.
pub fn sleep_cron(spec: &str) -> Result<(), String> {
  raw::sleep_cron(spec)
}

/// Subscribe to lifecycle events (`reload`, `shutdown`, `error`).
/// Dropping the guard unsubscribes.
pub fn subscribe_lifecycle() -> Result<LifecycleGuard, String> {
  raw::subscribe_lifecycle().map(|uuid| LifecycleGuard { uuid })
}

/// A lifecycle subscription; unsubscribes on drop.
pub struct LifecycleGuard {
  uuid: String,
}

impl LifecycleGuard {
  /// The UUID handle tagging lifecycle events.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Unsubscribe.
  pub fn unsubscribe(&self) {
    raw::unsubscribe_lifecycle(&self.uuid);
  }
}

impl Drop for LifecycleGuard {
  fn drop(&mut self) {
    self.unsubscribe();
  }
}

/// Subscribe to messages from another agent. Dropping the guard
/// unsubscribes.
pub fn subscribe_agent(agent: &str) -> Result<AgentGuard, String> {
  raw::subscribe_agent(agent).map(|uuid| AgentGuard { uuid })
}

/// An agent subscription; unsubscribes on drop.
pub struct AgentGuard {
  uuid: String,
}

impl AgentGuard {
  /// The UUID handle tagging this agent's `message` events.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Unsubscribe.
  pub fn unsubscribe(&self) {
    raw::unsubscribe_agent(&self.uuid);
  }
}

impl Drop for AgentGuard {
  fn drop(&mut self) {
    self.unsubscribe();
  }
}

/// Send a payload to another agent. It lands only if the recipient
/// subscribed to the sender.
pub fn send_agent(agent: &str, payload: &str) {
  raw::send_agent(agent, payload);
}

/// Blocking receive of the next inbox event.
pub fn recv() -> Result<EventEnvelope, String> {
  raw::recv()
}

/// Non-blocking poll of the next inbox event.
pub fn try_recv() -> Result<Option<EventEnvelope>, String> {
  raw::try_recv()
}

/// A fresh v4 UUID string.
pub fn new_uuid() -> String {
  raw::new_uuid()
}

/// Encode raw bytes as standard padded base64.
pub fn base64_encode(bytes: &[u8]) -> String {
  raw::base64_encode(bytes)
}

/// Decode standard padded base64 back to raw bytes.
pub fn base64_decode(data: &str) -> Result<Vec<u8>, String> {
  raw::base64_decode(data)
}

/// Read a memory value by key. `None` when absent.
pub fn memory_get(key: &str) -> Option<String> {
  raw::memory_get(key)
}

/// Store a memory value under a key.
pub fn memory_set(key: &str, value: &str) {
  raw::memory_set(key, value);
}

/// Delete a memory value. `true` when a value was present.
pub fn memory_remove(key: &str) -> bool {
  raw::memory_remove(key)
}

/// Subscribe this agent to the endpoint under model `model`. Dropping
/// the guard unsubscribes and terminates in-flight sessions.
pub fn subscribe_endpoint(model: &str) -> Result<EndpointGuard, String> {
  raw::subscribe_endpoint(model).map(|uuid| EndpointGuard { uuid })
}

/// An endpoint subscription; unsubscribes on drop.
pub struct EndpointGuard {
  uuid: String,
}

impl EndpointGuard {
  /// The UUID handle tagging inbound `endpoint-message` events.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Unsubscribe; the model drops off `/v1/models`.
  pub fn unsubscribe(&self) {
    raw::unsubscribe_endpoint(&self.uuid);
  }
}

impl Drop for EndpointGuard {
  fn drop(&mut self) {
    self.unsubscribe();
  }
}

/// Stream one delta to the endpoint `session`. A delta carrying a
/// `finish-reason` ends the session.
pub fn stream_endpoint(session: &str, delta: &ChatDelta) -> Result<(), String> {
  raw::stream_endpoint(session, delta)
}
