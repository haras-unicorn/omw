//! An in-memory scripted tooling for tests and local development.
//!
//! The config maps one-to-one onto the WIT `tooling` interface: `tools`
//! answers `list-tools`; an ordered `tool_calls` list answers `call-tool`
//! (name-verified; an unscripted, exhausted or mismatched call is an error the
//! brain receives); `initial_resource_list` / `initial_resource_contents`
//! answer `list-resources` / `read-resource`; and the ordered
//! `resource_list_updates` / `resource_content_updates` lists drive the two
//! subscription streams, each step optionally gated on the trace with an
//! `after`. `delay_ms` paces every scripted step.
//!
//! There is no fallback: a subscription with no configured updates emits
//! nothing. Each `call-tool` records the invocation (name and arguments) so
//! tests can assert what the guest actually sent.
#![allow(
  dead_code,
  reason = "the mock back end is a test double; its inspection helpers and failure fakes are used by unit tests and by omw-test, not by the library itself"
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{
  Factory, ResourceContent, ResourceInfo, ResourceInjections,
  ResourceNotification, Tool, ToolCallResult, Tooling,
};
use crate::host::bus::{Injection, MessageBus};
use crate::host::events::{Event, ToolResult};
use crate::host::streams::CancelRegistry;
use crate::host::trace::{TraceEvent, TraceSender};
use crate::testing::{After, TraceLog};

/// Lock a `std::sync::Mutex`, recovering the guard on poison (a poisoned lock
/// should never take the whole runtime down).
fn lock<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
  mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn default_delay_ms() -> u64 {
  10
}

/// Impl-specific configuration for the mock tooling.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct Config {
  #[serde(default)]
  pub tools: Vec<Tool>,

  /// Ordered `call-tool` results; consumed one per call and name-verified.
  #[serde(default)]
  pub tool_calls: Vec<ScriptedToolCall>,

  /// The resource list `list-resources` starts from.
  #[serde(default)]
  pub initial_resource_list: Vec<ResourceInfo>,

  /// Canned resource content keyed by URI, returned by `read-resource`.
  #[serde(default)]
  pub initial_resource_contents: HashMap<String, String>,

  /// Ordered, full-replacement resource-list updates driven by
  /// `subscribe-resource-list`.
  #[serde(default)]
  pub resource_list_updates: Vec<ResourceListUpdate>,

  /// Ordered, one-by-one content updates driven by `subscribe-resource`.
  #[serde(default)]
  pub resource_content_updates: Vec<ResourceContentUpdate>,

  /// How long each scripted step waits before firing.
  #[serde(default = "default_delay_ms")]
  pub delay_ms: u64,
}

/// One scripted `call-tool` result. `result` is a text convenience that
/// becomes a single MCP text content block; `structured_content` is passed
/// through verbatim.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ScriptedToolCall {
  name: String,
  #[serde(default)]
  result: String,
  #[serde(default)]
  structured_content: Option<Value>,
  /// Fail the call with this error (after the gate and delay).
  #[serde(default)]
  error: Option<String>,
  /// Never complete: the call stays pending forever (for cancellation tests).
  #[serde(default)]
  pending: bool,
  #[serde(default)]
  after: After,
}

/// Build an MCP-style content array holding a single text block.
fn text_content(text: &str) -> Value {
  serde_json::json!([{ "type": "text", "text": text }])
}

/// One scripted, full-replacement resource-list update.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ResourceListUpdate {
  resources: Vec<ResourceInfo>,
  /// Yield this error for the step instead of a `ListChanged`.
  #[serde(default)]
  error: Option<String>,
  /// Never yield: the step stays pending forever (for cancellation tests).
  #[serde(default)]
  pending: bool,
  #[serde(default)]
  after: After,
}

/// One scripted, one-by-one resource-content update.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ResourceContentUpdate {
  uri: String,
  content: String,
  /// Yield this error for the step instead of an `Updated`.
  #[serde(default)]
  error: Option<String>,
  /// Never yield: the step stays pending forever (for cancellation tests).
  #[serde(default)]
  pending: bool,
  #[serde(default)]
  after: After,
}

