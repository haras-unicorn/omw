//! An in-memory scripted provider for tests and local development.
//!
//! A `chat` pops the next configured turn (repeating the last one once the
//! script is exhausted) and records the call (model, messages, tools) so tests
//! can assert what the guest actually sent. A turn is either plain content or a
//! tool call, which is what a ReAct brain needs.
#![allow(
  dead_code,
  reason = "the mock back end is a test double; its inspection helpers are used by unit tests and by omw-test, not by the library itself"
)]

use std::sync::Arc;

use anyhow::Context as _;
use futures_util::StreamExt as _;
use futures_util::stream::BoxStream;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use super::{ChatDelta, ChatMessage, Factory, Provider, ToolCall, Usage};
use crate::tooling::Tool;

/// Impl-specific configuration for the mock provider.
#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
pub(crate) struct Config {
  /// Scripted turns; one is popped per `chat` (repeating the last once
  /// exhausted).
  #[serde(default)]
  pub turns: Vec<Turn>,

  /// The model names `list-models` returns. Defaults to `["mock-model"]`.
  #[serde(default)]
  pub models: Vec<String>,
}

/// One scripted turn: optional reasoning, content, a tool call, usage, and an
/// optional failure or pending marker.
#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
pub struct Turn {
  /// Plain content emitted before the terminal finish reason.
  #[serde(default)]
  pub content: Option<String>,
  /// Reasoning/thinking content emitted as its own delta.
  #[serde(default)]
  pub reasoning: Option<String>,
  /// A tool call, which makes the terminal finish reason `"tool_calls"`.
  #[serde(default)]
  pub tool_call: Option<ToolCallSpec>,
  /// Token accounting attached to the terminal delta.
  #[serde(default)]
  pub usage: Option<UsageSpec>,
  /// Fail the stream with this error, after emitting the turn's deltas.
  #[serde(default)]
  pub error: Option<String>,
  /// Never yield: the stream stays pending forever (for cancellation tests).
  #[serde(default)]
  pub pending: bool,
}

/// The scripted token counts of one turn.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct UsageSpec {
  #[serde(default)]
  pub prompt_tokens: Option<u64>,
  #[serde(default)]
  pub completion_tokens: Option<u64>,
  #[serde(default)]
  pub total_tokens: Option<u64>,
}

/// The scripted values of one tool call.
#[derive(Debug, Clone)]
pub struct ToolCallSpec {
  pub id: String,
  pub name: String,
  /// The wire-shape JSON string the model would have produced. A config may
  /// supply it either as a string or as an inline value, which is stringified
  /// when the mock is built.
  pub arguments: String,
}

/// Mock tool-call arguments: the wire string, or an inline JSON value that is
/// stringified at build time.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Arguments {
  Text(String),
  Value(Value),
}

impl Arguments {
  fn into_string(self) -> String {
    match self {
      Self::Text(text) => text,
      Self::Value(value) => value.to_string(),
    }
  }
}

impl<'de> Deserialize<'de> for ToolCallSpec {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    #[derive(Deserialize)]
    struct Raw {
      id: String,
      name: String,
      arguments: Arguments,
    }
    let raw = Raw::deserialize(deserializer)?;
    Ok(Self {
      id: raw.id,
      name: raw.name,
      arguments: raw.arguments.into_string(),
    })
  }
}

impl schemars::JsonSchema for ToolCallSpec {
  fn schema_name() -> std::borrow::Cow<'static, str> {
    std::borrow::Cow::Borrowed("MockToolCallSpec")
  }

  fn json_schema(
    _generator: &mut schemars::SchemaGenerator,
  ) -> schemars::Schema {
    schemars::json_schema!({
      "type": "object",
      "properties": {
        "id": { "type": "string" },
        "name": { "type": "string" },
        "arguments": {}
      },
      "required": ["id", "name", "arguments"]
    })
  }
}

