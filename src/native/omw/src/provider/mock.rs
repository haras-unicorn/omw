//! An in-memory scripted provider for tests and local development.
//!
//! Every `chat` returns the configured canned responses as a single-streamed
//! delta each, and records the call (model, messages, tools) so tests can
//! assert what the guest actually sent.

use std::sync::Arc;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use super::{ChatDelta, ChatMessage, Provider, ProviderEntry};
use crate::tooling::Tool;

/// Impl-specific configuration for the mock provider.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
  /// Each string is emitted as one content delta per `chat`.
  #[serde(default)]
  pub responses: Vec<String>,
}

/// A single recorded `chat` invocation.
#[derive(Debug, Clone)]
pub struct ChatCall {
  pub model: String,
  pub messages: Vec<ChatMessage>,
  pub tools: Vec<Tool>,
}

/// A scripted provider backed by an in-memory response queue.
pub struct MockProvider {
  responses: Vec<String>,
  calls: Arc<Mutex<Vec<ChatCall>>>,
}

/// Build a `mock` provider from its opaque config params.
pub fn build(name: &str, params: &Value) -> anyhow::Result<ProviderEntry> {
  let config = Config::deserialize(params)
    .with_context(|| format!("invalid mock provider config for {name:?}"))?;
  Ok(ProviderEntry {
    name: name.to_string(),
    kind: MockProvider::kind(),
    provider: Arc::new(MockProvider {
      responses: config.responses,
      calls: Arc::new(Mutex::new(Vec::new())),
    }),
  })
}

impl MockProvider {
  /// A bare mock with no responses (for tests that only assert the call).
  pub fn noop() -> Arc<Self> {
    Arc::new(Self {
      responses: Vec::new(),
      calls: Arc::new(Mutex::new(Vec::new())),
    })
  }

  /// Recorded chat calls, in order.
  pub async fn calls(&self) -> Vec<ChatCall> {
    self.calls.lock().await.clone()
  }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
  fn kind() -> &'static str {
    "mock"
  }

  async fn models(&self) -> Vec<String> {
    vec!["mock-model".to_string()]
  }

  async fn chat(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> anyhow::Result<BoxStream<'static, Result<ChatDelta, String>>> {
    self.calls.lock().await.push(ChatCall {
      model: model.to_string(),
      messages,
      tools,
    });
    let responses = self.responses.clone();
    Ok(Box::pin(futures_util::stream::iter(
      responses.into_iter().map(|content| {
        Ok(ChatDelta {
          content: Some(content),
          tool_call: None,
          finish_reason: None,
        })
      }),
    )))
  }
}
