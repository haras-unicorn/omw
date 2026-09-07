//! The OpenAI-compatible HTTP endpoint. When `[endpoint]` is configured,
//! a small axum server is started exposing `/v1/models` and `POST
//! /v1/chat/completions`. Each subscribed agent is addressable as a model
//! under the name it `host.endpoint-subscribe`d; a request opens a session and
//! delivers an `endpoint-message` event into the owning agent's inbox. The
//! agent streams its reply back with `host.endpoint-stream`, which queues into a
//! per-session buffer the server drains: as SSE chunks for `stream: true`, or
//! buffered into a single JSON completion for `stream: false`.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::unfold;

use serde::Deserialize;

use crate::host::bus::MessageBus;
use crate::host::endpoint::{EndpointRegistry, OpenSession, Outbound};
use crate::provider::{ChatDelta, ChatMessage, Role, ToolCall};
use crate::tooling::Tool;

/// Shared, process-level server state handed to every handler.
#[derive(Clone)]
pub struct ServerState {
  bus: Arc<MessageBus>,
  registry: Arc<EndpointRegistry>,
}

impl ServerState {
  pub fn new(bus: Arc<MessageBus>, registry: Arc<EndpointRegistry>) -> Self {
    Self { bus, registry }
  }
}

/// Build the axum router for the endpoint.
pub fn router(state: ServerState) -> Router {
  Router::new()
    .route("/v1/models", get(list_models))
    .route("/v1/chat/completions", post(chat_completions))
    .with_state(state)
}

async fn list_models(State(state): State<ServerState>) -> impl IntoResponse {
  let models = state.bus.endpoint_models();
  Json(serde_json::json!({
    "object": "list",
    "data": models.into_iter().map(|id| serde_json::json!({
      "id": id,
      "object": "model",
      "created": 0,
      "owned_by": "omw",
    })).collect::<Vec<_>>(),
  }))
}

async fn chat_completions(
  State(state): State<ServerState>,
  body: Result<Json<ChatRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
  let Json(body) = match body {
    Ok(Json(body)) => Json(body),
    Err(error) => {
      return error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
  };
  let messages = match parse_messages(&body.messages) {
    Ok(messages) => messages,
    Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
  };
  let tools = match parse_tools(body.tools.as_deref()) {
    Ok(tools) => tools,
    Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
  };
  let (agent, subscription) = match state.bus.endpoint_lookup(&body.model) {
    Some(pair) => pair,
    None => {
      return error_response(
        StatusCode::NOT_FOUND,
        &format!("no such endpoint model {:?}", body.model),
      );
    }
  };

  // Open the session before routing, so the event the agent receives carries
  // the real session id the server will drain.
  let mut open = state.registry.clone().open(&agent, &subscription);
  if let Err(error) =
    state
      .bus
      .endpoint_route(&body.model, &open.session, messages, tools)
  {
    // The model unsubscribed mid-request: drop the session silently. The
    // agent never saw an `endpoint-message`, so it must not get an
    // `endpoint-session-end` for a session it doesn't know.
    state.registry.remove_silent(&open.session);
    return error_response(StatusCode::NOT_FOUND, &error);
  }
  let model = body.model.clone();
  if body.stream {
    Sse::new(sse_body(open, Arc::clone(&state.registry), model)).into_response()
  } else {
    let mut content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut finish_reason = None;
    let mut normal = false;
    while let Some(outbound) = open.rx.recv().await {
      match outbound {
        Outbound::Delta(d) => {
          if let Some(chunk) = d.content {
            content.push_str(&chunk);
          }
          if let Some(tc) = d.tool_call {
            merge_tool_call(&mut tool_calls, tc);
          }
          if d.finish_reason.is_some() {
            finish_reason = d.finish_reason;
          }
        }
        Outbound::Close => {
          normal = true;
          break;
        }
      }
    }
    if normal {
      state.registry.deliver_ended(
        &open.agent,
        &open.subscription,
        &open.session,
        None,
      );
    }
    Json(chat_completion(
      &body.model,
      &content,
      &tool_calls,
      finish_reason,
    ))
    .into_response()
  }
}

/// The SSE body for a streaming request: relays every delta as an SSE `data:`
/// chunk, and ends with `data: [DONE]` when the agent's reply completes.
fn sse_body(
  open: OpenSession,
  registry: Arc<EndpointRegistry>,
  model: String,
) -> impl futures_util::Stream<Item = Result<SseEvent, Infallible>> {
  let state = SseState {
    rx: open.rx,
    registry,
    agent: open.agent,
    subscription: open.subscription,
    session: open.session,
    model,
    completion_id: format!("chatcmpl-{}", crate::host::bus::new_uuid()),
    first: true,
    done: false,
  };
  unfold(state, |mut state: SseState| async move {
    if state.done {
      return None;
    }
    let outbound = state.rx.recv().await?;
    match outbound {
      Outbound::Delta(d) => {
        let event =
          delta_sse(&d, &state.completion_id, &state.model, state.first);
        state.first = false;
        Some((Ok(event), state))
      }
      Outbound::Close => {
        state.done = true;
        state.registry.deliver_ended(
          &state.agent,
          &state.subscription,
          &state.session,
          None,
        );
        Some((Ok(SseEvent::default().data("[DONE]")), state))
      }
    }
  })
}

struct SseState {
  rx: crate::host::endpoint::SessionRx,

  registry: Arc<EndpointRegistry>,
  agent: String,
  subscription: String,
  session: String,
  model: String,
  completion_id: String,
  first: bool,
  done: bool,
}

fn delta_sse(
  d: &ChatDelta,
  completion_id: &str,
  model: &str,
  first: bool,
) -> SseEvent {
  let mut delta = serde_json::Map::new();
  if first {
    delta.insert("role".into(), "assistant".into());
  }
  if let Some(content) = &d.content {
    delta.insert("content".into(), content.clone().into());
  }
  if let Some(tc) = &d.tool_call {
    delta.insert(
      "tool_calls".into(),
      serde_json::json!([{
        "index": 0,
        "id": tc.id,
        "type": "function",
        "function": { "name": tc.name, "arguments": tc.arguments },
      }]),
    );
  }
  let data = serde_json::json!({
    "id": completion_id,
    "object": "chat.completion.chunk",
    "created": 0,
    "model": model,
    "choices": [{ "index": 0, "delta": delta, "finish_reason": d.finish_reason }],
  });
  let payload =
    serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());
  SseEvent::default().data(payload)
}

