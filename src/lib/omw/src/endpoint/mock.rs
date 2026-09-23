//! A scripted in-process endpoint double for `omw-test`: it plays the client.
//!
//! Instead of listening on a socket it reads `requests = [{ model, messages,
//! tools, stream }]` from its params, waits until the brain subscribes to each
//! model, routes the request into the owning agent's inbox, and drains the
//! session's outbound deltas back onto the trace channel. No socket and no
//! `listen`, so it stays safe for `dev test fast`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use serde::Deserialize;
use serde_json::Value;

use super::{Endpoint, Factory};
use crate::host::bus::MessageBus;
use crate::host::endpoint::{EndpointRegistry, Outbound, SessionRx};
use crate::provider::{ChatMessage, Role};
use crate::shutdown::Shutdown;
use crate::testing::{After, TraceLog};
use crate::tooling::Tool;

/// One inbound chat message from the scripted client. Kept local so the mock
/// does not require the provider DTOs to be deserializable.
#[derive(Debug, Clone, Deserialize)]
struct InboundMessage {
  #[serde(default = "default_role")]
  role: String,
  #[serde(default)]
  content: Option<String>,
}

fn default_role() -> String {
  "user".to_string()
}

impl InboundMessage {
  fn into_chat_message(self) -> ChatMessage {
    ChatMessage {
      role: match self.role.as_str() {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
      },
      content: self.content,
      tool_call: None,
    }
  }
}

/// One scripted request the mock routes into a subscribed agent.
#[derive(Debug, Clone, Deserialize)]
struct Request {
  pub model: String,
  #[serde(default)]
  pub messages: Vec<InboundMessage>,
  #[serde(default)]
  pub tools: Vec<Tool>,
  /// Whether the client asked for SSE. Informational only; the mock drains
  /// the same session either way.
  #[serde(default)]
  pub stream: bool,
  /// Ordering gate: absent or `"start"` fires as soon as the model is
  /// subscribed; a `call`/`inbound` pattern waits for a matching trace event.
  #[serde(default)]
  pub after: Option<After>,
}

/// Impl-specific configuration for the endpoint mock.
#[derive(Debug, Clone, Deserialize)]
struct Config {
  #[serde(default)]
  pub requests: Vec<Request>,
  /// How long to wait between polls for a model subscription.
  #[serde(default = "default_poll_ms")]
  pub poll_ms: u64,
}

fn default_poll_ms() -> u64 {
  10
}

/// A scripted endpoint client.
pub struct MockEndpoint {
  requests: Vec<Request>,
  poll: Duration,
}

impl Factory for MockEndpoint {
  fn build(params: &Value) -> anyhow::Result<Arc<Self>> {
    let config = Config::deserialize(params)
      .with_context(|| "invalid mock endpoint config".to_string())?;
    Ok(Arc::new(MockEndpoint {
      requests: config.requests,
      poll: Duration::from_millis(config.poll_ms),
    }))
  }
}

#[async_trait::async_trait]
impl Endpoint for MockEndpoint {
  fn kind() -> &'static str {
    "mock"
  }

  async fn serve(
    &self,
    bus: Arc<MessageBus>,
    registry: Arc<EndpointRegistry>,
    shutdown: Shutdown,
  ) -> anyhow::Result<()> {
    // Log the trace once, before any agent runs, so a gate can observe any
    // event, including one that precedes the request that waits on it.
    let trace = bus.trace_sender().map(TraceLog::attach);
    for request in &self.requests {
      if shutdown.is_requested() {
        return Ok(());
      }
      let Some((agent, subscription)) =
        wait_subscribed(&bus, &request.model, self.poll, &shutdown).await
      else {
        return Ok(());
      };
      if let Some(after) = &request.after
        && !wait_for_trace(trace.as_ref(), after, &shutdown).await
      {
        return Ok(());
      }
      tracing::debug!(
        model = %request.model,
        agent = %agent,
        stream = request.stream,
        "endpoint mock routing a request"
      );
      let open = Arc::clone(&registry).open(&agent, &subscription);
      let session = open.session.clone();
      let messages: Vec<ChatMessage> = request
        .messages
        .iter()
        .cloned()
        .map(InboundMessage::into_chat_message)
        .collect();
      if let Err(error) = bus.endpoint_route(
        &request.model,
        &session,
        messages,
        request.tools.clone(),
      ) {
        tracing::warn!(model = %request.model, error = %error, "endpoint mock route miss");
        registry.remove_silent(&session);
        continue;
      }
      drain(&registry, &session, open.rx, &shutdown).await;
    }
    Ok(())
  }
}

/// Poll until `model` is subscribed, or shutdown is requested.
async fn wait_subscribed(
  bus: &MessageBus,
  model: &str,
  poll: Duration,
  shutdown: &Shutdown,
) -> Option<(String, String)> {
  loop {
    if shutdown.is_requested() {
      return None;
    }
    if let Some(pair) = bus.endpoint_lookup(model) {
      return Some(pair);
    }
    tokio::time::sleep(poll).await;
  }
}