/// The mutable resource state a subscription step advances.
#[derive(Debug, Default)]
struct State {
  resources: Vec<ResourceInfo>,
  contents: HashMap<String, String>,
}

/// A single recorded `call-tool` invocation.
#[derive(Debug, Clone)]
pub struct ToolCall {
  pub name: String,
  pub arguments: Value,
}

/// A recorded `call-tool` invocation and the result it produced, when it
/// succeeded. `result` is `None` for a call that errored (name mismatch,
/// exhausted script, …).
#[derive(Debug, Clone)]
pub struct RecordedToolCall {
  pub call: ToolCall,
  pub result: Option<ToolCallResult>,
}

/// A scripted tooling backed by in-memory state. `call-tool` consumes the
/// ordered script; subscriptions replay their ordered update lists, each step
/// waiting for its `after` gate (when the trace is attached) and `delay_ms`.
pub struct MockTooling {
  tools: Vec<Tool>,
  tool_calls: Vec<ScriptedToolCall>,
  tool_call_next: Arc<Mutex<usize>>,
  resource_list_updates: Vec<ResourceListUpdate>,
  resource_content_updates: Vec<ResourceContentUpdate>,
  state: Arc<Mutex<State>>,
  calls: Arc<Mutex<Vec<RecordedToolCall>>>,
  /// The append-only trace log, attached once before the run so any gate can
  /// observe any event, including ones that precede the step that waits on it.
  trace: Arc<OnceLock<Arc<TraceLog>>>,
  delay: Duration,
}

impl Factory for MockTooling {
  fn build(
    name: &str,
    params: &Value,
    _tunables: crate::config::Tunables,
  ) -> anyhow::Result<Arc<Self>> {
    let config = Config::deserialize(params)
      .with_context(|| format!("invalid mock tooling config for {name:?}"))?;
    Ok(Arc::new(MockTooling {
      tools: config.tools,
      tool_calls: config.tool_calls,
      tool_call_next: Arc::new(Mutex::new(0)),
      resource_list_updates: config.resource_list_updates,
      resource_content_updates: config.resource_content_updates,
      state: Arc::new(Mutex::new(State {
        resources: config.initial_resource_list,
        contents: config.initial_resource_contents,
      })),
      calls: Arc::new(Mutex::new(Vec::new())),
      trace: Arc::new(OnceLock::new()),
      delay: Duration::from_millis(config.delay_ms),
    }))
  }

  fn opaque_fields() -> &'static [&'static str] {
    &["initial_resource_contents"]
  }
}

impl MockTooling {
  /// A bare mock with no tools (for tests that only assert the call).
  pub fn noop() -> Arc<Self> {
    Arc::new(Self::noop_owned())
  }

  /// A mock whose first `call-tool` for `name` returns `result`.
  pub fn with_tool_call(name: &str, result: &str) -> Arc<Self> {
    Arc::new(Self {
      tool_calls: vec![ScriptedToolCall {
        name: name.to_string(),
        result: result.to_string(),
        structured_content: None,
        error: None,
        pending: false,
        after: After::Start,
      }],
      ..Self::noop_owned()
    })
  }

  /// A mock exposing the given resources and replaying them once on
  /// `subscribe-resource-list` (so a subscriber sees one change).
  pub fn with_resources(resources: Vec<ResourceInfo>) -> Arc<Self> {
    Arc::new(Self {
      resource_list_updates: vec![ResourceListUpdate {
        resources: resources.clone(),
        error: None,
        pending: false,
        after: After::Start,
      }],
      state: Arc::new(Mutex::new(State {
        resources,
        contents: HashMap::new(),
      })),
      ..Self::noop_owned()
    })
  }

  /// A mock exposing one resource whose `read-resource` returns `content`, and
  /// which replays that content once on `subscribe-resource`.
  pub fn with_resource_content(uri: &str, content: &str) -> Arc<Self> {
    let resources = vec![ResourceInfo {
      uri: uri.to_string(),
      name: uri.to_string(),
      description: None,
      mime_type: Some("text/plain".to_string()),
    }];
    let mut contents = HashMap::new();
    contents.insert(uri.to_string(), content.to_string());
    Arc::new(Self {
      resource_content_updates: vec![ResourceContentUpdate {
        uri: uri.to_string(),
        content: content.to_string(),
        error: None,
        pending: false,
        after: After::Start,
      }],
      state: Arc::new(Mutex::new(State {
        resources,
        contents,
      })),
      ..Self::noop_owned()
    })
  }