fn chat_completion(
  model: &str,
  content: &str,
  tool_calls: &[ToolCall],
  finish_reason: Option<String>,
) -> serde_json::Value {
  let mut message = serde_json::Map::new();
  message.insert("role".into(), "assistant".into());
  if !content.is_empty() {
    message.insert("content".into(), content.into());
  }
  if !tool_calls.is_empty() {
    message.insert(
      "tool_calls".into(),
      serde_json::json!(
        tool_calls
          .iter()
          .map(|tc| {
            serde_json::json!({
              "id": tc.id,
              "type": "function",
              "function": { "name": tc.name, "arguments": tc.arguments },
            })
          })
          .collect::<Vec<_>>()
      ),
    );
  }
  serde_json::json!({
    "id": format!("chatcmpl-{}", crate::host::bus::new_uuid()),
    "object": "chat.completion",
    "created": 0,
    "model": model,
    "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason }],
  })
}

fn error_response(status: StatusCode, message: &str) -> Response {
  (
    status,
    Json(serde_json::json!({
      "error": { "message": message, "type": "omw_error", "code": null },
    })),
  )
    .into_response()
}

fn parse_messages(
  messages: &[WireMessage],
) -> Result<Vec<ChatMessage>, String> {
  messages
    .iter()
    .map(|m| {
      let role = match m.role.as_str() {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,

        "user" => Role::User,
        other => return Err(format!("unknown role {other:?}")),
      };
      let content = match &m.content {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(parts)) => {
          let mut text = String::new();
          for part in parts {
            match part {
              serde_json::Value::String(s) => text.push_str(s),
              serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(s)) = map.get("text") {
                  text.push_str(s);
                }
              }
              _ => {}
            }
          }
          Some(text)
        }
        Some(other) => {
          return Err(format!("malformed message content: {other}"));
        }
      };
      let tool_call =
        match m.tool_calls.as_ref().and_then(|calls| calls.first()) {
          None => None,
          Some(tc) => {
            let id = tc
              .id
              .clone()
              .ok_or_else(|| "malformed tool call: missing id".to_string())?;
            let name = tc.function.name.clone().ok_or_else(|| {
              "malformed tool call: missing function name".to_string()
            })?;
            Some(ToolCall {
              id,
              name,
              arguments: match &tc.function.arguments {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
              },
            })
          }
        };
      Ok(ChatMessage {
        role,
        content,
        tool_call,
      })
    })
    .collect()
}

