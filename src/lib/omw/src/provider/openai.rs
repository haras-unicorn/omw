//! OpenAI-compatible provider over `reqwest`, always streaming SSE deltas.
//!
//! Cancellation is by dropping the returned stream: dropping it drops the
//! in-flight `reqwest` response and closes the connection.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::Context as _;
use futures_util::stream::{BoxStream, Stream, StreamExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ChatDelta, ChatMessage, Provider, ToolCallDelta};
use crate::secret::Secret;
use crate::tooling::Tool;

/// Impl-specific configuration for the OpenAI-family provider.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct Config {
  #[serde(default)]
  pub base_url: Option<String>,
  #[serde(default)]
  pub api_key: Option<Secret>,
  #[serde(default)]
  pub model: Option<String>,
  /// Default generation params merged into every request body (temperature,
  /// max_tokens, reasoning_effort, …). Per-call params override these.
  #[serde(default)]
  pub params: Option<serde_json::Map<String, Value>>,
}

/// An OpenAI-compatible chat provider backed by `reqwest`.
pub struct OpenAIProvider {
  config: Config,
  client: reqwest::Client,
}

impl super::Factory for OpenAIProvider {
  fn build(name: &str, params: &Value) -> anyhow::Result<Arc<Self>> {
    let config = Config::deserialize(params).with_context(|| {
      format!("invalid openai provider config for {name:?}")
    })?;
    tracing::debug!(name, config = ?config, "built openai provider");
    Ok(Arc::new(OpenAIProvider::new(config)?))
  }

  fn opaque_fields() -> &'static [&'static str] {
    &["params"]
  }
}

impl OpenAIProvider {
  fn new(config: Config) -> anyhow::Result<Self> {
    let client = reqwest::Client::builder()
      .build()
      .context("failed to build http client")?;
    Ok(Self { config, client })
  }

  /// Fetch the model list from `GET /models`, erroring if the endpoint cannot
  /// be reached or does not answer with a success status.
  async fn fetch_models(&self) -> anyhow::Result<Vec<String>> {
    let url = format!(
      "{}/models",
      self
        .config
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com/v1")
        .trim_end_matches('/')
    );
    let mut request = self.client.get(&url).timeout(Duration::from_secs(5));
    if let Some(api_key) = &self.config.api_key {
      request = request.bearer_auth(api_key.expose());
    }
    let resp = request
      .send()
      .await
      .context("failed to send models request")?;
    if !resp.status().is_success() {
      let status = resp.status();
      let text = resp.text().await.unwrap_or_default();
      anyhow::bail!("models request failed with status {status}: {text}");
    }
    let body: WireModels = resp
      .json()
      .await
      .context("failed to decode models response")?;
    Ok(body.data.into_iter().map(|model| model.id).collect())
  }
}