  fn noop_owned() -> Self {
    Self {
      tools: Vec::new(),
      tool_calls: Vec::new(),
      tool_call_next: Arc::new(Mutex::new(0)),
      resource_list_updates: Vec::new(),
      resource_content_updates: Vec::new(),
      state: Arc::new(Mutex::new(State::default())),
      calls: Arc::new(Mutex::new(Vec::new())),
      trace: Arc::new(OnceLock::new()),
      delay: Duration::from_millis(default_delay_ms()),
    }
  }

  /// Recorded tool calls, in order.
  pub async fn calls(&self) -> Vec<ToolCall> {
    lock(&self.calls)
      .iter()
      .map(|record| record.call.clone())
      .collect()
  }
}

/// Wait for an `after` gate on the mock's shared trace log. The log is
/// append-only, so gates never consume each other's events and a gate may
/// match an event observed before the step that waits on it.
async fn gate(trace: &Arc<OnceLock<Arc<TraceLog>>>, after: &After) {
  if after.is_start() {
    return;
  }
  match trace.get() {
    Some(log) => log.wait_for(after).await,
    None => {
      tracing::warn!(
        "tooling mock `after` has no trace channel; firing immediately"
      );
    }
  }
}

#[async_trait::async_trait]
impl Tooling for MockTooling {
  fn kind() -> &'static str {
    "mock"
  }

  async fn list_tools(&self) -> anyhow::Result<Vec<Tool>> {
    Ok(self.tools.clone())
  }

  async fn call_tool(
    &self,
    name: &str,
    args: Value,
  ) -> anyhow::Result<ToolCallResult> {
    let record_index = {
      let mut calls = lock(&self.calls);
      calls.push(RecordedToolCall {
        call: ToolCall {
          name: name.to_string(),
          arguments: args,
        },
        result: None,
      });
      calls.len().saturating_sub(1)
    };
    if self.tool_calls.is_empty() {
      anyhow::bail!("mock has no scripted result for tool {name:?}");
    }
    let index = {
      let mut next = lock(&self.tool_call_next);
      let index = *next;
      *next = next.saturating_add(1);
      index
    };
    let Some(step) = self.tool_calls.get(index).cloned() else {
      anyhow::bail!("mock tool calls exhausted at {name:?}");
    };
    if step.name != name {
      anyhow::bail!("mock expected tool {:?}, got {name:?}", step.name);
    }
    gate(&self.trace, &step.after).await;
    tokio::time::sleep(self.delay).await;
    if step.pending {
      std::future::pending::<()>().await;
    }
    if let Some(error) = step.error {
      anyhow::bail!("{error}");
    }
    let result = ToolCallResult {
      content: text_content(&step.result),
      structured_content: step.structured_content,
    };
    if let Some(record) = lock(&self.calls).get_mut(record_index) {
      record.result = Some(result.clone());
    }
    Ok(result)
  }

  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>> {
    Ok(lock(&self.state).resources.clone())
  }

  async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContent> {
    let state = lock(&self.state);
    let content = state
      .contents
      .get(uri)
      .cloned()
      .with_context(|| format!("mock has no content for resource {uri:?}"))?;
    let mime_type = state
      .resources
      .iter()
      .find(|r| r.uri == uri)
      .and_then(|r| r.mime_type.clone());
    Ok(ResourceContent {
      uri: uri.to_string(),
      mime_type,
      content,
    })
  }

  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    let steps = self.resource_list_updates.clone();
    let state = Arc::clone(&self.state);
    let trace = Arc::clone(&self.trace);
    let delay = self.delay;
    Ok(Box::pin(futures_util::stream::unfold(
      (steps, 0usize),
      move |(steps, index)| {
        let state = Arc::clone(&state);
        let trace = Arc::clone(&trace);
        async move {
          let step = steps.get(index)?.clone();
          gate(&trace, &step.after).await;
          tokio::time::sleep(delay).await;
          if step.pending {
            std::future::pending::<()>().await;
          }
          if let Some(error) = step.error {
            return Some((Err(error), (steps, index.saturating_add(1))));
          }
          lock(&state).resources = step.resources;
          Some((
            Ok(ResourceNotification::ListChanged),
            (steps, index.saturating_add(1)),
          ))
        }
      },
    )))
  }

  async fn subscribe_resource(
    &self,
    uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    let steps: Vec<ResourceContentUpdate> = self
      .resource_content_updates
      .iter()
      .filter(|update| update.uri == uri)
      .cloned()
      .collect();
    let state = Arc::clone(&self.state);
    let trace = Arc::clone(&self.trace);
    let delay = self.delay;
    let uri = uri.to_string();
    Ok(Box::pin(futures_util::stream::unfold(
      (steps, 0usize),
      move |(steps, index)| {
        let state = Arc::clone(&state);
        let trace = Arc::clone(&trace);
        let uri = uri.clone();
        async move {
          let step = steps.get(index)?.clone();
          gate(&trace, &step.after).await;
          tokio::time::sleep(delay).await;
          if step.pending {
            std::future::pending::<()>().await;
          }
          if let Some(error) = step.error {
            return Some((Err(error), (steps, index.saturating_add(1))));
          }
          lock(&state).contents.insert(step.uri.clone(), step.content);
          Some((
            Ok(ResourceNotification::Updated { uri }),
            (steps, index.saturating_add(1)),
          ))
        }
      },
    )))
  }

  fn attach_trace(&self, trace: TraceSender) {
    if self.trace.set(TraceLog::attach(trace)).is_err() {
      tracing::warn!("could not attach the trace to the tooling mock");
    }
  }

  fn snapshot(&self) -> Option<Value> {
    let consumed = self
      .tool_call_next
      .try_lock()
      .ok()
      .map(|cursor| (*cursor).min(self.tool_calls.len()))?;
    let calls = self.calls.try_lock().ok().map(|calls| {
      calls
        .iter()
        .map(|record| {
          serde_json::json!({
            "name": record.call.name,
            "arguments": record.call.arguments,
            "result": record.result,
          })
        })
        .collect::<Vec<_>>()
    })?;
    Some(serde_json::json!({
      "kind": "tooling",
      "tool_calls_total": self.tool_calls.len(),
      "tool_calls_consumed": consumed,
      "tool_calls_remaining": self.tool_calls.len().saturating_sub(consumed),
      "calls": calls,
    }))
  }
}