fn parse_tools(tools: Option<&[WireTool]>) -> Result<Vec<Tool>, String> {
  let Some(tools) = tools else {
    return Ok(Vec::new());
  };
  tools
    .iter()
    .map(|t| match t {
      WireTool::Function { kind, function } => {
        if kind != "function" {
          return Err(format!("malformed tool type {kind:?}"));
        }
        Ok(Tool {
          name: function.name.clone(),
          description: function.description.clone(),
          input_schema: function
            .parameters
            .clone()
            .unwrap_or(serde_json::Value::Null),
        })
      }
      WireTool::Flat {
        name,
        description,
        parameters,
      } => Ok(Tool {
        name: name.clone(),
        description: description.clone(),
        input_schema: parameters.clone().unwrap_or(serde_json::Value::Null),
      }),
    })
    .collect()
}

fn merge_tool_call(tool_calls: &mut Vec<ToolCall>, incoming: ToolCall) {
  if let Some(existing) = tool_calls.iter_mut().find(|c| c.id == incoming.id) {
    let args = &incoming.arguments;
    if args.starts_with(&existing.arguments) {
      existing.arguments = args.clone();
    } else if !args.is_empty() {
      existing.arguments.push_str(args);
    }
  } else {
    tool_calls.push(incoming);
  }
}

#[derive(Deserialize)]
struct ChatRequest {
  model: String,
  messages: Vec<WireMessage>,

  #[serde(default)]
  tools: Option<Vec<WireTool>>,
  #[serde(default)]
  stream: bool,
}

#[derive(Deserialize)]
struct WireMessage {
  role: String,
  #[serde(default)]
  content: Option<serde_json::Value>,
  #[serde(default)]
  tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Deserialize)]
struct WireToolCall {
  #[serde(default)]
  id: Option<String>,
  #[allow(dead_code, reason = "OpenAI sends per-call index; we ignore it")]
  #[serde(default)]
  index: Option<u64>,
  function: WireFunction,
}

#[derive(Deserialize)]
struct WireFunction {
  #[serde(default)]
  name: Option<String>,
  #[serde(default)]
  arguments: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WireTool {
  Function {
    #[serde(rename = "type", default = "default_tool_type")]
    kind: String,
    function: WireToolFunction,
  },
  Flat {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<serde_json::Value>,
  },
}

fn default_tool_type() -> String {
  "function".to_string()
}

#[derive(Deserialize)]
struct WireToolFunction {
  name: String,
  #[serde(default)]
  description: Option<String>,
  #[serde(default)]
  parameters: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::host::events::Event;
  use crate::provider::ChatDelta;
  use std::time::Duration;
  use tokio::net::TcpListener;

