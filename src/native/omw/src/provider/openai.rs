//! OpenAI-compatible provider over `reqwest`, always streaming SSE deltas.
//!
//! Cancellation is by dropping the returned stream: dropping it drops the
//! in-flight `reqwest` response and closes the connection.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::Context as _;
use futures_util::stream::{BoxStream, Stream, StreamExt};
use serde::Deserialize;
use serde_json::Value;

use super::{ChatDelta, ChatMessage, Provider, ProviderEntry, Role, ToolCall};
use crate::tooling::Tool;

/// Impl-specific configuration for the OpenAI-family provider.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
  #[serde(default)]
  pub base_url: Option<String>,
  #[serde(default)]
  pub api_key: Option<String>,
  #[serde(default)]
  pub model: Option<String>,
}

impl Config {
  /// A debug-friendly view with `api_key` redacted, so it can be logged
  /// without leaking the secret.
  fn debug_redacted(&self) -> String {
    serde_json::json!({
      "base_url": self.base_url,
      "api_key": self.api_key.as_ref().map(|_| "<redacted>"),
      "model": self.model,
    })
    .to_string()
  }
}

/// An OpenAI-compatible chat provider backed by `reqwest`.
pub struct OpenAIProvider {
  config: Config,
  client: reqwest::Client,
}

/// Build an `openai` provider from its opaque config params.
pub fn build(name: &str, params: &Value) -> anyhow::Result<ProviderEntry> {
  let config = Config::deserialize(params)
    .with_context(|| format!("invalid openai provider config for {name:?}"))?;
  tracing::debug!(name, config = %config.debug_redacted(), "built openai provider");
  Ok(ProviderEntry {
    name: name.to_string(),
    kind: OpenAIProvider::kind(),
    provider: Arc::new(OpenAIProvider::new(config)?),
  })
}

impl OpenAIProvider {
  pub fn new(config: Config) -> anyhow::Result<Self> {
    let client = reqwest::Client::builder()
      .build()
      .context("failed to build http client")?;
    Ok(Self { config, client })
  }
}

#[async_trait::async_trait]
impl Provider for OpenAIProvider {
  fn kind() -> &'static str {
    "openai"
  }

  async fn models(&self) -> Vec<String> {
    self
      .config
      .model
      .as_ref()
      .map(|m| vec![m.clone()])
      .unwrap_or_default()
  }

  async fn chat(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> anyhow::Result<BoxStream<'static, Result<ChatDelta, String>>> {
    tracing::debug!(
      model,
      n_messages = messages.len(),
      n_tools = tools.len(),
      "openai chat request"
    );
    let url = format!(
      "{}/chat/completions",
      self
        .config
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com/v1")
        .trim_end_matches('/')
    );
    let mut body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": messages.iter().map(to_wire_message).collect::<Vec<_>>(),
    });
    if !tools.is_empty() {
      body["tools"] =
        serde_json::json!(tools.iter().map(to_wire_tool).collect::<Vec<_>>());
    }

    let mut request = self.client.post(&url);
    if let Some(api_key) = &self.config.api_key {
      request = request.bearer_auth(api_key);
    }
    let resp = request
      .json(&body)
      .send()
      .await
      .context("failed to send chat request")?;

    if !resp.status().is_success() {
      let status = resp.status();
      let text = resp.text().await.unwrap_or_default();
      tracing::error!(
        model,
        status = %status,
        body = %text,
        "openai chat request failed with a non-2xx status"
      );
      anyhow::bail!("chat request failed with status {status}: {text}");
    }

    let byte_stream = resp.bytes_stream().map(|r| {
      r.map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| e.to_string())
    });
    Ok(Box::pin(SseDeltas::new(byte_stream)))
  }
}