/// A single recorded `chat` invocation.
#[derive(Debug, Clone)]
pub struct ChatCall {
  pub model: String,
  pub messages: Vec<ChatMessage>,
  pub tools: Vec<Tool>,
  /// The opaque per-call generation params the guest passed, if any.
  pub params: Option<Value>,
  /// The deltas this call returned.
  pub deltas: Vec<ChatDelta>,
}

/// A scripted provider backed by an in-memory turn queue.
pub struct MockProvider {
  /// Each chat pops the next turn.
  turns: Vec<Turn>,
  models: Vec<String>,
  cursor: Arc<Mutex<usize>>,
  calls: Arc<Mutex<Vec<ChatCall>>>,
}

impl Factory for MockProvider {
  fn build(_name: &str, params: &Value) -> anyhow::Result<Arc<Self>> {
    let config = Config::deserialize(params)
      .with_context(|| "invalid mock provider config".to_string())?;
    Ok(Arc::new(MockProvider {
      turns: config.turns,
      models: config.models,
      cursor: Arc::new(Mutex::new(0)),
      calls: Arc::new(Mutex::new(Vec::new())),
    }))
  }
}

impl MockProvider {
  /// A bare mock with no turns (for tests that only assert the call).
  pub fn noop() -> Arc<Self> {
    Arc::new(Self {
      turns: Vec::new(),
      models: Vec::new(),
      cursor: Arc::new(Mutex::new(0)),
      calls: Arc::new(Mutex::new(Vec::new())),
    })
  }

  /// Recorded chat calls, in order.
  pub async fn calls(&self) -> Vec<ChatCall> {
    self.calls.lock().await.clone()
  }

  /// Pop the next turn, repeating the last one once the script is exhausted.
  async fn next_turn(&self) -> Option<Turn> {
    if self.turns.is_empty() {
      return None;
    }
    let mut cursor = self.cursor.lock().await;
    let last = self.turns.len().saturating_sub(1);
    let index = (*cursor).min(last);
    let next = cursor.saturating_add(1);
    *cursor = next;
    self.turns.get(index).cloned()
  }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
  fn kind() -> &'static str {
    "mock"
  }

  async fn list_models(&self) -> anyhow::Result<Vec<String>> {
    Ok(if self.models.is_empty() {
      vec!["mock-model".to_string()]
    } else {
      self.models.clone()
    })
  }

  async fn chat_stream(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
    params: Option<Value>,
  ) -> anyhow::Result<BoxStream<'static, Result<ChatDelta, String>>> {
    let turn = self.next_turn().await;
    let pending = turn.as_ref().is_some_and(|turn| turn.pending);
    let error = turn.as_ref().and_then(|turn| turn.error.clone());
    // A pending stream yields nothing, so no deltas are recorded as returned.
    let deltas = if pending {
      Vec::new()
    } else {
      turn_to_deltas(turn)
    };
    self.calls.lock().await.push(ChatCall {
      model: model.to_string(),
      messages,
      tools,
      params,
      deltas: deltas.clone(),
    });
    if pending {
      return Ok(Box::pin(futures_util::stream::pending()));
    }
    let items: Vec<Result<ChatDelta, String>> =
      deltas.into_iter().map(Ok).collect();
    match error {
      Some(error) => {
        let stream = futures_util::stream::iter(items)
          .chain(futures_util::stream::once(async move { Err(error) }));
        Ok(Box::pin(stream))
      }
      None => Ok(Box::pin(futures_util::stream::iter(items))),
    }
  }

  fn snapshot(&self) -> Option<Value> {
    let cursor = self.cursor.try_lock().ok().map(|cursor| *cursor)?;
    let calls = self.calls.try_lock().ok()?;
    let consumed = cursor.min(self.turns.len());
    let recorded = calls
      .iter()
      .map(|call| {
        serde_json::json!({
          "model": call.model,
          "messages": call.messages,
          "tools": call.tools,
          "params": call.params,
          "deltas": call.deltas,
        })
      })
      .collect::<Vec<_>>();
    Some(serde_json::json!({
      "kind": "provider",
      "turns_total": self.turns.len(),
      "turns_consumed": consumed,
      "turns_remaining": self.turns.len().saturating_sub(consumed),
      "chat_calls": calls.len(),
      "models": self.models,
      "calls": recorded,
    }))
  }
}