/// Whether a scripted step's `after` gate has been satisfied by the observed
/// trace. `"start"` is always ready.
fn after_ready(after: &After, observed: &[TraceEvent]) -> bool {
  match after {
    After::Start => true,
    After::Pattern(assertion) => {
      observed.iter().any(|event| assertion.matches(event))
    }
  }
}

/// Build a `resource-updated` event for a content step, resolving the MIME
/// type from the mock's current resource list.
fn content_event(state: &Mutex<State>, uri: &str, content: String) -> Event {
  let mime_type = lock(state)
    .resources
    .iter()
    .find(|r| r.uri == uri)
    .and_then(|r| r.mime_type.clone());
  Event::ResourceUpdated(ResourceContent {
    uri: uri.to_string(),
    mime_type,
    content,
  })
}

/// The mock's call-boundary injection hook. Instead of a pump racing the guest,
/// each subscription registers injections that fire the next time the host
/// flushes at a guest call boundary. Deliveries therefore land at a point
/// determined by the config and brain, never by the scheduler.
impl ResourceInjections for MockTooling {
  fn inject_resource_list(
    &self,
    bus: &MessageBus,
    registry: &Arc<CancelRegistry>,
    agent: &str,
    uuid: &str,
  ) {
    let steps = Arc::new(self.resource_list_updates.clone());
    if steps.is_empty() {
      return;
    }
    let state = Arc::clone(&self.state);
    let registry = Arc::clone(registry);
    let agent = agent.to_string();
    let uuid = uuid.to_string();
    let cursor = Arc::new(Mutex::new(0usize));
    bus.register_injection(Injection::new(move |bus, observed, _rt| {
      if !registry.is_open(&uuid) {
        return true;
      }
      loop {
        let index = *lock(&cursor);
        let Some(step) = steps.get(index) else {
          return true;
        };
        if step.pending || !after_ready(&step.after, observed) {
          return false;
        }
        if let Some(error) = &step.error {
          bus.deliver(&agent, &uuid, Event::Error(error.clone()));
        } else {
          lock(&state).resources = step.resources.clone();
          bus.deliver(
            &agent,
            &uuid,
            Event::ResourceListUpdated(step.resources.clone()),
          );
        }
        *lock(&cursor) = index.saturating_add(1);
      }
    }));
  }

