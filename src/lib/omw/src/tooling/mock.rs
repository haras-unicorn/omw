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
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use super::{
  Factory, ResourceContent, ResourceInfo, ResourceNotification, Tool, Tooling,
};
use crate::host::trace::TraceSender;
use crate::testing::{After, TraceLog};

fn default_delay_ms() -> u64 {
  10
}

/// Impl-specific configuration for the mock tooling.
#[derive(Debug, Clone, Deserialize)]
struct Config {
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

/// One scripted `call-tool` result.
#[derive(Debug, Clone, Deserialize)]
struct ScriptedToolCall {
  name: String,
  #[serde(default)]
  result: String,
  #[serde(default)]
  after: After,
}

/// One scripted, full-replacement resource-list update.
#[derive(Debug, Clone, Deserialize)]
struct ResourceListUpdate {
  resources: Vec<ResourceInfo>,
  #[serde(default)]
  after: After,
}

/// One scripted, one-by-one resource-content update.
#[derive(Debug, Clone, Deserialize)]
struct ResourceContentUpdate {
  uri: String,
  content: String,
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

/// A scripted tooling whose calls never complete, for testing cancellation:
/// `call-tool` awaits a pending future and each subscription returns a
/// pending stream, so a pump can only exit via cancel. This makes
/// cancel-suppresses-delivery tests deterministic (no race between an
/// immediately-completing call and `cancel`).
pub struct PendingTooling;

#[async_trait::async_trait]
impl Tooling for PendingTooling {
  fn kind() -> &'static str {
    "mock"
  }

  async fn list_tools(&self) -> anyhow::Result<Vec<Tool>> {
    Ok(Vec::new())
  }

  async fn call_tool(
    &self,
    _name: &str,
    _args: Value,
  ) -> anyhow::Result<String> {
    std::future::pending().await
  }

  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>> {
    Ok(Vec::new())
  }

  async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContent> {
    anyhow::bail!("mock has no content for resource {uri:?}")
  }

  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    Ok(Box::pin(futures_util::stream::pending()))
  }

  async fn subscribe_resource(
    &self,
    _uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    Ok(Box::pin(futures_util::stream::pending()))
  }
}

/// A scripted tooling that fails every call, for testing error paths.
pub struct FailingTooling;

#[async_trait::async_trait]
impl Tooling for FailingTooling {
  fn kind() -> &'static str {
    "mock"
  }

  async fn list_tools(&self) -> anyhow::Result<Vec<Tool>> {
    Ok(Vec::new())
  }

  async fn call_tool(
    &self,
    _name: &str,
    _args: Value,
  ) -> anyhow::Result<String> {
    anyhow::bail!("mock failure")
  }

  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>> {
    Ok(Vec::new())
  }

  async fn read_resource(&self, _uri: &str) -> anyhow::Result<ResourceContent> {
    anyhow::bail!("mock failure")
  }

  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    anyhow::bail!("mock failure")
  }

  async fn subscribe_resource(
    &self,
    _uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    anyhow::bail!("mock failure")
  }
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
  calls: Arc<Mutex<Vec<ToolCall>>>,
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
    self.calls.lock().await.clone()
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

  async fn call_tool(&self, name: &str, args: Value) -> anyhow::Result<String> {
    self.calls.lock().await.push(ToolCall {
      name: name.to_string(),
      arguments: args,
    });
    if self.tool_calls.is_empty() {
      anyhow::bail!("mock has no scripted result for tool {name:?}");
    }
    let index = {
      let mut next = self.tool_call_next.lock().await;
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
    Ok(step.result)
  }

  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>> {
    Ok(self.state.lock().await.resources.clone())
  }

  async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContent> {
    let state = self.state.lock().await;
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
          state.lock().await.resources = step.resources;
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
          state
            .lock()
            .await
            .contents
            .insert(step.uri.clone(), step.content);
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
      tooling.call_tool("echo", serde_json::json!({})).await?,
      "hi"
    );
    assert_eq!(tooling.call_tool("add", serde_json::json!({})).await?, "3");
    // A name mismatch is an error, not a silent wrong result.
    let error = tooling
      .call_tool("echo", serde_json::json!({}))
      .await
      .expect_err("exhausted script should error");
    assert!(error.to_string().contains("exhausted"), "{error}");
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
}
