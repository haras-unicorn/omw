//! Per-agent memory that survives hot reloads and `loop` restarts.
//!
//! Each agent owns one `Memory`, so agents never share keys and cannot race
//! each other. Hot reloads reuse the same
//! [`AgentContext`](crate::host::ctx::AgentContext), and `loop` builds a fresh
//! context per iteration but adopts the agent's store from the supervisor's
//! `Shared::memories` (see `agent::run_agent`), so values set in one iteration
//! are still visible in the next. A fresh process (or a fresh context in
//! tests) starts empty.
//!
//! Treat entries like variables: subscription handles, state-machine state,
//! small checkpoints. Not a database, not a blob store.

use dashmap::DashMap;

/// String-to-string store scoped to a single agent.
#[derive(Debug, Default)]
pub struct Memory {
  inner: DashMap<String, String>,
}

impl Memory {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn get(&self, key: &str) -> Option<String> {
    self.inner.get(key).map(|value| value.clone())
  }

  pub fn set(&self, key: String, value: String) {
    self.inner.insert(key, value);
  }

  /// Insert several entries at once, overwriting any existing values.
  pub(crate) fn seed(
    &self,
    entries: impl IntoIterator<Item = (String, String)>,
  ) {
    for (key, value) in entries {
      self.inner.insert(key, value);
    }
  }

  pub fn remove(&self, key: &str) -> bool {
    self.inner.remove(key).is_some()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn get_returns_none_for_missing_keys() {
    let memory = Memory::new();
    assert_eq!(memory.get("nope"), None);
  }

  #[test]
  fn set_then_get_roundtrips() {
    let memory = Memory::new();
    memory.set("handle".to_string(), "uuid-1".to_string());
    assert_eq!(memory.get("handle"), Some("uuid-1".to_string()));
  }

  #[test]
  fn set_overwrites_the_previous_value() {
    let memory = Memory::new();
    memory.set("state".to_string(), "a".to_string());
    memory.set("state".to_string(), "b".to_string());
    assert_eq!(memory.get("state"), Some("b".to_string()));
  }

  #[test]
  fn remove_reports_presence() {
    let memory = Memory::new();
    assert!(!memory.remove("missing"));
    memory.set("k".to_string(), "v".to_string());
    assert!(memory.remove("k"));
    assert_eq!(memory.get("k"), None);
    assert!(!memory.remove("k"));
  }

  #[test]
  fn each_memory_is_independent() {
    let first = Memory::new();
    let second = Memory::new();
    first.set("k".to_string(), "v".to_string());
    assert_eq!(second.get("k"), None);
  }

  #[test]
  fn seed_inserts_all_entries_and_overwrites() {
    let memory = Memory::new();
    memory.set("handle".to_string(), "old".to_string());
    memory.seed([
      ("handle".to_string(), "uuid-1".to_string()),
      ("state".to_string(), "waiting".to_string()),
    ]);
    assert_eq!(memory.get("handle"), Some("uuid-1".to_string()));
    assert_eq!(memory.get("state"), Some("waiting".to_string()));
  }

  #[test]
  fn seed_survives_a_shared_handle_like_a_reload() {
    let memory = std::sync::Arc::new(Memory::new());
    memory.seed([("handle".to_string(), "uuid-1".to_string())]);
    let reloaded = std::sync::Arc::clone(&memory);
    assert_eq!(reloaded.get("handle"), Some("uuid-1".to_string()));
  }
}
