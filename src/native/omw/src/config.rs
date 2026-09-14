//! TOML configuration: global provider/tooling/runtime implementations plus
//! per-agent wiring.
//!
//! Config is deliberately impl-agnostic: each provider/tooling/runtime/endpoint
//! entry is a `kind` string plus an opaque params blob. The kind is validated
//! and the params are deserialized into an impl-specific struct only when the
//! impl is constructed (see the `Registry` in each of `provider`, `tooling`,
//! `runtime` and `endpoint`).

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A single configured implementation: which kind plus opaque params.
#[derive(Deserialize, Clone, Serialize, JsonSchema)]
pub struct ImplConfig {
  /// Which implementation this is.
  pub kind: String,
  /// Impl-specific options, validated at construction time.
  #[serde(flatten)]
  pub params: serde_json::Value,
}

impl std::fmt::Debug for ImplConfig {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self.params.as_object() {
      Some(map) => {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        f.debug_struct("ImplConfig")
          .field("kind", &self.kind)
          .field("params_keys", &keys)
          .finish()
      }
      None => f
        .debug_struct("ImplConfig")
        .field("kind", &self.kind)
        .field("params", &"<non-object>")
        .finish(),
    }
  }
}

/// Runtime tunables: channel sizes, timeouts, and backoffs. All optional;
/// omitted values fall back to the built-in defaults.
#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema,
)]
#[serde(default)]
pub struct Tunables {
  /// How many events a single agent inbox buffers before sends fail.
  #[serde(default = "default_inbox_bound")]
  pub inbox_bound: usize,
  /// How long `recv_while` parks between early-abort checks, in ms.
  #[serde(default = "default_recv_slice_ms")]
  pub recv_slice_ms: u64,
  /// How long a blocking `recv` waits before timing out, in seconds.
  #[serde(default = "default_recv_timeout_secs")]
  pub recv_timeout_secs: u64,
  /// How long the blocking-call helper waits between reload checks, in ms.
  #[serde(default = "default_reload_poll_ms")]
  pub reload_poll_ms: u64,
  /// Uninterrupted runtime execution allowed after grace expires, in ms.
  #[serde(default = "default_interrupt_budget_ms")]
  pub interrupt_budget_ms: u64,
  /// How long the supervisor waits for a cooperative exit, in seconds.
  #[serde(default = "default_reload_grace_secs")]
  pub reload_grace_secs: u64,
  /// Backoff start for `loop` restarts on failure, in ms.
  #[serde(default = "default_loop_backoff_start_ms")]
  pub loop_backoff_start_ms: u64,
  /// Backoff cap for `loop` restarts on failure, in seconds.
  #[serde(default = "default_loop_backoff_cap_secs")]
  pub loop_backoff_cap_secs: u64,
  /// Backoff start for tooling reconnects on failure, in ms.
  #[serde(default = "default_tooling_connect_backoff_start_ms")]
  pub tooling_connect_backoff_start_ms: u64,
  /// Backoff cap for tooling reconnects on failure, in seconds.
  #[serde(default = "default_tooling_connect_backoff_cap_secs")]
  pub tooling_connect_backoff_cap_secs: u64,
  /// How long to coalesce the burst of file events a single save produces,
  /// in ms.
  #[serde(default = "default_watch_debounce_ms")]
  pub watch_debounce_ms: u64,
  /// How many deltas a single endpoint session buffers before drops.
  #[serde(default = "default_session_buffer")]
  pub session_buffer: usize,
  /// Cancel open pumps (streams, timers, resources, tool calls) on reload.
  /// `false` keeps them across reload.
  #[serde(default = "default_cancel_pumps_on_reload")]
  pub cancel_pumps_on_reload: bool,
}

fn default_inbox_bound() -> usize {
  1024
}

fn default_recv_slice_ms() -> u64 {
  200
}

fn default_recv_timeout_secs() -> u64 {
  60
}

fn default_reload_poll_ms() -> u64 {
  200
}

