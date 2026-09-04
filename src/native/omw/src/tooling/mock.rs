//! An in-memory scripted tooling for tests and local development.
//!
//! `list-tools` returns the configured tools; `call-tool` returns the
//! configured canned result and records the invocation (name and arguments) so
//! tests can assert what the guest actually sent. Resources behave the same:
//! `list-resources` returns the configured resources; each `subscribe-*` returns a
//! stream that auto-notifies once shortly after subscribing (so synchronous scripts
//! work without a driver task).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};

use super::{
  ResourceContent, ResourceInfo, ResourceNotification, Tool, Tooling,
  ToolingEntry,
};

/// A notification sender registered on a mock subscription.
type NotificationSender =
  mpsc::UnboundedSender<Result<ResourceNotification, String>>;

/// Impl-specific configuration for the mock tooling.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
  #[serde(default)]
  pub tools: Vec<Tool>,

  /// The canned result every `call-tool` returns.
  #[serde(default)]
  pub result: String,

  #[serde(default)]
  pub resources: Vec<ResourceInfo>,

  /// Canned resource content keyed by URI, returned by `read-resource`.
  #[serde(default)]
  pub resource_contents: HashMap<String, String>,
}

/// A single recorded `call-tool` invocation.
#[derive(Debug, Clone)]
pub struct ToolCall {
  pub name: String,
  pub arguments: Value,
}

/// A scripted tooling backed by in-memory state. Each subscription registers a
/// sender and auto-notifies the subscriber once shortly after subscribing, so
/// tests and scripts recv an event without an external driver.
pub struct MockTooling {
  tools: Vec<Tool>,
  result: String,
  resources: Vec<ResourceInfo>,
  resource_contents: HashMap<String, String>,
  calls: Arc<Mutex<Vec<ToolCall>>>,
  resource_subs: Arc<Mutex<HashMap<String, Vec<NotificationSender>>>>,
  resource_list_subs: Arc<Mutex<Vec<NotificationSender>>>,
}

/// Build a `mock` tooling from its opaque config params.
pub fn build(name: &str, params: &Value) -> anyhow::Result<ToolingEntry> {
  let config = Config::deserialize(params)
    .with_context(|| format!("invalid mock tooling config for {name:?}"))?;
  Ok(ToolingEntry {
    name: name.to_string(),
    kind: MockTooling::kind(),
    tooling: Arc::new(MockTooling {
      tools: config.tools,
      result: config.result,
      resources: config.resources,
      resource_contents: config.resource_contents,
      calls: Arc::new(Mutex::new(Vec::new())),
      resource_subs: Arc::new(Mutex::new(HashMap::new())),
      resource_list_subs: Arc::new(Mutex::new(Vec::new())),
    }),
  })
}

impl MockTooling {
  /// A bare mock with no tools (for tests that only assert the call).
  pub fn noop() -> Arc<Self> {
    Arc::new(Self {
      tools: Vec::new(),
      result: String::new(),
      resources: Vec::new(),
      resource_contents: HashMap::new(),
      calls: Arc::new(Mutex::new(Vec::new())),
      resource_subs: Arc::new(Mutex::new(HashMap::new())),
      resource_list_subs: Arc::new(Mutex::new(Vec::new())),
    })
  }

  /// A mock exposing the given resources (for tests that assert the lists).
  pub fn with_resources(resources: Vec<ResourceInfo>) -> Arc<Self> {
    Arc::new(Self {
      tools: Vec::new(),
      result: String::new(),
      resources,
      resource_contents: HashMap::new(),
      calls: Arc::new(Mutex::new(Vec::new())),
      resource_subs: Arc::new(Mutex::new(HashMap::new())),
      resource_list_subs: Arc::new(Mutex::new(Vec::new())),
    })
  }

  /// A mock exposing one resource whose `read-resource` returns `content`.
  pub fn with_resource_content(uri: &str, content: &str) -> Arc<Self> {
    let resources = vec![ResourceInfo {
      uri: uri.to_string(),
      name: uri.to_string(),
      description: None,
      mime_type: Some("text/plain".to_string()),
    }];
    let mut resource_contents = HashMap::new();
    resource_contents.insert(uri.to_string(), content.to_string());
    Arc::new(Self {
      tools: Vec::new(),
      result: String::new(),
      resources,
      resource_contents,
      calls: Arc::new(Mutex::new(Vec::new())),
      resource_subs: Arc::new(Mutex::new(HashMap::new())),
      resource_list_subs: Arc::new(Mutex::new(Vec::new())),
    })
  }

  /// Recorded tool calls, in order.
  pub async fn calls(&self) -> Vec<ToolCall> {
    self.calls.lock().await.clone()
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
    Ok(self.result.clone())
  }

  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>> {
    Ok(self.resources.clone())
  }

  async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContent> {
    let content = self
      .resource_contents
      .get(uri)
      .cloned()
      .with_context(|| format!("mock has no content for resource {uri:?}"))?;
    let mime_type = self
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
    let (tx, rx) =
      mpsc::unbounded_channel::<Result<ResourceNotification, String>>();
    self.resource_list_subs.lock().await.push(tx.clone());
    std::thread::spawn(move || {
      std::thread::sleep(Duration::from_millis(10));
      let _ = tx.send(Ok(ResourceNotification::ListChanged));
    });
    Ok(subscription_stream(rx))
  }

  async fn subscribe_resource(
    &self,
    uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    let (tx, rx) =
      mpsc::unbounded_channel::<Result<ResourceNotification, String>>();
    self
      .resource_subs
      .lock()
      .await
      .entry(uri.to_string())
      .or_default()
      .push(tx.clone());
    let uri = uri.to_string();
    std::thread::spawn(move || {
      std::thread::sleep(Duration::from_millis(10));
      let _ = tx.send(Ok(ResourceNotification::Updated { uri }));
    });
    Ok(subscription_stream(rx))
  }
}

/// Drain an mpsc receiver into a `BoxStream` of notifications. Dropping the
/// returned stream drops the receiver, which cancels the feeding task.
fn subscription_stream(
  rx: mpsc::UnboundedReceiver<Result<ResourceNotification, String>>,
) -> BoxStream<'static, Result<ResourceNotification, String>> {
  Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
    rx.recv().await.map(|item| (item, rx))
  }))
}