fn to_wire_tool(tool: &Tool) -> serde_json::Value {
  let mut function = serde_json::json!({
      "name": tool.name,
      "parameters": tool.input_schema,
  });
  if let Some(description) = &tool.description {
    function["description"] = serde_json::Value::String(description.clone());
  }
  serde_json::json!({
      "type": "function",
      "function": function,
  })
}

fn to_wire_message(msg: &ChatMessage) -> serde_json::Value {
  let role = match msg.role {
    Role::System => "system",
    Role::User => "user",
    Role::Assistant => "assistant",
    Role::Tool => "tool",
  };
  let mut m = serde_json::json!({ "role": role });
  if let Some(content) = &msg.content {
    m["content"] = serde_json::Value::String(content.clone());
  }
  if let Some(tool_call) = &msg.tool_call {
    let tc = serde_json::json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.arguments,
        },
    });
    m["tool_calls"] = serde_json::json!([tc]);
  }
  m
}

/// A single SSE data payload from the wire, with only the fields we consume.
#[derive(Debug, Deserialize)]
struct WireChunk {
  choices: Vec<WireChoice>,
}

#[derive(Debug, Deserialize)]
struct WireChoice {
  delta: WireDelta,
  #[serde(default)]
  finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct WireDelta {
  #[serde(default)]
  content: Option<String>,
  #[serde(default)]
  tool_calls: Vec<WireToolCall>,
}

#[derive(Debug, Deserialize)]
struct WireToolCall {
  index: Option<u64>,
  #[serde(default)]
  id: Option<String>,
  #[serde(default)]
  function: Option<WireFunction>,
}

#[derive(Debug, Deserialize)]
struct WireFunction {
  #[serde(default)]
  name: Option<String>,
  #[serde(default)]
  arguments: Option<String>,
}

/// Decodes SSE `data:` lines into [`ChatDelta`]s, reassembling tool calls
/// (whose `arguments` may be fragmented across many chunks) per tool index.
struct SseDeltas<S> {
  inner: S,
  buffer: String,
  /// Accumulated tool-call fragments, keyed by tool index. `BTreeMap` keeps
  /// iteration order deterministic (lowest index first) across chunks.
  tool_calls: BTreeMap<u64, (String, String, String)>,
}

impl<S: Stream<Item = Result<String, String>> + Unpin> SseDeltas<S> {
  fn new(inner: S) -> Self {
    Self {
      inner,
      buffer: String::new(),
      tool_calls: BTreeMap::new(),
    }
  }

  fn take_line(&mut self) -> Option<String> {
    let idx = self.buffer.find('\n')?;
    let (head, tail) = self.buffer.split_at(idx);
    let line = head.to_string();
    self.buffer = tail[1..].to_string();
    Some(line)
  }

  fn decode_chunk(&mut self, chunk: &WireChunk) -> Option<ChatDelta> {
    let choice = chunk.choices.first()?;
    let content = choice.delta.content.clone();
    let mut tool_call = None;

    for tc in &choice.delta.tool_calls {
      let index = tc.index.unwrap_or(0);
      let entry = self.tool_calls.entry(index).or_insert_with(|| {
        (
          tc.id.clone().unwrap_or_default(),
          tc.function
            .as_ref()
            .and_then(|f| f.name.clone())
            .unwrap_or_default(),
          String::new(),
        )
      });
      if let Some(name) = tc.function.as_ref().and_then(|f| f.name.clone()) {
        entry.1 = name;
      }
      if let Some(args) = tc.function.as_ref().and_then(|f| f.arguments.clone())
      {
        entry.2.push_str(&args);
      }
    }

    // Surface a partially-reassembled tool call once its name is known.
    if tool_call.is_none() {
      tool_call = self.tool_calls.values().find_map(|(id, name, args)| {
        if id.is_empty() || name.is_empty() {
          None
        } else {
          Some(ToolCall {
            id: id.clone(),
            name: name.clone(),
            arguments: args.clone(),
          })
        }
      });
    }

    tracing::trace!(
      content_len = content.as_ref().map_or(0, String::len),
      tool_call = tool_call.as_ref().map(|t| t.name.as_str()),
      finish_reason = choice.finish_reason.as_deref(),
      "openai chat delta"
    );
    Some(ChatDelta {
      content,
      tool_call,
      finish_reason: choice.finish_reason.clone(),
    })
  }

