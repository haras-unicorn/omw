//! Configuration: global provider/tooling/runtime implementations plus
//! per-agent wiring, in TOML, YAML or JSON.
//!
//! Config is deliberately impl-agnostic: each provider/tooling/runtime/endpoint
//! entry is a `kind` string plus an opaque params blob. The kind is validated
//! and the params are deserialized into an impl-specific struct only when the
//! impl is constructed (see the `Registry` in each of `provider`, `tooling`,
//! `runtime` and `endpoint`).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::str::FromStr;

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The on-disk format a configuration (or `[assertions]` section) is written
/// in. Inferred from a file's extension, or chosen explicitly with `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
  /// TOML (`.toml`).
  Toml,
  /// YAML (`.yaml` / `.yml`).
  Yaml,
  /// JSON (`.json`).
  Json,
}

impl Format {
  /// Infer a format from a bare extension (case-insensitive).
  pub fn from_extension(extension: &str) -> Option<Self> {
    match extension.to_ascii_lowercase().as_str() {
      "toml" => Some(Self::Toml),
      "yaml" | "yml" => Some(Self::Yaml),
      "json" => Some(Self::Json),
      _ => None,
    }
  }

  /// Infer a format from a path's extension.
  pub fn from_path(path: &Path) -> Option<Self> {
    path
      .extension()
      .and_then(|extension| extension.to_str())
      .and_then(Self::from_extension)
  }

  /// Parse `source` into `T` using this format.
  pub fn parse<T: DeserializeOwned>(self, source: &str) -> anyhow::Result<T> {
    match self {
      Self::Toml => toml::from_str(source).context("invalid TOML"),
      Self::Yaml => serde_norway::from_str(source).context("invalid YAML"),
      Self::Json => serde_json::from_str(source).context("invalid JSON"),
    }
  }

  /// The canonical lowercase name.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Toml => "toml",
      Self::Yaml => "yaml",
      Self::Json => "json",
    }
  }
}

impl std::fmt::Display for Format {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

impl FromStr for Format {
  type Err = anyhow::Error;

  fn from_str(value: &str) -> anyhow::Result<Self> {
    match value.to_ascii_lowercase().as_str() {
      "toml" => Ok(Self::Toml),
      "yaml" | "yml" => Ok(Self::Yaml),
      "json" => Ok(Self::Json),
      other => {
        anyhow::bail!(
          "unknown config format {other:?} (expected toml, yaml or json)"
        )
      }
    }
  }
}

/// OMW configuration.
#[derive(Debug, Deserialize, Clone, Serialize, JsonSchema)]
pub struct Config {
  /// Agents that OMW is going to run, keyed by agent name.
  #[serde(default)]
  pub agents: BTreeMap<String, AgentConfig>,

  /// Named provider implementations.
  #[serde(default)]
  #[schemars(with = "crate::provider::ProviderImpls")]
  pub providers: HashMap<String, ImplConfig>,
  /// Named tooling implementations.
  #[serde(default)]
  #[schemars(with = "crate::tooling::ToolingImpls")]
  pub tooling: HashMap<String, ImplConfig>,
  /// Named runtime implementations.
  #[serde(default)]
  #[schemars(with = "crate::runtime::RuntimeImpls")]
  pub runtime: HashMap<String, ImplConfig>,
  /// Optional endpoint implementation.
  #[serde(default)]
  #[schemars(with = "crate::endpoint::EndpointImpls")]
  pub endpoint: Option<ImplConfig>,

  /// Per-agent seeded memory, keyed by agent name then key. Seeded into the
  /// agent's memory before its brain first runs, so a test (or a deployment)
  /// can fast-forward an agent to a state. Seeded values persist like any
  /// other memory, including across hot reloads.
  ///
  /// A value that is a string is stored verbatim; anything else (object,
  /// array, number, bool) is JSON-stringified, mirroring `memory-set`. Read it
  /// back with `memory-get` (raw) or `memory-get-as` (parsed).
  #[serde(default)]
  pub memory: BTreeMap<String, BTreeMap<String, serde_json::Value>>,