#[async_trait::async_trait]
impl Provider for OpenAIProvider {
  fn kind() -> &'static str {
    "openai"
  }

  async fn list_models(&self) -> anyhow::Result<Vec<String>> {
    let mut models = self.fetch_models().await?;
    // A reachable endpoint that reports no models falls back to the configured
    // `model`; a failed request is an error the caller sees.
    if models.is_empty()
      && let Some(model) = &self.config.model
    {
      models.push(model.clone());
    }
    Ok(models)
  }

  async fn chat_stream(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
    params: Option<Value>,
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
    // Start from the configured defaults, overlay the per-call params, then
    // pin the mandatory fields so neither layer can break the request shape.
    let mut body = serde_json::Map::new();
    if let Some(defaults) = &self.config.params {
      for (key, value) in defaults {
        body.insert(key.clone(), value.clone());
      }
    }
    if let Some(Value::Object(params)) = &params {
      for (key, value) in params {
        body.insert(key.clone(), value.clone());
      }
    }
    body.insert("model".into(), Value::String(model.to_string()));
    body.insert("stream".into(), Value::Bool(true));
    body.insert(
      "messages".into(),
      serde_json::json!(
        messages.iter().map(to_wire_message).collect::<Vec<_>>()
      ),
    );
    if tools.is_empty() {
      body.remove("tools");
    } else {
      body.insert(
        "tools".into(),
        serde_json::json!(tools.iter().map(to_wire_tool).collect::<Vec<_>>()),
      );
    }
    let body = Value::Object(body);

    let mut request = self.client.post(&url);
    if let Some(api_key) = &self.config.api_key {
      request = request.bearer_auth(api_key.expose());
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

/// The `GET /models` response, with only the fields we consume.
#[derive(Debug, Deserialize)]
struct WireModels {
  #[serde(default)]
  data: Vec<WireModel>,
}

#[derive(Debug, Deserialize)]
struct WireModel {
  id: String,
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
  let mut m = serde_json::json!({ "role": msg.role });
  if let Some(content) = &msg.content {
    m["content"] = serde_json::Value::String(content.clone());
  }
  if let Some(reasoning) = &msg.reasoning {
    m["reasoning"] = serde_json::Value::String(reasoning.clone());
  }
  if !msg.tool_calls.is_empty() {
    let calls = msg
      .tool_calls
      .iter()
      .map(|tool_call| {
        serde_json::json!({
            "id": tool_call.id,
            "type": "function",
            "function": {
                "name": tool_call.name,
                "arguments": tool_call.arguments,
            },
        })
      })
      .collect::<Vec<_>>();
    m["tool_calls"] = serde_json::Value::Array(calls);
  }
  if let Some(tool_call_id) = &msg.tool_call_id {
    m["tool_call_id"] = serde_json::Value::String(tool_call_id.clone());
  }
  m
}

/// A single SSE data payload from the wire, with only the fields we consume.
#[derive(Debug, Deserialize)]
struct WireChunk {
  #[serde(default)]
  choices: Vec<WireChoice>,
  /// The wire's token accounting, kept as opaque JSON so every provider's
  /// shape (including a growing `*_tokens_details` breakdown) passes through.
  #[serde(default)]
  usage: Option<serde_json::Value>,
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
  /// Some providers spell it `reasoning`, others `reasoning_content`.
  #[serde(default, alias = "reasoning_content")]
  reasoning: Option<String>,
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

/// Decodes SSE `data:` lines into [`ChatDelta`]s, surfacing each chunk's
/// tool-call fragments verbatim (tagged with their index) so consumers
/// reassemble parallel calls themselves.
struct SseDeltas<S> {
  inner: S,
  buffer: String,
}

impl<S: Stream<Item = Result<String, String>> + Unpin> SseDeltas<S> {
  fn new(inner: S) -> Self {
    Self {
      inner,
      buffer: String::new(),
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
    let usage = chunk.usage.as_ref().map(serde_json::Value::to_string);
    let Some(choice) = chunk.choices.first() else {
      // A usage-only trailing chunk carries no choices.
      return usage.map(|usage| ChatDelta {
        content: None,
        reasoning: None,
        tool_calls: Vec::new(),
        finish_reason: None,
        usage: Some(usage),
      });
    };
    let content = choice.delta.content.clone();
    let reasoning = choice.delta.reasoning.clone();
    let tool_calls = choice
      .delta
      .tool_calls
      .iter()
      .map(|tc| ToolCallDelta {
        index: u32::try_from(tc.index.unwrap_or(0)).unwrap_or(u32::MAX),
        id: tc.id.clone(),
        name: tc.function.as_ref().and_then(|f| f.name.clone()),
        arguments: tc.function.as_ref().and_then(|f| f.arguments.clone()),
      })
      .collect::<Vec<_>>();

    tracing::trace!(
      content_len = content.as_ref().map_or(0, String::len),
      reasoning_len = reasoning.as_ref().map_or(0, String::len),
      tool_calls = tool_calls.len(),
      finish_reason = choice.finish_reason.as_deref(),
      "openai chat delta"
    );
    Some(ChatDelta {
      content,
      reasoning,
      tool_calls,
      finish_reason: choice.finish_reason.clone(),
      usage,
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
            return Poll::Ready(None);
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
          return Poll::Ready(None);
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
      return None;
    }
    let line = std::mem::take(&mut self.buffer);
    if let Some(data) = line.strip_prefix("data:").map(str::trim) {
      if data == "[DONE]" {
        return None;
      }
      if let Ok(chunk) = serde_json::from_str::<WireChunk>(data) {
        return self.decode_chunk(&chunk);
      }
    }
    None
  }
}

#[cfg(test)]
mod tests {
  use futures_util::StreamExt;

  use super::*;
  use crate::provider::ToolCall;
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
    for role in ["system", "developer", "user", "assistant", "tool"] {
      let msg = ChatMessage {
        role: role.to_string(),
        content: Some("hi".to_string()),
        reasoning: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
      };
      let wire = to_wire_message(&msg);
      assert_eq!(wire["role"], role);
      assert_eq!(wire["content"], "hi");
      assert!(wire.get("tool_calls").is_none());
    }
  }

  #[test]
  fn to_wire_message_without_content_omits_the_key() {
    let msg = ChatMessage {
      role: "user".to_string(),
      content: None,
      reasoning: None,
      tool_calls: Vec::new(),
      tool_call_id: None,
    };
    let wire = to_wire_message(&msg);
    assert!(wire.get("content").is_none());
  }

  #[test]
  fn to_wire_message_serializes_parallel_tool_calls() -> anyhow::Result<()> {
    let msg = ChatMessage {
      role: "assistant".to_string(),
      content: None,
      reasoning: None,
      tool_calls: vec![
        ToolCall {
          id: "call_1".to_string(),
          name: "get_weather".to_string(),
          arguments: r#"{"city":"Paris"}"#.to_string(),
        },
        ToolCall {
          id: "call_2".to_string(),
          name: "get_time".to_string(),
          arguments: "{}".to_string(),
        },
      ],
      tool_call_id: None,
    };
    let wire = to_wire_message(&msg);
    let calls = wire["tool_calls"]
      .as_array()
      .ok_or_else(|| anyhow::anyhow!("missing tool_calls"))?;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["type"], "function");
    assert_eq!(calls[0]["id"], "call_1");
    assert_eq!(calls[0]["function"]["name"], "get_weather");
    assert_eq!(calls[0]["function"]["arguments"], r#"{"city":"Paris"}"#);
    assert_eq!(calls[1]["id"], "call_2");
    Ok(())
  }

  #[test]
  fn to_wire_message_emits_tool_call_id_for_tool_role() -> anyhow::Result<()> {
    let msg = ChatMessage {
      role: "tool".to_string(),
      content: Some("18 degrees".to_string()),
      reasoning: None,
      tool_calls: Vec::new(),
      tool_call_id: Some("call_1".to_string()),
    };
    let wire = to_wire_message(&msg);
    assert_eq!(wire["role"], "tool");
    assert_eq!(wire["tool_call_id"], "call_1");
    Ok(())
  }

  #[test]
  fn to_wire_tool_serializes_schema_and_optional_description() {
    let tool = Tool {
      name: "weather".to_string(),
      title: None,
      description: Some("gets the weather".to_string()),
      input_schema: serde_json::json!({ "type": "object" }),
      output_schema: None,
      annotations: None,
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
      title: None,
      description: None,
      input_schema: serde_json::json!({}),
      output_schema: None,
      annotations: None,
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
    assert!(deltas[0].tool_calls.is_empty());
    assert_eq!(deltas[0].finish_reason, None);
    Ok(())
  }

  #[test]
  fn sse_surfaces_tool_call_fragments_verbatim() -> anyhow::Result<()> {
    let body = format!(
      "{}data: [DONE]\n\n",
      data(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"n","arguments":"{}"}}]},"finish_reason":null}]}"#
      )
    );
    let deltas = collect_deltas(&body)?;
    // The fragment is surfaced as-is; the `[DONE]` marker adds nothing.
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].tool_calls.len(), 1);
    let tc = &deltas[0].tool_calls[0];
    assert_eq!(tc.index, 0);
    assert_eq!(tc.id.as_deref(), Some("c"));
    assert_eq!(tc.name.as_deref(), Some("n"));
    assert_eq!(tc.arguments.as_deref(), Some("{}"));
    Ok(())
  }

  #[test]
  fn sse_fragmented_arguments_stay_fragments() -> anyhow::Result<()> {
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
    // No reassembly at the codec level: each chunk surfaces its own fragment.
    assert_eq!(deltas.len(), 3);
    let args: Vec<Option<&str>> = deltas
      .iter()
      .map(|d| d.tool_calls.first().and_then(|t| t.arguments.as_deref()))
      .collect();
    assert_eq!(args[0], Some(""));
    assert_eq!(args[1], Some(r#"{"city":"#));
    assert_eq!(args[2], Some(r#""Paris"}"#));
    Ok(())
  }

  #[test]
  fn sse_surfaces_each_parallel_call_with_its_index() -> anyhow::Result<()> {
    let body = format!(
      "{}data: [DONE]\n\n",
      [
        data(r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c1","function":{"name":"n1","arguments":"{}"}}]},"finish_reason":null}]}"#),
        data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c0","function":{"name":"n0","arguments":"{}"}}]},"finish_reason":null}]}"#),
      ]
      .concat()
    );
    let deltas = collect_deltas(&body)?;
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].tool_calls[0].index, 1);
    assert_eq!(deltas[0].tool_calls[0].id.as_deref(), Some("c1"));
    assert_eq!(deltas[1].tool_calls[0].index, 0);
    assert_eq!(deltas[1].tool_calls[0].id.as_deref(), Some("c0"));
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
  fn sse_reasoning_and_usage_are_decoded() -> anyhow::Result<()> {
    let body = format!(
      "{}data: [DONE]\n\n",
      [
        data(r#"{"choices":[{"delta":{"reasoning_content":"think "},"finish_reason":null}]}"#),
        data(r#"{"choices":[{"delta":{"reasoning":"harder","content":"answer"},"finish_reason":"stop"}]}"#),
        data(r#"{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":5,"total_tokens":8}}"#),
      ]
      .concat()
    );
    let deltas = collect_deltas(&body)?;
    assert_eq!(deltas.len(), 3);
    assert_eq!(deltas[0].reasoning.as_deref(), Some("think "));
    assert_eq!(deltas[1].reasoning.as_deref(), Some("harder"));
    assert_eq!(deltas[1].content.as_deref(), Some("answer"));
    let usage = deltas[2]
      .usage
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("missing usage"))?;
    let usage: serde_json::Value = serde_json::from_str(usage)?;
    assert_eq!(usage["prompt_tokens"], 3);
    assert_eq!(usage["completion_tokens"], 5);
    assert_eq!(usage["total_tokens"], 8);
    Ok(())
  }

  #[test]
  fn config_defaults_are_none() -> anyhow::Result<()> {
    let cfg = Config::deserialize(&serde_json::json!({}))?;
    assert_eq!(cfg.base_url, None);
    assert_eq!(cfg.api_key, None);
    assert_eq!(cfg.model, None);
    assert!(cfg.params.is_none());

    let cfg = Config::deserialize(&serde_json::json!({
        "base_url": "https://example.com/v1",
        "api_key": "sk-test",
        "model": "gpt-test",
        "params": { "temperature": 0.2, "reasoning_effort": "high" },
    }))?;
    assert_eq!(cfg.base_url.as_deref(), Some("https://example.com/v1"));
    assert_eq!(cfg.api_key.as_ref().map(|k| k.expose()), Some("sk-test"));
    assert_eq!(cfg.model.as_deref(), Some("gpt-test"));
    let params = cfg
      .params
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("missing params"))?;
    assert_eq!(params["temperature"], serde_json::json!(0.2));
    assert_eq!(params["reasoning_effort"], serde_json::json!("high"));
    Ok(())
  }
}
