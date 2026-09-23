//! Custom provider: a scripted `EchoProvider` that streams canned
//! responses and records what the caller sent.
//!
//! Library-only teaching material (brains use built-in kinds, not this).
//! The inline TOML wires `[providers.echo] kind = "echo"`; the example
//! builds the `Config` programmatically, registers the back end with
//! `register_providers!`, then runs `chat`/`chat_stream` and asserts on
//! the recorded model/messages before exiting 0.

use std::sync::Arc;

use futures_util::stream::BoxStream;
use omw::prelude::*;
use serde::Deserialize;
use tokio::sync::Mutex;

/// Impl-specific config: each string is one content delta per `chat`.
#[derive(Debug, Clone, Deserialize)]
struct EchoConfig {
  #[serde(default)]
  responses: Vec<String>,
}

/// One recorded `chat_stream` invocation.
#[derive(Debug, Clone)]
struct EchoCall {
  model: String,
  messages: Vec<ChatMessage>,
  tools: Vec<Tool>,
}

/// A scripted provider backed by an in-memory response list.
struct EchoProvider {
  responses: Vec<String>,
  calls: Arc<Mutex<Vec<EchoCall>>>,
}

impl omw::provider::Factory for EchoProvider {
  fn build(
    _name: &str,
    params: &serde_json::Value,
  ) -> anyhow::Result<Arc<Self>> {
    let config: EchoConfig = serde_json::from_value(params.clone())?;
    Ok(Arc::new(Self {
      responses: config.responses,
      calls: Arc::new(Mutex::new(Vec::new())),
    }))
  }
}

impl EchoProvider {
  async fn calls(&self) -> Vec<EchoCall> {
    self.calls.lock().await.clone()
  }
}

#[async_trait::async_trait]
impl Provider for EchoProvider {
  fn kind() -> &'static str {
    "echo"
  }

  async fn list_models(&self) -> Vec<String> {
    vec!["echo-model".to_string()]
  }

  async fn chat_stream(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> anyhow::Result<BoxStream<'static, Result<ChatDelta, String>>> {
    self.calls.lock().await.push(EchoCall {
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let raw = r#"
[providers.echo]
kind = "echo"
responses = ["hello from echo"]
"#;
  let cfg: Config = toml::from_str(raw)?;
  let mut registries = Registries::new();
  omw::register_providers!(registries.providers, EchoProvider);
  let entries = registries.providers.build_entries(&cfg)?;
  let entry = entries
    .get("echo")
    .ok_or_else(|| anyhow::anyhow!("missing provider \"echo\""))?;
  anyhow::ensure!(
    entry.kind() == EchoProvider::kind(),
    "expected kind {:?}, got {:?}",
    EchoProvider::kind(),
    entry.kind()
  );

  let params = cfg
    .providers
    .get("echo")
    .map(|c| c.params.clone())
    .ok_or_else(|| anyhow::anyhow!("missing provider \"echo\""))?;
  let provider =
    <EchoProvider as omw::provider::Factory>::build("echo", &params)?;

  let messages = vec![ChatMessage {
    role: Role::User,
    content: Some("say hi".to_string()),
    tool_call: None,
  }];
  let result = provider
    .chat("echo-model", messages.clone(), Vec::new())
    .await?;
  anyhow::ensure!(
    result.content.as_deref() == Some("hello from echo"),
    "unexpected chat content: {result:?}"
  );

  use futures_util::StreamExt as _;
  let mut stream = provider
    .chat_stream("echo-model", messages.clone(), Vec::new())
    .await?;
  let mut seen = String::new();
  while let Some(delta) = stream.next().await {
    let delta = delta.map_err(anyhow::Error::msg)?;
    if let Some(chunk) = delta.content {
      seen.push_str(&chunk);
    }
  }
  anyhow::ensure!(
    seen == "hello from echo",
    "unexpected stream content: {seen:?}"
  );

  let calls = provider.calls().await;
  anyhow::ensure!(calls.len() == 2, "expected 2 calls, got {}", calls.len());
  for call in &calls {
    anyhow::ensure!(
      call.model == "echo-model",
      "unexpected model: {:?}",
      call.model
    );
    anyhow::ensure!(
      call.messages == messages,
      "unexpected messages: {:?}",
      call.messages
    );
    anyhow::ensure!(
      call.tools.is_empty(),
      "unexpected tools: {:?}",
      call.tools
    );
  }
  Ok(())
}