  /// Global runtime tunables.
  #[serde(default)]
  pub tunables: Tunables,
}

impl Config {
  /// The JSON schema document for the configuration, pretty-printed with a
  /// trailing newline.
  ///
  /// Feature-aware: each of `providers`/`tooling`/`runtime`/`endpoint` unions
  /// only the built-in kinds compiled into this build (so `omw-cli` describes
  /// openai/mcp/openai-endpoint/wasm, while `omw-test` adds the `mock` test
  /// doubles and any script runtimes it enables). Unknown kinds still validate
  /// through the generic escape hatch.
  pub fn schema_json() -> anyhow::Result<String> {
    let schema = schemars::schema_for!(Config);
    let mut json = serde_json::to_string_pretty(&schema)
      .context("failed to serialize config schema")?;
    json.push('\n');
    Ok(json)
  }
}

/// Normalize one seeded memory value into the string the store holds: a JSON
/// string stays verbatim, anything else is JSON-stringified. The inverse is a
/// guest's `memory-get-as` / `memoryGetAs`.
pub(crate) fn memory_string(value: &serde_json::Value) -> String {
  match value {
    serde_json::Value::String(s) => s.clone(),
    other => {
      serde_json::to_string(other).unwrap_or_else(|_| "null".to_string())
    }
  }
}

/// A single agent wiring itself to the globals above. The agent's name is the
/// key it appears under in [`Config::agents`].
#[derive(Debug, Deserialize, Clone, Serialize, JsonSchema)]
pub struct AgentConfig {
  /// Which named runtime implementation this agent's brain uses.
  pub runtime: String,
  /// The agent's brain script.
  pub script: String,
}

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
  /// How long the live view's render loop waits between redraws, in ms. It is
  /// both the spinner cadence and the maximum redraw interval.
  #[serde(default = "default_tui_tick_ms")]
  pub tui_tick_ms: u64,
  /// How many log lines each live-view tab keeps before the oldest are
  /// dropped. `0` means unlimited.
  #[serde(default = "default_tui_tab_capacity")]
  pub tui_tab_capacity: usize,
  /// How many trace events the `omw-test` broadcast channel buffers.
  #[serde(default = "default_trace_buffer")]
  pub trace_buffer: usize,
  /// How long a single testing-harness run may take before it is force-stopped
  /// and every unsettled agent marked timed out, in seconds. `0` disables it.
  #[serde(default = "default_test_timeout_secs")]
  pub test_timeout_secs: u64,
  /// How many deltas a single endpoint session buffers before drops.
  #[serde(default = "default_session_buffer")]
  pub session_buffer: usize,
  /// Cancel open pumps (streams, timers, resources, tool calls) on reload.
  /// `false` keeps them across reload.
  #[serde(default = "default_cancel_pumps_on_reload")]
  pub cancel_pumps_on_reload: bool,
  /// Permit secrets to stay unlocked (pageable) when `mlock` fails, e.g.
  /// inside containers where the outer `RLIMIT_MEMLOCK` cannot be raised.
  /// Default `false` (fail-closed). Only enable where the weaker guarantee
  /// is acceptable.
  #[serde(default = "default_allow_unlocked_secrets")]
  pub allow_unlocked_secrets: bool,
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

fn default_tui_tick_ms() -> u64 {
  80
}

fn default_tui_tab_capacity() -> usize {
  2000
}

fn default_trace_buffer() -> usize {
  crate::host::trace::DEFAULT_TRACE_BUFFER
}

fn default_test_timeout_secs() -> u64 {
  30
}

fn default_session_buffer() -> usize {
  8192
}

fn default_cancel_pumps_on_reload() -> bool {
  true
}

fn default_allow_unlocked_secrets() -> bool {
  false
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
      tui_tick_ms: default_tui_tick_ms(),
      tui_tab_capacity: default_tui_tab_capacity(),
      trace_buffer: default_trace_buffer(),
      test_timeout_secs: default_test_timeout_secs(),
      session_buffer: default_session_buffer(),
      cancel_pumps_on_reload: default_cancel_pumps_on_reload(),
      allow_unlocked_secrets: default_allow_unlocked_secrets(),
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

  /// The live view's redraw interval.
  pub fn tui_tick(&self) -> std::time::Duration {
    std::time::Duration::from_millis(self.tui_tick_ms)
  }

  /// The live view's per-tab log cap, or `None` when unlimited (`0`).
  pub fn tui_tab_capacity(&self) -> Option<usize> {
    if self.tui_tab_capacity == 0 {
      None
    } else {
      Some(self.tui_tab_capacity)
    }
  }

  /// The harness run timeout, or `None` when disabled (`0`).
  pub fn test_timeout(&self) -> Option<std::time::Duration> {
    if self.test_timeout_secs == 0 {
      None
    } else {
      Some(std::time::Duration::from_secs(self.test_timeout_secs))
    }
  }
}

#[cfg(test)]
mod schema_tests {
  use super::Config;