  /// Emit a final delta carrying the lowest-index accumulated tool call.
  fn flush_tool_calls(&mut self) -> Option<ChatDelta> {
    let (id, name, args) = self.tool_calls.pop_first()?.1;
    if id.is_empty() && name.is_empty() {
      return None;
    }
    Some(ChatDelta {
      content: None,
      tool_call: Some(ToolCall {
        id,
        name,
        arguments: args,
      }),
      finish_reason: None,
    })
  }
}

impl<S: Stream<Item = Result<String, String>> + Unpin> Stream for SseDeltas<S> {
  type Item = Result<ChatDelta, String>;

  fn poll_next(
    mut self: Pin<&mut Self>,
    cx: &mut Context<'_>,
  ) -> Poll<Option<Self::Item>> {
    let this = &mut *self;
    loop {
      if let Some(line) = this.take_line() {
        if let Some(data) = line.strip_prefix("data:") {
          let data = data.trim();
          if data == "[DONE]" {
            let tail = this.flush_tool_calls();
            return if let Some(ev) = tail {
              Poll::Ready(Some(Ok(ev)))
            } else {
              Poll::Ready(None)
            };
          }
          match serde_json::from_str::<WireChunk>(data) {
            Ok(chunk) => {
              if let Some(ev) = this.decode_chunk(&chunk) {
                return Poll::Ready(Some(Ok(ev)));
              }
            }
            Err(_) => {
              return Poll::Ready(Some(Err("malformed SSE delta".into())));
            }
          }
        }
        continue;
      }
      // Buffer holds no complete line; pull more bytes from the wire.
      let inner = Pin::new(&mut this.inner);
      match inner.poll_next(cx) {
        Poll::Ready(Some(Ok(chunk))) => {
          this.buffer.push_str(&chunk);
          continue;
        }
        Poll::Ready(Some(Err(_))) => {
          let tail = this.flush_tool_calls();
          return Poll::Ready(tail.map(Ok));
        }
        Poll::Ready(None) => {
          // Drain a final buffered line that ended without a newline.
          if let Some(ev) = this.decoded_rest() {
            return Poll::Ready(Some(Ok(ev)));
          }
          return Poll::Ready(None);
        }
        Poll::Pending => return Poll::Pending,
      }
    }
  }
}

impl<S: Stream<Item = Result<String, String>> + Unpin> SseDeltas<S> {
  fn decoded_rest(&mut self) -> Option<ChatDelta> {
    if self.buffer.is_empty() {
      return self.flush_tool_calls();
    }
    let line = std::mem::take(&mut self.buffer);
    if let Some(data) = line.strip_prefix("data:").map(str::trim) {
      if data == "[DONE]" {
        return self.flush_tool_calls();
      }
      if let Ok(chunk) = serde_json::from_str::<WireChunk>(data) {
        return self.decode_chunk(&chunk);
      }
    }
    self.flush_tool_calls()
  }
}

#[cfg(test)]
mod tests {
  use futures_util::StreamExt;

  use super::*;
  use crate::provider::{Role, ToolCall};
  use crate::tooling::Tool;

  /// Build a single SSE `data:` line, newline-terminated.
  fn data(json: &str) -> String {
    format!("data: {json}\n\n")
  }

  /// Feed one `body` chunk through `SseDeltas` and collect every delta.
  fn collect_deltas(body: &str) -> anyhow::Result<Vec<ChatDelta>> {
    let stream =
      futures_util::stream::iter(vec![Ok::<_, String>(body.to_string())]);
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()?;
    let results: Vec<Result<ChatDelta, String>> =
      rt.block_on(SseDeltas::new(stream).collect());
    let deltas = results
      .into_iter()
      .collect::<Result<Vec<_>, _>>()
      .map_err(anyhow::Error::msg)?;
    Ok(deltas)
  }