/// Wait until `after`'s gate is satisfied, or shutdown is requested. Returns
/// `true` to proceed, `false` on shutdown. The gate itself (degraded behavior
/// included) lives in [`TraceLog::wait_for`].
async fn wait_for_trace(
  trace: Option<&Arc<TraceLog>>,
  after: &After,
  shutdown: &Shutdown,
) -> bool {
  match trace {
    Some(trace) => tokio::select! {
      biased;
      () = shutdown.wait() => false,
      () = trace.wait_for(after) => true,
    },
    None => {
      if !after.is_start() {
        tracing::warn!(
          "endpoint mock `after` has no trace channel; firing immediately"
        );
      }
      true
    }
  }
}

/// Drain one session's outbound deltas until it closes, the client-side
/// receiver ends, or shutdown is requested. The deltas are not traced: the
/// brain's own `stream_endpoint` calls already record them. The session is
/// always removed afterwards so a shutdown mid-reply fires no abort event.
async fn drain(
  registry: &EndpointRegistry,
  session: &str,
  mut rx: SessionRx,
  shutdown: &Shutdown,
) {
  loop {
    tokio::select! {
      biased;
      () = shutdown.wait() => break,
      outbound = rx.recv() => match outbound {
        Some(Outbound::Delta(_)) => {}
        Some(Outbound::Close) | None => break,
      },
    }
  }
  registry.remove_silent(session);
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::host::events::Event;
  use crate::host::trace::TraceEvent;
  use crate::provider::ChatDelta;

  fn delta(content: Option<&str>, finish_reason: Option<&str>) -> ChatDelta {
    ChatDelta {
      content: content.map(str::to_string),
      tool_call: None,
      finish_reason: finish_reason.map(str::to_string),
    }
  }

  #[test]
  fn poll_ms_defaults_and_overrides() -> anyhow::Result<()> {
    let default = MockEndpoint::build(&serde_json::json!({}))?;
    assert_eq!(default.poll, Duration::from_millis(10));
    let custom = MockEndpoint::build(&serde_json::json!({ "poll_ms": 25 }))?;
    assert_eq!(custom.poll, Duration::from_millis(25));
    Ok(())
  }

  #[tokio::test]
  async fn routes_a_request_and_drains_the_reply() -> anyhow::Result<()> {
    let (trace_tx, _trace_rx) = tokio::sync::broadcast::channel(16);
    let bus = Arc::new(MessageBus::with_trace(
      crate::config::Tunables::default(),
      trace_tx,
    ));
    bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let endpoint = MockEndpoint::build(&serde_json::json!({
      "requests": [
        { "model": "gpt-4o", "messages": [{ "content": "hi" }] },
      ],
    }))?;

    // A fake agent that answers each routed request with one delta then a
    // terminal `stop`, returning the session it saw.
    let agent_bus = Arc::clone(&bus);
    let agent_registry = Arc::clone(&registry);
    let agent = tokio::spawn(async move {
      loop {
        match agent_bus.try_recv("alice") {
          Ok(Some(envelope)) => {
            if let Event::EndpointMessage(message) = envelope.event {
              let _ = agent_registry.push(
                "alice",
                &message.session,
                delta(Some("hello"), None),
              );
              let _ = agent_registry.push(
                "alice",
                &message.session,
                delta(None, Some("stop")),
              );
              return message.session;
            }
          }
          Ok(None) => tokio::task::yield_now().await,
          Err(_) => return String::new(),
        }
      }
    });

    endpoint
      .serve(Arc::clone(&bus), Arc::clone(&registry), Shutdown::new())
      .await?;
    let session = agent.await?;
    assert!(!session.is_empty());

    // The drain removed the session once it closed.
    let err = registry
      .push("alice", &session, delta(None, None))
      .unwrap_err();
    assert_eq!(err, "unknown endpoint session");
    Ok(())
  }

  #[tokio::test]
  async fn after_gates_a_request_on_a_matching_trace_event()
  -> anyhow::Result<()> {
    let (trace_tx, _trace_rx) = tokio::sync::broadcast::channel(16);
    let bus = Arc::new(MessageBus::with_trace(
      crate::config::Tunables::default(),
      trace_tx,
    ));
    bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    let endpoint = MockEndpoint::build(&serde_json::json!({
      "requests": [{
        "model": "gpt-4o",
        "messages": [{ "content": "hi" }],
        "after": { "kind": "call", "op": "go" },
      }],
    }))?;

    let agent_bus = Arc::clone(&bus);
    let agent_registry = Arc::clone(&registry);
    let agent = tokio::spawn(async move {
      loop {
        match agent_bus.try_recv("alice") {
          Ok(Some(envelope)) => {
            if let Event::EndpointMessage(message) = envelope.event {
              let _ = agent_registry.push(
                "alice",
                &message.session,
                delta(None, Some("stop")),
              );
              return true;
            }
          }
          Ok(None) => tokio::task::yield_now().await,
          Err(_) => return false,
        }
      }
    });

    // A non-matching call first, then the matching one.
    let emitter_bus = Arc::clone(&bus);
    let emitter = tokio::spawn(async move {
      emitter_bus.trace_event(TraceEvent::Call {
        agent: "alice".to_string(),
        op: "other".to_string(),
        detail: serde_json::json!({}),
      });
      tokio::task::yield_now().await;
      emitter_bus.trace_event(TraceEvent::Call {
        agent: "alice".to_string(),
        op: "go".to_string(),
        detail: serde_json::json!({}),
      });
    });

    endpoint
      .serve(Arc::clone(&bus), Arc::clone(&registry), Shutdown::new())
      .await?;
    emitter.await?;
    assert!(agent.await?);
    Ok(())
  }
}