  fn inject_resource(
    &self,
    bus: &MessageBus,
    registry: &Arc<CancelRegistry>,
    agent: &str,
    uuid: &str,
    uri: &str,
  ) {
    let steps: Vec<ResourceContentUpdate> = self
      .resource_content_updates
      .iter()
      .filter(|update| update.uri == uri)
      .cloned()
      .collect();
    if steps.is_empty() {
      return;
    }
    let steps = Arc::new(steps);
    let state = Arc::clone(&self.state);
    let registry = Arc::clone(registry);
    let agent = agent.to_string();
    let uuid = uuid.to_string();
    let cursor = Arc::new(Mutex::new(0usize));
    bus.register_injection(Injection::new(move |bus, observed, _rt| {
      if !registry.is_open(&uuid) {
        return true;
      }
      loop {
        let index = *lock(&cursor);
        let Some(step) = steps.get(index) else {
          return true;
        };
        if step.pending || !after_ready(&step.after, observed) {
          return false;
        }
        if let Some(error) = &step.error {
          bus.deliver(&agent, &uuid, Event::Error(error.clone()));
        } else {
          lock(&state)
            .contents
            .insert(step.uri.clone(), step.content.clone());
          let event = content_event(&state, &step.uri, step.content.clone());
          bus.deliver(&agent, &uuid, event);
        }
        *lock(&cursor) = index.saturating_add(1);
      }
    }));
  }

  fn inject_tool_call(
    &self,
    bus: &MessageBus,
    registry: &Arc<CancelRegistry>,
    agent: &str,
    uuid: &str,
    tool: &str,
    args: Value,
  ) {
    // Record the invocation and advance the script cursor synchronously, so
    // queued calls stay ordered and the snapshot reflects what the guest sent.
    let record_index = {
      let mut calls = lock(&self.calls);
      calls.push(RecordedToolCall {
        call: ToolCall {
          name: tool.to_string(),
          arguments: args.clone(),
        },
        result: None,
      });
      calls.len().saturating_sub(1)
    };
    let index = {
      let mut next = lock(&self.tool_call_next);
      let index = *next;
      *next = next.saturating_add(1);
      index
    };
    let step = self.tool_calls.get(index).cloned();
    let arguments = args.to_string();
    let calls = Arc::clone(&self.calls);
    let registry = Arc::clone(registry);
    let agent = agent.to_string();
    let uuid = uuid.to_string();
    let tool = tool.to_string();
    bus.register_injection(Injection::new(move |bus, observed, _rt| {
      if !registry.is_open(&uuid) {
        return true;
      }
      let Some(step) = &step else {
        bus.deliver(
          &agent,
          &uuid,
          Event::Error(format!("mock tool calls exhausted at {tool:?}")),
        );
        return true;
      };
      if step.name != tool {
        bus.deliver(
          &agent,
          &uuid,
          Event::Error(format!(
            "mock expected tool {:?}, got {tool:?}",
            step.name
          )),
        );
        return true;
      }
      if step.pending || !after_ready(&step.after, observed) {
        return false;
      }
      if let Some(error) = &step.error {
        bus.deliver(&agent, &uuid, Event::Error(error.clone()));
        return true;
      }
      let result = ToolCallResult {
        content: text_content(&step.result),
        structured_content: step.structured_content.clone(),
      };
      if let Some(record) = lock(&calls).get_mut(record_index) {
        record.result = Some(result.clone());
      }
      bus.deliver(
        &agent,
        &uuid,
        Event::ToolResult(ToolResult {
          name: tool.clone(),
          arguments: arguments.clone(),
          content: result.content.to_string(),
          structured_content: result.structured_content.map(|v| v.to_string()),
        }),
      );
      true
    }));
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[tokio::test]
  async fn tool_calls_are_name_verified_and_ordered() -> anyhow::Result<()> {
    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({
        "tool_calls": [
          { "name": "echo", "result": "hi" },
          { "name": "add", "result": "3" },
        ],
      }),
      crate::config::Tunables::default(),
    )?;
    assert_eq!(
      tooling
        .call_tool("echo", serde_json::json!({}))
        .await?
        .content,
      text_content("hi")
    );
    assert_eq!(
      tooling
        .call_tool("add", serde_json::json!({}))
        .await?
        .content,
      text_content("3")
    );
    // A name mismatch is an error, not a silent wrong result.
    let error = tooling
      .call_tool("echo", serde_json::json!({}))
      .await
      .expect_err("exhausted script should error");
    assert!(error.to_string().contains("exhausted"), "{error}");
    Ok(())
  }