/// Expand one scripted turn into the deltas the mock streams: reasoning first
/// (when present), then a terminal delta carrying the content or tool call plus
/// the turn's usage.
fn turn_to_deltas(turn: Option<Turn>) -> Vec<ChatDelta> {
  let Some(turn) = turn else {
    return Vec::new();
  };
  let usage = turn.usage.as_ref().map(|usage| Usage {
    prompt_tokens: usage.prompt_tokens,
    completion_tokens: usage.completion_tokens,
    total_tokens: usage.total_tokens,
  });
  let mut deltas = Vec::new();
  if let Some(reasoning) = turn.reasoning {
    deltas.push(ChatDelta {
      content: None,
      reasoning: Some(reasoning),
      tool_call: None,
      finish_reason: None,
      usage: None,
    });
  }
  match turn.tool_call {
    Some(tool_call) => deltas.push(ChatDelta {
      content: turn.content,
      reasoning: None,
      tool_call: Some(ToolCall {
        id: tool_call.id,
        name: tool_call.name,
        arguments: tool_call.arguments,
      }),
      finish_reason: Some("tool_calls".to_string()),
      usage,
    }),
    None => deltas.push(ChatDelta {
      content: turn.content,
      reasoning: None,
      tool_call: None,
      finish_reason: Some("stop".to_string()),
      usage,
    }),
  }
  deltas
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn turns_are_sequenced_and_repeat_the_last() -> anyhow::Result<()> {
    let provider = MockProvider::build(
      "m",
      &serde_json::json!({
        "turns": [
          { "content": "one" },
          { "tool_call": { "id": "c1", "name": "t", "arguments": "{}" } },
        ],
      }),
    )?;
    let first = provider.chat("m", Vec::new(), Vec::new(), None).await?;
    assert_eq!(first.content.as_deref(), Some("one"));
    let second = provider.chat("m", Vec::new(), Vec::new(), None).await?;
    assert_eq!(second.tool_calls.len(), 1);
    assert_eq!(second.tool_calls[0].name, "t");
    // The script is exhausted, so the last turn repeats.
    let third = provider.chat("m", Vec::new(), Vec::new(), None).await?;
    assert_eq!(third.tool_calls.len(), 1);
    assert_eq!(third.tool_calls[0].id, "c1");
    Ok(())
  }

  #[tokio::test]
  async fn tool_call_arguments_accept_a_string_or_an_inline_value()
  -> anyhow::Result<()> {
    let provider = MockProvider::build(
      "m",
      &serde_json::json!({
        "turns": [
          {
            "tool_call": {
              "id": "c1",
              "name": "echo",
              "arguments": "{\"input\":\"hi\"}",
            },
          },
          {
            "tool_call": {
              "id": "c2",
              "name": "add",
              "arguments": { "a": 1, "b": [2, 3] },
            },
          },
        ],
      }),
    )?;
    // A string is left verbatim; an inline value is stringified at build.
    let first = provider.chat("m", Vec::new(), Vec::new(), None).await?;
    assert_eq!(first.tool_calls[0].arguments, "{\"input\":\"hi\"}");
    let second = provider.chat("m", Vec::new(), Vec::new(), None).await?;
    assert_eq!(second.tool_calls[0].arguments, "{\"a\":1,\"b\":[2,3]}");
    Ok(())
  }

  #[tokio::test]
  async fn reasoning_usage_and_params_are_scripted_and_recorded()
  -> anyhow::Result<()> {
    let provider = MockProvider::build(
      "m",
      &serde_json::json!({
        "turns": [{
          "reasoning": "thinking",
          "content": "answer",
          "usage": { "prompt_tokens": 2, "completion_tokens": 4 },
        }],
      }),
    )?;
    let params = serde_json::json!({ "temperature": 0.5 });
    let result = provider
      .chat("m", Vec::new(), Vec::new(), Some(params.clone()))
      .await?;
    assert_eq!(result.content.as_deref(), Some("answer"));
    assert_eq!(result.reasoning.as_deref(), Some("thinking"));
    let usage = result
      .usage
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("missing usage"))?;
    assert_eq!(usage.prompt_tokens, Some(2));
    assert_eq!(usage.completion_tokens, Some(4));
    assert_eq!(usage.total_tokens, None);

    let calls = provider.calls().await;
    assert_eq!(calls[0].params.as_ref(), Some(&params));
    Ok(())
  }

  #[tokio::test]
  async fn models_default_and_override() -> anyhow::Result<()> {
    let default = MockProvider::noop();
    assert_eq!(default.list_models().await?, vec!["mock-model"]);
    let configured =
      MockProvider::build("m", &serde_json::json!({ "models": ["a", "b"] }))?;
    assert_eq!(configured.list_models().await?, vec!["a", "b"]);
    Ok(())
  }

  #[tokio::test]
  async fn snapshot_reports_consumed_and_remaining_turns() -> anyhow::Result<()>
  {
    let provider = MockProvider::build(
      "m",
      &serde_json::json!({
        "turns": [{ "content": "a" }, { "content": "b" }],
        "models": ["m"],
      }),
    )?;
    let snapshot = provider
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["turns_total"], 2);
    assert_eq!(snapshot["turns_consumed"], 0);

    let _ = provider.chat("m", Vec::new(), Vec::new(), None).await?;
    let snapshot = provider
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["turns_consumed"], 1);
    assert_eq!(snapshot["turns_remaining"], 1);
    assert_eq!(snapshot["chat_calls"], 1);
    assert_eq!(snapshot["calls"][0]["model"], "m");
    assert_eq!(snapshot["calls"][0]["deltas"][0]["content"], "a");
    assert_eq!(snapshot["calls"][0]["deltas"][0]["finish_reason"], "stop");
    Ok(())
  }

  #[tokio::test]
  async fn a_turn_errors_after_emitting_its_deltas() -> anyhow::Result<()> {
    let provider = MockProvider::build(
      "m",
      &serde_json::json!({
        "turns": [{ "content": "partial", "error": "boom" }],
      }),
    )?;
    let stream = provider
      .chat_stream("m", Vec::new(), Vec::new(), None)
      .await?;
    let items: Vec<Result<ChatDelta, String>> = stream.collect().await;
    assert_eq!(items.len(), 2);
    assert_eq!(
      items[0].as_ref().ok().and_then(|d| d.content.as_deref()),
      Some("partial")
    );
    assert_eq!(items[1].as_ref().err().map(String::as_str), Some("boom"));
    // The emitted deltas are still recorded for the snapshot.
    let calls = provider.calls().await;
    assert_eq!(
      calls[0].deltas[0].content.as_deref(),
      Some("partial"),
      "the emitted deltas should be recorded"
    );
    Ok(())
  }

  #[tokio::test]
  async fn a_pending_turn_never_yields() -> anyhow::Result<()> {
    let provider = MockProvider::build(
      "m",
      &serde_json::json!({ "turns": [{ "pending": true }] }),
    )?;
    let mut stream = provider
      .chat_stream("m", Vec::new(), Vec::new(), None)
      .await?;
    let next =
      tokio::time::timeout(std::time::Duration::from_millis(50), stream.next())
        .await;
    assert!(next.is_err(), "a pending stream should never yield");
    // A pending turn returns nothing, so no deltas are recorded.
    let calls = provider.calls().await;
    assert!(calls[0].deltas.is_empty());
    Ok(())
  }
}