  /// The generated schema names every built-in kind enabled in this build.
  #[test]
  fn schema_json_describes_enabled_kinds() -> anyhow::Result<()> {
    let json = Config::schema_json()?;
    let asserts = |kind: &str| {
      assert!(
        json.contains(&format!("\"const\": \"{kind}\"")),
        "schema is missing the {kind:?} kind"
      );
    };
    #[cfg(feature = "provider-openai")]
    asserts("openai");
    #[cfg(feature = "tooling-mcp")]
    asserts("mcp");
    #[cfg(feature = "endpoint-openai")]
    asserts("openai");
    #[cfg(feature = "runtime-wasm")]
    asserts("wasm");
    #[cfg(feature = "runtime-rhai")]
    asserts("rhai");
    #[cfg(feature = "runtime-js")]
    asserts("js");
    #[cfg(feature = "runtime-python")]
    asserts("python");
    #[cfg(feature = "mock")]
    asserts("mock");
    // The generic escape hatch keeps custom kinds valid.
    assert!(
      json.contains("ImplConfig"),
      "schema is missing the escape hatch"
    );
    Ok(())
  }
}

#[cfg(test)]
mod tunables_tests {
  use super::Tunables;

  #[test]
  fn live_view_defaults() {
    let tunables = Tunables::default();
    assert_eq!(tunables.tui_tick_ms, 80);
    assert_eq!(tunables.tui_tab_capacity, 2000);
    assert_eq!(tunables.tui_tick(), std::time::Duration::from_millis(80));
    assert_eq!(tunables.tui_tab_capacity(), Some(2000));
  }

  #[test]
  fn zero_tab_capacity_means_unlimited() {
    let tunables = Tunables {
      tui_tab_capacity: 0,
      ..Tunables::default()
    };
    assert_eq!(tunables.tui_tab_capacity(), None);
  }
}

#[cfg(test)]
mod memory_tests {
  use super::memory_string;

  #[test]
  fn strings_are_stored_verbatim() {
    assert_eq!(memory_string(&serde_json::json!("handle-1")), "handle-1");
  }

  #[test]
  fn non_strings_are_json_stringified() {
    assert_eq!(memory_string(&serde_json::json!({ "a": 1 })), r#"{"a":1}"#);
    assert_eq!(memory_string(&serde_json::json!([1, 2])), "[1,2]");
    assert_eq!(memory_string(&serde_json::json!(42)), "42");
    assert_eq!(memory_string(&serde_json::json!(true)), "true");
  }

  #[test]
  fn parses_into_the_structured_config() -> anyhow::Result<()> {
    let config: crate::config::Config = toml::from_str(
      r#"
        [memory.alice]
        handle = "seed-42"
        state = { step = 3, waiting = true }
      "#,
    )?;
    let alice = &config.memory["alice"];
    assert_eq!(alice["handle"], serde_json::json!("seed-42"));
    assert_eq!(alice["state"]["step"], serde_json::json!(3));
    assert_eq!(alice["state"]["waiting"], serde_json::json!(true));
    Ok(())
  }
}