  #[test]
  fn to_wire_message_maps_roles() {
    for (role, expected) in [
      (Role::System, "system"),
      (Role::User, "user"),
      (Role::Assistant, "assistant"),
      (Role::Tool, "tool"),
    ] {
      let msg = ChatMessage {
        role,
        content: Some("hi".to_string()),
        tool_call: None,
      };
      let wire = to_wire_message(&msg);
      assert_eq!(wire["role"], expected);
      assert_eq!(wire["content"], "hi");
      assert!(wire.get("tool_calls").is_none());
    }
  }

  #[test]
  fn to_wire_message_without_content_omits_the_key() {
    let msg = ChatMessage {
      role: Role::User,
      content: None,
      tool_call: None,
    };
    let wire = to_wire_message(&msg);
    assert!(wire.get("content").is_none());
  }

  #[test]
  fn to_wire_message_serializes_tool_call() -> anyhow::Result<()> {
    let msg = ChatMessage {
      role: Role::Assistant,
      content: None,
      tool_call: Some(ToolCall {
        id: "call_1".to_string(),
        name: "get_weather".to_string(),
        arguments: r#"{"city":"Paris"}"#.to_string(),
      }),
    };
    let wire = to_wire_message(&msg);
    let calls = wire["tool_calls"]
      .as_array()
      .ok_or_else(|| anyhow::anyhow!("missing tool_calls"))?;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["type"], "function");
    assert_eq!(calls[0]["id"], "call_1");
    assert_eq!(calls[0]["function"]["name"], "get_weather");
    assert_eq!(calls[0]["function"]["arguments"], r#"{"city":"Paris"}"#);
    Ok(())
  }

  #[test]
  fn to_wire_tool_serializes_schema_and_optional_description() {
    let tool = Tool {
      name: "weather".to_string(),
      description: Some("gets the weather".to_string()),
      input_schema: serde_json::json!({ "type": "object" }),
    };
    let wire = to_wire_tool(&tool);
    assert_eq!(wire["type"], "function");
    assert_eq!(wire["function"]["name"], "weather");
    assert_eq!(wire["function"]["description"], "gets the weather");
    assert_eq!(
      wire["function"]["parameters"],
      serde_json::json!({ "type": "object" })
    );

    let bare = Tool {
      name: "bare".to_string(),
      description: None,
      input_schema: serde_json::json!({}),
    };
    let wire = to_wire_tool(&bare);
    assert!(wire["function"].get("description").is_none());
  }

  #[test]
  fn sse_content_deltas_stream_in_order() -> anyhow::Result<()> {
    let body = format!(
      "{}data: [DONE]\n\n",
      [
        data(r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#),
        data(r#"{"choices":[{"delta":{"content":", world"},"finish_reason":null}]}"#),
      ]
      .concat()
    );
    let deltas = collect_deltas(&body)?;
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].content.as_deref(), Some("Hello"));
    assert_eq!(deltas[1].content.as_deref(), Some(", world"));
    assert!(deltas[0].tool_call.is_none());
    assert_eq!(deltas[0].finish_reason, None);
    Ok(())
  }