fn default_interrupt_budget_ms() -> u64 {
  100
}

fn default_reload_grace_secs() -> u64 {
  5
}

fn default_loop_backoff_start_ms() -> u64 {
  100
}

fn default_loop_backoff_cap_secs() -> u64 {
  30
}

fn default_tooling_connect_backoff_start_ms() -> u64 {
  100
}

fn default_tooling_connect_backoff_cap_secs() -> u64 {
  30
}

fn default_watch_debounce_ms() -> u64 {
  200
}

fn default_session_buffer() -> usize {
  8192
}

fn default_cancel_pumps_on_reload() -> bool {
  true
}

impl Default for Tunables {
  fn default() -> Self {
    Self {
      inbox_bound: default_inbox_bound(),
      recv_slice_ms: default_recv_slice_ms(),
      recv_timeout_secs: default_recv_timeout_secs(),
      reload_poll_ms: default_reload_poll_ms(),
      interrupt_budget_ms: default_interrupt_budget_ms(),
      reload_grace_secs: default_reload_grace_secs(),
      loop_backoff_start_ms: default_loop_backoff_start_ms(),
      loop_backoff_cap_secs: default_loop_backoff_cap_secs(),
      tooling_connect_backoff_start_ms:
        default_tooling_connect_backoff_start_ms(),
      tooling_connect_backoff_cap_secs:
        default_tooling_connect_backoff_cap_secs(),
      watch_debounce_ms: default_watch_debounce_ms(),
      session_buffer: default_session_buffer(),
      cancel_pumps_on_reload: default_cancel_pumps_on_reload(),
    }
  }
}

impl Tunables {
  pub fn recv_slice(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.recv_slice_ms)
  }

  pub fn recv_timeout(&self) -> std::time::Duration {
    std::time::Duration::from_secs(self.recv_timeout_secs)
  }

  pub fn reload_poll(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.reload_poll_ms)
  }

  pub fn interrupt_budget(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.interrupt_budget_ms)
  }

  pub fn reload_grace(&self) -> std::time::Duration {
    std::time::Duration::from_secs(self.reload_grace_secs)
  }

  pub fn loop_backoff_start(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.loop_backoff_start_ms)
  }

  pub fn loop_backoff_cap(&self) -> std::time::Duration {
    std::time::Duration::from_secs(self.loop_backoff_cap_secs)
  }

  pub fn tooling_connect_backoff_start(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.tooling_connect_backoff_start_ms)
  }

  pub fn tooling_connect_backoff_cap(&self) -> std::time::Duration {
    std::time::Duration::from_secs(self.tooling_connect_backoff_cap_secs)
  }

  pub fn watch_debounce(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.watch_debounce_ms)
  }
}

/// OMW configuration.
#[derive(Debug, Deserialize, Clone, Serialize, JsonSchema)]
pub struct Config {
  /// Named provider implementations.
  #[serde(default)]
  pub providers: HashMap<String, ImplConfig>,
  /// Named tooling implementations.
  #[serde(default)]
  pub tooling: HashMap<String, ImplConfig>,
  /// Named runtime implementations.
  #[serde(default)]
  pub runtime: HashMap<String, ImplConfig>,

  /// Optional endpoint implementation: a `kind` string plus opaque params,
  /// like providers/tooling/runtime. When set, a server is started and the
  /// agents can subscribe to it as models.
  #[serde(default)]
  pub endpoint: Option<ImplConfig>,
  #[serde(default)]
  pub agents: Vec<AgentConfig>,
  /// Global runtime tunables; all optional with built-in defaults.
  #[serde(default)]
  pub tunables: Tunables,
}

/// A single agent wiring itself to the globals above.
#[derive(Debug, Deserialize, Clone, Serialize, JsonSchema)]
pub struct AgentConfig {
  pub name: String,
  /// Which named runtime implementation this agent's brain uses.
  pub runtime: String,
  /// The agent's brain script.
  pub script: String,
}