  /// Run the whole endpoint round-trip for one request: spawn the server,
  /// subscribe an agent, POST a chat completion, drive deltas plus a
  /// finish-reason through the registry, and return the response body plus the
  /// subscription UUID. Along the way it asserts the inbox endpoint-message
  /// event, and the single normal endpoint-session-end delivery.
  async fn drive_reply(stream: bool) -> anyhow::Result<(String, String)> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let server_bus = Arc::clone(&bus);
    let server_registry = Arc::clone(&registry);
    tokio::spawn(async move {
      if let Err(error) = axum::serve(
        listener,
        router(ServerState::new(server_bus, server_registry)),
      )
      .await
      {
        tracing::error!(error = %error, "endpoint test server failed");
      }
    });
    let client = reqwest::Client::new();
    let reader = tokio::spawn(async move {
      let body = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&serde_json::json!({
          "model": "gpt-4o",
          "messages": [{ "role": "user", "content": "hi" }],
          "stream": stream,
        }))
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("endpoint POST failed: {error}"))?
        .text()
        .await
        .map_err(|error| {
          anyhow::anyhow!("endpoint response read failed: {error}")
        })?;
      Ok::<_, anyhow::Error>(body)
    });
    let mut session = None;
    for _ in 0..100 {
      if let Some(envelope) = bus.try_recv("alice")? {
        match envelope.event {
          Event::EndpointMessage(message) => {
            session = Some(message.session);
            break;
          }
          other => {
            return Err(anyhow::anyhow!("unexpected first event: {other:?}"));
          }
        }
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let Some(session) = session else {
      return Err(anyhow::anyhow!("no endpoint-message arrived"));
    };
    registry
      .push(
        "alice",
        &session,
        ChatDelta {
          content: Some("Hello".to_string()),
          tool_call: None,
          finish_reason: None,
        },
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    registry
      .push(
        "alice",
        &session,
        ChatDelta {
          content: None,
          tool_call: None,
          finish_reason: Some("stop".to_string()),
        },
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    let body = reader.await.map_err(|error| {
      anyhow::anyhow!("endpoint reader task failed: {error}")
    })??;
    let end = bus
      .try_recv("alice")?
      .ok_or_else(|| anyhow::anyhow!("expected an endpoint-session-end"))?;
    match end.event {
      Event::EndpointSessionEnd(end) => {
        assert_eq!(end.session, session);
        assert!(end.error.is_none());
      }
      other => {
        return Err(anyhow::anyhow!("unexpected terminal event: {other:?}"));
      }
    }
    Ok((body, sub))
  }

  #[tokio::test]
  async fn chat_completions_streams_sse_until_done() -> anyhow::Result<()> {
    let (body, _) = drive_reply(true).await?;
    assert!(body.contains("Hello"));
    assert!(body.contains("\"finish_reason\":\"stop\""));
    assert!(body.contains("\"model\":\"gpt-4o\""));
    assert!(body.contains("\"role\":\"assistant\""));
    assert!(body.contains("data: [DONE]"));
    Ok(())
  }

  #[tokio::test]
  async fn chat_completions_buffers_json_completion() -> anyhow::Result<()> {
    let (body, _) = drive_reply(false).await?;
    let json: serde_json::Value = serde_json::from_str(&body)
      .map_err(|error| anyhow::anyhow!("expected a JSON body: {error}"))?;
    assert_eq!(json["model"], "gpt-4o");
    assert_eq!(json["choices"][0]["message"]["content"], "Hello");
    assert_eq!(json["choices"][0]["finish_reason"], "stop");
    Ok(())
  }

  async fn spawn_server(
    bus: Arc<MessageBus>,
    registry: Arc<EndpointRegistry>,
  ) -> anyhow::Result<(reqwest::Client, String)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
      if let Err(error) =
        axum::serve(listener, router(ServerState::new(bus, registry))).await
      {
        tracing::error!(error = %error, "endpoint test server failed");
      }
    });
    Ok((reqwest::Client::new(), format!("http://{addr}")))
  }

  async fn post_completion(
    client: &reqwest::Client,
    base: &str,
    payload: serde_json::Value,
  ) -> anyhow::Result<(reqwest::StatusCode, String)> {
    let response = client
      .post(format!("{base}/v1/chat/completions"))
      .json(&payload)
      .send()
      .await
      .map_err(|error| anyhow::anyhow!("endpoint POST failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
      anyhow::anyhow!("endpoint response read failed: {error}")
    })?;
    Ok((status, body))
  }

  #[tokio::test]
  async fn chat_completions_rejects_unknown_model_as_openai_error()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let (client, base) = spawn_server(bus, registry).await?;
    let (status, body) = post_completion(
      &client,
      &base,
      serde_json::json!({
        "model": "nope",
        "messages": [{ "role": "user", "content": "hi" }],
      }),
    )
    .await?;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_str(&body)
      .map_err(|error| anyhow::anyhow!("expected a JSON body: {error}"))?;
    assert!(
      json["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("nope")
    );
    Ok(())
  }

  #[tokio::test]
  async fn chat_completions_rejects_unknown_role_and_malformed_body()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let (client, base) = spawn_server(bus, registry).await?;
    let (status, _) = post_completion(
      &client,
      &base,
      serde_json::json!({
        "model": "gpt-4o",
        "messages": [{ "role": "wizard", "content": "hi" }],
      }),
    )
    .await?;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    let (status, _) =
      post_completion(&client, &base, serde_json::json!({ "model": "gpt-4o" }))
        .await?;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    let (status, body) = post_completion(
      &client,
      &base,
      serde_json::json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [{ "type": "nope", "function": { "name": "x" } }],
      }),
    )
    .await?;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    assert!(body.contains("malformed tool"));
    Ok(())
  }

  #[tokio::test]
  async fn chat_completions_accepts_openai_tool_and_complex_content_shapes()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let (client, base) =
      spawn_server(Arc::clone(&bus), Arc::clone(&registry)).await?;
    let reader = tokio::spawn({
      let client = client.clone();
      let base = base.clone();
      async move {
        post_completion(
          &client,
          &base,
          serde_json::json!({
            "model": "gpt-4o",
            "messages": [{
              "role": "user",
              "content": [
                { "type": "text", "text": "he" },
                { "type": "text", "text": "llo" },
              ],
            }],
            "tools": [{
              "type": "function",
              "function": {
                "name": "get_weather",
                "description": "weather",
                "parameters": { "type": "object" },
              },
            }],
            "stream": false,
          }),
        )
        .await
      }
    });
    let envelope = loop {
      if let Some(envelope) = bus.try_recv("alice")? {
        break envelope;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let Event::EndpointMessage(message) = envelope.event else {
      return Err(anyhow::anyhow!("expected an endpoint-message"));
    };
    assert_eq!(envelope.id, sub);
    assert_eq!(
      message.messages[0].content.as_deref(),
      Some("hello"),
      "array content parts should concatenate"
    );
    assert_eq!(message.tools.len(), 1);
    assert_eq!(message.tools[0].name, "get_weather");
    registry
      .push(
        "alice",
        &message.session,
        ChatDelta {
          content: None,
          tool_call: None,
          finish_reason: Some("stop".to_string()),
        },
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    let (status, _) = reader.await.map_err(|error| {
      anyhow::anyhow!("endpoint reader task failed: {error}")
    })??;
    assert!(status.is_success());
    Ok(())
  }

  #[tokio::test]
  async fn models_lists_subscribed_models_in_sorted_order() -> anyhow::Result<()>
  {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    bus
      .endpoint_subscribe("alice", "zeta".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    bus
      .endpoint_subscribe("bob", "alpha".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let (client, base) = spawn_server(bus, registry).await?;
    let body = client
      .get(format!("{base}/v1/models"))
      .send()
      .await
      .map_err(|error| anyhow::anyhow!("models GET failed: {error}"))?
      .text()
      .await
      .map_err(|error| {
        anyhow::anyhow!("models response read failed: {error}")
      })?;
    let json: serde_json::Value = serde_json::from_str(&body)
      .map_err(|error| anyhow::anyhow!("expected a JSON body: {error}"))?;
    let ids: Vec<&str> = json["data"]
      .as_array()
      .ok_or_else(|| anyhow::anyhow!("expected a data array"))?
      .iter()
      .filter_map(|m| m["id"].as_str())
      .collect();
    assert_eq!(ids, vec!["alpha", "zeta"]);
    Ok(())
  }

  #[tokio::test]
  async fn route_miss_after_unsubscribe_drops_silently_without_session_end()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    // Simulate lookup -> open -> unsubscribe -> route: open the session
    // first, then drop the subscription, then route and clean up silently.
    let open = registry.clone().open("alice", &sub);
    bus.endpoint_unsubscribe("alice", &sub);
    let route =
      bus.endpoint_route("gpt-4o", &open.session, Vec::new(), Vec::new());
    assert!(route.is_err());
    registry.remove_silent(&open.session);
    assert!(bus.try_recv("alice")?.is_none());
    Ok(())
  }

  #[tokio::test]
  async fn client_disconnect_aborts_session_with_single_error_end()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let open = registry.clone().open("alice", &sub);
    let session = open.session.clone();
    drop(open.rx);
    let envelope = bus
      .try_recv("alice")?
      .ok_or_else(|| anyhow::anyhow!("expected an endpoint-session-end"))?;
    assert_eq!(envelope.id, sub);
    let Event::EndpointSessionEnd(end) = envelope.event else {
      return Err(anyhow::anyhow!("expected an endpoint-session-end"));
    };
    assert_eq!(end.session, session);
    assert!(end.error.is_some());
    assert!(bus.try_recv("alice")?.is_none());
    Ok(())
  }

  #[tokio::test]
  async fn unsubscribe_mid_flight_ends_sessions_without_handler_double_fire()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let open = registry.clone().open("alice", &sub);
    let session = open.session.clone();
    bus.endpoint_unsubscribe("alice", &sub);
    registry.cancel_subscription(&sub);
    // The rx half is now orphaned: dropping it must not fire a second end.
    drop(open.rx);
    let envelope = bus
      .try_recv("alice")?
      .ok_or_else(|| anyhow::anyhow!("expected an endpoint-session-end"))?;
    let Event::EndpointSessionEnd(end) = envelope.event else {
      return Err(anyhow::anyhow!("expected an endpoint-session-end"));
    };
    assert_eq!(end.session, session);
    assert!(end.error.is_some());
    assert!(bus.try_recv("alice")?.is_none());
    Ok(())
  }

  #[tokio::test]
  async fn double_terminal_push_errors_as_unknown_session() -> anyhow::Result<()>
  {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let mut open = registry.clone().open("alice", &sub);
    registry
      .push(
        "alice",
        &open.session,
        ChatDelta {
          content: None,
          tool_call: None,
          finish_reason: Some("stop".to_string()),
        },
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    // Drain terminal delta + close so the drop below doesn't abort.
    let _ = open.rx.recv().await;
    let _ = open.rx.recv().await;
    let err = registry
      .push(
        "alice",
        &open.session,
        ChatDelta {
          content: None,
          tool_call: None,
          finish_reason: Some("stop".to_string()),
        },
      )
      .unwrap_err();
    assert_eq!(err, "unknown endpoint session");
    Ok(())
  }
}