  #[test]
  fn sse_done_terminates_and_flushes_pending_tool_call() -> anyhow::Result<()> {
    let body = format!(
      "{}data: [DONE]\n\n",
      data(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"n","arguments":"{}"}}]},"finish_reason":null}]}"#
      )
    );
    let deltas = collect_deltas(&body)?;
    assert_eq!(deltas.len(), 2);
    for delta in &deltas {
      let tc = delta
        .tool_call
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing tool call"))?;
      assert_eq!(tc.id, "c");
      assert_eq!(tc.name, "n");
      assert_eq!(tc.arguments, "{}");
    }
    Ok(())
  }

  #[test]
  fn sse_fragmented_tool_call_arguments_reassemble() -> anyhow::Result<()> {
    let body = format!(
      "{}data: [DONE]\n\n",
      [
        data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#),
        data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\":"}}]},"finish_reason":null}]}"#),
        data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Paris\"}"}}]},"finish_reason":null}]}"#),
      ]
      .concat()
    );
    let deltas = collect_deltas(&body)?;
    assert_eq!(deltas.len(), 4);
    let args: Vec<Option<String>> = deltas
      .iter()
      .map(|d| d.tool_call.as_ref().map(|t| t.arguments.clone()))
      .collect();
    assert_eq!(args[0].as_deref(), Some(""));
    assert_eq!(args[1].as_deref(), Some(r#"{"city":"#));
    assert_eq!(args[2].as_deref(), Some(r#"{"city":"Paris"}"#));
    // The final `[DONE]` flush carries the fully reassembled arguments.
    assert_eq!(args[3].as_deref(), Some(r#"{"city":"Paris"}"#));
    Ok(())
  }

  #[test]
  fn sse_multiple_tool_calls_surface_lowest_index_first() -> anyhow::Result<()>
  {
    // Index 1 completes before index 0; once both are present, the lowest
    // index wins (BTreeMap determinism, not HashMap random order).
    let body = format!(
      "{}data: [DONE]\n\n",
      [
        data(r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c1","function":{"name":"n1","arguments":"{}"}}]},"finish_reason":null}]}"#),
        data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c0","function":{"name":"n0","arguments":"{}"}}]},"finish_reason":null}]}"#),
      ]
      .concat()
    );
    let deltas = collect_deltas(&body)?;
    // In-chunk surfacing picks index 1 while it is the only known call, then
    // flips to the lowest index once both are present; the `[DONE]` flush and
    // the end-of-stream flush each pop the lowest remaining index (0, then 1).
    assert_eq!(deltas.len(), 4);
    let ids: Vec<Option<String>> = deltas
      .iter()
      .map(|d| d.tool_call.as_ref().map(|t| t.id.clone()))
      .collect();
    assert_eq!(ids[0].as_deref(), Some("c1"));
    assert_eq!(ids[1].as_deref(), Some("c0"));
    assert_eq!(ids[2].as_deref(), Some("c0"));
    assert_eq!(ids[3].as_deref(), Some("c1"));
    Ok(())
  }

  #[test]
  fn sse_malformed_json_yields_error() -> anyhow::Result<()> {
    let body = "data: not-json\n\n";
    match collect_deltas(body) {
      Ok(_) => anyhow::bail!("expected a malformed SSE delta error"),
      Err(e) => assert_eq!(e.to_string(), "malformed SSE delta"),
    }
    Ok(())
  }

  #[test]
  fn sse_final_line_without_trailing_newline_is_decoded() -> anyhow::Result<()>
  {
    let body = r#"data: {"choices":[{"delta":{"content":"no-newline"},"finish_reason":null}]}"#;
    let deltas = collect_deltas(body)?;
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].content.as_deref(), Some("no-newline"));
    Ok(())
  }

  #[test]
  fn config_defaults_are_none() -> anyhow::Result<()> {
    let cfg = Config::deserialize(&serde_json::json!({}))?;
    assert_eq!(cfg.base_url, None);
    assert_eq!(cfg.api_key, None);
    assert_eq!(cfg.model, None);

    let cfg = Config::deserialize(&serde_json::json!({
        "base_url": "https://example.com/v1",
        "api_key": "sk-test",
        "model": "gpt-test",
    }))?;
    assert_eq!(cfg.base_url.as_deref(), Some("https://example.com/v1"));
    assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));
    assert_eq!(cfg.model.as_deref(), Some("gpt-test"));
    Ok(())
  }
}