  #[tokio::test]
  async fn structured_content_is_passed_through() -> anyhow::Result<()> {
    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({
        "tool_calls": [{
          "name": "echo",
          "result": "hi",
          "structured_content": { "echoed": "hi" },
        }],
      }),
      crate::config::Tunables::default(),
    )?;
    let result = tooling.call_tool("echo", serde_json::json!({})).await?;
    assert_eq!(result.content, text_content("hi"));
    assert_eq!(
      result.structured_content,
      Some(serde_json::json!({ "echoed": "hi" }))
    );
    Ok(())
  }

  #[tokio::test]
  async fn a_name_mismatch_is_an_error() -> anyhow::Result<()> {
    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({ "tool_calls": [{ "name": "echo", "result": "hi" }] }),
      crate::config::Tunables::default(),
    )?;
    let error = tooling
      .call_tool("other", serde_json::json!({}))
      .await
      .expect_err("mismatched name should error");
    assert!(error.to_string().contains("expected tool"), "{error}");
    Ok(())
  }

  #[tokio::test]
  async fn an_unscripted_call_is_an_error() -> anyhow::Result<()> {
    let tooling = MockTooling::noop();
    let error = tooling
      .call_tool("echo", serde_json::json!({}))
      .await
      .expect_err("an unscripted call should error");
    assert!(error.to_string().contains("no scripted result"), "{error}");
    Ok(())
  }

  #[tokio::test]
  async fn subscriptions_without_updates_emit_nothing() -> anyhow::Result<()> {
    use futures_util::StreamExt as _;

    let tooling = MockTooling::noop();
    let mut list = tooling.subscribe_resource_list().await?;
    assert!(list.next().await.is_none());
    let mut content = tooling.subscribe_resource("file:///a").await?;
    assert!(content.next().await.is_none());
    Ok(())
  }

  #[tokio::test]
  async fn list_and_content_updates_replay_in_order() -> anyhow::Result<()> {
    use futures_util::StreamExt as _;

    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({
        "initial_resource_list": [
          { "uri": "mem://a", "name": "a" },
        ],
        "initial_resource_contents": { "mem://a": "v0" },
        "resource_list_updates": [
          { "resources": [{ "uri": "mem://a", "name": "a" }] },
          { "resources": [{ "uri": "mem://b", "name": "b" }] },
        ],
        "resource_content_updates": [
          { "uri": "mem://a", "content": "v1" },
          { "uri": "mem://a", "content": "v2" },
        ],
      }),
      crate::config::Tunables::default(),
    )?;

    let mut updates = tooling.subscribe_resource("mem://a").await?;
    assert!(matches!(
      updates.next().await,
      Some(Ok(ResourceNotification::Updated { .. }))
    ));
    assert_eq!(
      tooling.read_resource("mem://a").await?.content,
      "v1",
      "the first update should already be applied"
    );
    assert!(matches!(
      updates.next().await,
      Some(Ok(ResourceNotification::Updated { .. }))
    ));
    assert_eq!(tooling.read_resource("mem://a").await?.content, "v2");
    assert!(updates.next().await.is_none());

    let mut list = tooling.subscribe_resource_list().await?;
    assert!(matches!(
      list.next().await,
      Some(Ok(ResourceNotification::ListChanged))
    ));
    assert!(matches!(
      list.next().await,
      Some(Ok(ResourceNotification::ListChanged))
    ));
    assert_eq!(tooling.list_resources().await?[0].uri, "mem://b");
    assert!(list.next().await.is_none());
    Ok(())
  }

  #[tokio::test]
  async fn snapshot_reports_consumed_and_remaining_tool_calls()
  -> anyhow::Result<()> {
    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({
        "tool_calls": [
          { "name": "echo", "result": "hi" },
          { "name": "add", "result": "3" },
        ],
      }),
      crate::config::Tunables::default(),
    )?;
    let snapshot = tooling
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["tool_calls_total"], 2);
    assert_eq!(snapshot["tool_calls_consumed"], 0);

    tooling.call_tool("echo", serde_json::json!({})).await?;
    let snapshot = tooling
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["tool_calls_consumed"], 1);
    assert_eq!(snapshot["tool_calls_remaining"], 1);
    assert_eq!(snapshot["calls"][0]["name"], "echo");
    assert_eq!(snapshot["calls"][0]["result"]["content"][0]["text"], "hi");
    Ok(())
  }

  #[tokio::test]
  async fn snapshot_records_a_failed_call_without_a_result()
  -> anyhow::Result<()> {
    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({
        "tool_calls": [{ "name": "echo", "result": "hi" }],
      }),
      crate::config::Tunables::default(),
    )?;
    let error = tooling
      .call_tool("wrong", serde_json::json!({}))
      .await
      .expect_err("a name mismatch is an error");
    assert!(error.to_string().contains("expected tool"), "{error}");
    let snapshot = tooling
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["calls"][0]["name"], "wrong");
    assert!(snapshot["calls"][0]["result"].is_null());
    Ok(())
  }

  #[tokio::test]
  async fn a_scripted_tool_error_is_delivered() -> anyhow::Result<()> {
    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({
        "tool_calls": [{ "name": "echo", "error": "boom" }],
      }),
      crate::config::Tunables::default(),
    )?;
    let error = tooling
      .call_tool("echo", serde_json::json!({}))
      .await
      .expect_err("a scripted error should be delivered");
    assert_eq!(error.to_string(), "boom");
    let snapshot = tooling
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["calls"][0]["name"], "echo");
    assert!(snapshot["calls"][0]["result"].is_null());
    Ok(())
  }

  #[tokio::test]
  async fn a_pending_tool_call_never_completes() -> anyhow::Result<()> {
    let tooling = MockTooling::build(
      "t",
      &serde_json::json!({
        "tool_calls": [{ "name": "echo", "pending": true }],
      }),
      crate::config::Tunables::default(),
    )?;
    let call = tooling.call_tool("echo", serde_json::json!({}));
    assert!(
      tokio::time::timeout(Duration::from_millis(50), call)
        .await
        .is_err(),
      "a pending tool call should never complete"
    );
    Ok(())
  }

  #[tokio::test]
  async fn resource_steps_can_error_or_pend() -> anyhow::Result<()> {
    use futures_util::StreamExt as _;

    let erroring = MockTooling::build(
      "t",
      &serde_json::json!({
        "resource_list_updates": [{ "resources": [], "error": "boom" }],
        "resource_content_updates": [
          { "uri": "mem://a", "content": "x", "error": "boom" },
        ],
      }),
      crate::config::Tunables::default(),
    )?;
    let mut list = erroring.subscribe_resource_list().await?;
    assert_eq!(list.next().await, Some(Err("boom".to_string())));
    let mut content = erroring.subscribe_resource("mem://a").await?;
    assert_eq!(content.next().await, Some(Err("boom".to_string())));

    let pending = MockTooling::build(
      "t",
      &serde_json::json!({
        "resource_list_updates": [{ "resources": [], "pending": true }],
      }),
      crate::config::Tunables::default(),
    )?;
    let mut list = pending.subscribe_resource_list().await?;
    assert!(
      tokio::time::timeout(Duration::from_millis(50), list.next())
        .await
        .is_err(),
      "a pending resource step should never yield"
    );
    Ok(())
  }
}
