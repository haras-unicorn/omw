//! A scripted in-process endpoint double for `omw-test`: it plays the client.
//!
//! Instead of listening on a socket it reads `requests = [{ model, messages,
//! tools, stream }]` from its params, waits until the brain subscribes to each
//! model, routes the request into the owning agent's inbox, and drains the
//! session's outbound deltas back onto the trace channel. No socket and no
//! `listen`, so it stays safe for `dev test fast`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{Endpoint, EndpointInjections, Factory};
use crate::host::bus::{Injection, MessageBus};
use crate::host::endpoint::{EndpointRegistry, Outbound, SessionRx};
use crate::host::trace::TraceEvent;
use crate::provider::{ChatDelta, ChatMessage, Role};
use crate::shutdown::Shutdown;
use crate::testing::After;
use crate::tooling::Tool;

/// One inbound chat message from the scripted client. Kept local so the mock
/// does not require the provider DTOs to be deserializable.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct InboundMessage {
  #[serde(default = "default_role")]
  role: String,
  #[serde(default)]
  content: Option<String>,
  #[serde(default)]
  reasoning: Option<String>,
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
      reasoning: self.reasoning,
      tool_call: None,
    }
  }
}

/// One scripted request the mock routes into a subscribed agent.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct Request {
  pub model: String,
  /// A user-written label for the session, so concurrent sessions can be told
  /// apart. The mock correlates it with the opaque session UUID the host and
  /// brain see and records both in the snapshot; it is never asserted or
  /// matched. Reusing a label after its session closed is a new session with a
  /// new UUID.
  pub session_id: String,
  #[serde(default)]
  pub messages: Vec<InboundMessage>,
  #[serde(default)]
  pub tools: Vec<Tool>,
  /// Whether the client asked for SSE. Informational only; the mock drains
  /// the same session either way.
  #[serde(default)]
  pub stream: bool,
  /// Opaque generation params the client submitted (temperature, …).
  #[serde(default)]
  pub params: Option<Value>,
  /// Ordering gate: absent or `"start"` fires as soon as the model is
  /// subscribed; a `call`/`inbound` pattern waits for a matching trace event.
  #[serde(default)]
  pub after: Option<After>,
  /// Opt-in scripted ending: `{ close = N }` ends the session normally after
  /// `N` received deltas, `{ abort = N }` aborts it. Absent, the mock drains
  /// silently as before.
  #[serde(default)]
  pub session_end: Option<SessionEnd>,
}

/// How a scripted request ends its session.
///
/// `{ close = N }` removes the session and delivers a normal
/// `endpoint-session-end` after `N` received deltas; `{ abort = N }` aborts it
/// with an error instead. `N = 0` fires immediately after routing. If the
/// reply ends first (terminal finish-reason or `Close`) the script is a no-op.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct SessionEnd {
  #[serde(default)]
  close: Option<usize>,
  #[serde(default)]
  abort: Option<usize>,
}

impl SessionEnd {
  /// The number of deltas after which to end, and whether to abort. `abort`
  /// takes precedence when both are written.
  fn trigger(&self) -> Option<(usize, bool)> {
    if let Some(abort) = self.abort {
      Some((abort, true))
    } else {
      self.close.map(|close| (close, false))
    }
  }
}

/// Impl-specific configuration for the endpoint mock.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct Config {
  #[serde(default)]
  pub requests: Vec<Request>,
}

/// One drained endpoint reply: the deltas the scripted client received and how
/// the session ended.
#[derive(Debug)]
pub struct EndpointReply {
  pub model: String,
  /// The request's user label for the session.
  pub session_id: String,
  /// The opaque session UUID the host and brain saw.
  pub session: String,
  pub deltas: Vec<ChatDelta>,
  /// The reply ended normally (a terminal `Close` or a scripted `close = N`).
  pub closed: bool,
  /// The session was aborted (a scripted `abort = N`).
  pub aborted: bool,
  /// The `endpoint-session-end` error when the session was aborted.
  pub error: Option<String>,
}

/// A scripted endpoint client.
pub struct MockEndpoint {
  requests: Arc<Vec<Request>>,
  /// How many scripted requests have been routed so far.
  fired: Arc<AtomicUsize>,
  /// What each routed request received back.
  replies: Arc<Mutex<Vec<EndpointReply>>>,
}

impl Factory for MockEndpoint {
  fn build(params: &Value) -> anyhow::Result<Arc<Self>> {
    let config = Config::deserialize(params)
      .with_context(|| "invalid mock endpoint config".to_string())?;
    Ok(Arc::new(MockEndpoint {
      requests: Arc::new(config.requests),
      fired: Arc::new(AtomicUsize::new(0)),
      replies: Arc::new(Mutex::new(Vec::new())),
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
    _bus: Arc<MessageBus>,
    _registry: Arc<EndpointRegistry>,
    shutdown: Shutdown,
  ) -> anyhow::Result<()> {
    // The scripted requests are routed by call-boundary injections registered
    // synchronously before the run starts (see `inject_requests`). `serve` only
    // keeps the endpoint task alive until shutdown, matching a live server; it
    // never races the guest.
    shutdown.wait().await;
    Ok(())
  }

  fn snapshot(&self) -> Option<Value> {
    let fired = self.fired.load(Ordering::Relaxed);
    let replies = self.replies.lock().ok().map(|replies| {
      replies
        .iter()
        .map(|reply| {
          serde_json::json!({
            "model": reply.model,
            "session_id": reply.session_id,
            "session": reply.session,
            "deltas": reply.deltas,
            "closed": reply.closed,
            "aborted": reply.aborted,
            "error": reply.error,
          })
        })
        .collect::<Vec<_>>()
    })?;
    Some(serde_json::json!({
      "kind": "endpoint",
      "requests_total": self.requests.len(),
      "requests_fired": fired,
      "requests_remaining": self.requests.len().saturating_sub(fired),
      "replies": replies,
    }))
  }
}

/// Whether a request's `after` gate has been observed. A missing gate (or
/// `"start"`) fires as soon as the model is subscribed.
fn after_ready(after: &Option<After>, observed: &[TraceEvent]) -> bool {
  match after {
    None | Some(After::Start) => true,
    Some(After::Pattern(assertion)) => {
      observed.iter().any(|event| assertion.matches(event))
    }
  }
}

impl EndpointInjections for MockEndpoint {
  fn inject_requests(
    &self,
    bus: &Arc<MessageBus>,
    registry: &Arc<EndpointRegistry>,
    shutdown: &Shutdown,
  ) {
    for request in self.requests.iter().cloned() {
      let registry = Arc::clone(registry);
      let shutdown = shutdown.clone();
      let fired = Arc::clone(&self.fired);
      let replies = Arc::clone(&self.replies);
      bus.register_injection(Injection::new(move |bus, observed, rt| {
        // Not routable until the model is subscribed: stay pending.
        let Some((agent, subscription)) = bus.endpoint_lookup(&request.model)
        else {
          return false;
        };
        if !after_ready(&request.after, observed) {
          return false;
        }
        tracing::debug!(
          model = %request.model,
          session_id = %request.session_id,
          stream = request.stream,
          "endpoint mock routing a request"
        );
        fired.fetch_add(1, Ordering::Relaxed);
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
          request.params.clone(),
        ) {
          tracing::warn!(model = %request.model, error = %error, "endpoint mock route miss");
          registry.remove_silent(&session);
          return true;
        }
        rt.spawn(drain(
          Arc::clone(&registry),
          session,
          request.session_id.clone(),
          open.rx,
          shutdown.clone(),
          Arc::clone(&replies),
          request.model.clone(),
          request.session_end.clone(),
        ));
        true
      }));
    }
  }
}

/// Drain one session's outbound deltas until it closes, the client-side
/// receiver ends, or shutdown is requested. The deltas are not traced: the
/// brain's own `stream_endpoint` calls already record them.
///
/// A scripted `session_end` ends the session once `N` deltas have been
/// received: `close` removes it and delivers a normal `endpoint-session-end`
/// (via [`EndpointRegistry::close`]); `abort` aborts it (`registry.abort`).
/// If the reply ends first (terminal finish-reason / `Close`) or shutdown
/// arrives before `N`, the script is a no-op and the session is removed
/// silently, exactly as before. The drained reply is always recorded.
#[allow(
  clippy::too_many_arguments,
  reason = "aggregating the drain handles into a struct is left to a pumps refactor"
)]
async fn drain(
  registry: Arc<EndpointRegistry>,
  session: String,
  session_id: String,
  mut rx: SessionRx,
  shutdown: Shutdown,
  replies: Arc<Mutex<Vec<EndpointReply>>>,
  model: String,
  session_end: Option<SessionEnd>,
) {
  let trigger = session_end.as_ref().and_then(SessionEnd::trigger);
  let mut deltas = Vec::new();
  let mut closed = false;
  let mut aborted = false;
  let mut error: Option<String> = None;
  loop {
    if let Some((after, is_abort)) = trigger
      && deltas.len() >= after
    {
      if is_abort {
        registry.abort(&session);
        aborted = true;
        error = Some("endpoint session aborted".to_string());
      } else {
        registry.close(&session);
        closed = true;
      }
      break;
    }
    tokio::select! {
      biased;
      () = shutdown.wait() => break,
      outbound = rx.recv() => match outbound {
        Some(Outbound::Delta(delta)) => deltas.push(delta),
        Some(Outbound::Close) => {
          closed = true;
          break;
        }
        None => break,
      },
    }
  }
  registry.remove_silent(&session);
  if let Ok(mut replies) = replies.lock() {
    replies.push(EndpointReply {
      model,
      session_id,
      session,
      deltas,
      closed,
      aborted,
      error,
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::host::events::Event;
  use crate::host::trace::TraceEvent;
  use crate::provider::ChatDelta;
  use std::time::Duration;

  fn delta(content: Option<&str>, finish_reason: Option<&str>) -> ChatDelta {
    ChatDelta {
      content: content.map(str::to_string),
      reasoning: None,
      tool_call: None,
      finish_reason: finish_reason.map(str::to_string),
      usage: None,
    }
  }

  /// Wait until the mock has recorded a drained reply.
  async fn wait_for_replies(
    endpoint: &Arc<MockEndpoint>,
  ) -> anyhow::Result<()> {
    for _ in 0..1000 {
      let recorded = endpoint
        .snapshot()
        .and_then(|snapshot| {
          snapshot["replies"]
            .as_array()
            .map(|replies| !replies.is_empty())
        })
        .unwrap_or(false);
      if recorded {
        return Ok(());
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    anyhow::bail!("no endpoint reply was recorded")
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
        {
          "model": "gpt-4o",
          "session_id": "s1",
          "messages": [{ "content": "hi" }],
        },
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

    endpoint.inject_requests(&bus, &registry, &Shutdown::new());
    bus.flush_injections(&tokio::runtime::Handle::current());
    let session = agent.await?;
    assert!(!session.is_empty());
    wait_for_replies(&endpoint).await?;

    // The drain removed the session once it closed.
    let err = registry
      .push("alice", &session, delta(None, None))
      .unwrap_err();
    assert_eq!(err, "unknown endpoint session");

    // The drained reply is recorded for the snapshot.
    let snapshot = endpoint
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["replies"][0]["model"], "gpt-4o");
    assert_eq!(snapshot["replies"][0]["session_id"], "s1");
    assert_eq!(snapshot["replies"][0]["session"], session.as_str());
    assert_eq!(snapshot["replies"][0]["deltas"][0]["content"], "hello");
    assert_eq!(snapshot["replies"][0]["closed"], true);
    assert_eq!(snapshot["replies"][0]["aborted"], false);
    assert!(snapshot["replies"][0]["error"].is_null());
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
        "session_id": "s1",
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
    endpoint.inject_requests(&bus, &registry, &Shutdown::new());
    bus.trace_event(TraceEvent::Call {
      agent: "alice".to_string(),
      op: "other".to_string(),
      uuid: None,
      detail: serde_json::json!({}),
    });
    bus.flush_injections(&tokio::runtime::Handle::current());
    bus.trace_event(TraceEvent::Call {
      agent: "alice".to_string(),
      op: "go".to_string(),
      uuid: None,
      detail: serde_json::json!({}),
    });
    bus.flush_injections(&tokio::runtime::Handle::current());
    assert!(agent.await?);
    Ok(())
  }

  #[test]
  fn snapshot_reports_request_counts() -> anyhow::Result<()> {
    let endpoint = MockEndpoint::build(&serde_json::json!({
      "requests": [
        { "model": "a", "session_id": "s1" },
        { "model": "b", "session_id": "s2" },
      ],
    }))?;
    let snapshot = endpoint
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["requests_total"], 2);
    assert_eq!(snapshot["requests_fired"], 0);
    assert_eq!(snapshot["requests_remaining"], 2);
    Ok(())
  }

  #[test]
  fn session_id_is_required() {
    assert!(
      MockEndpoint::build(&serde_json::json!({
        "requests": [{ "model": "a" }],
      }))
      .is_err()
    );
  }

  /// Build a bus with `alice` subscribed to `gpt-4o`, and a registry.
  fn subscribed() -> anyhow::Result<(Arc<MessageBus>, Arc<EndpointRegistry>)> {
    let bus = Arc::new(MessageBus::new());
    bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    Ok((bus, registry))
  }

  /// Wait for the single `endpoint-session-end` on `alice`'s inbox and return
  /// its session and error.
  async fn session_end(
    bus: &MessageBus,
  ) -> anyhow::Result<(String, Option<String>)> {
    for _ in 0..1000 {
      if let Some(envelope) = bus.try_recv("alice")? {
        if let Event::EndpointSessionEnd(end) = envelope.event {
          return Ok((end.session, end.error));
        }
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    anyhow::bail!("no endpoint-session-end arrived")
  }

  #[tokio::test]
  async fn scripted_close_delivers_a_normal_session_end() -> anyhow::Result<()>
  {
    let (bus, registry) = subscribed()?;
    let endpoint = MockEndpoint::build(&serde_json::json!({
      "requests": [{
        "model": "gpt-4o",
        "session_id": "s1",
        "messages": [{ "content": "hi" }],
        "session_end": { "close": 0 },
      }],
    }))?;

    // The brain does not answer: the scripted close fires immediately after
    // routing, so a normal end is delivered without any delta.
    endpoint.inject_requests(&bus, &registry, &Shutdown::new());
    bus.flush_injections(&tokio::runtime::Handle::current());

    let (session, error) = session_end(&bus).await?;
    assert!(error.is_none());
    let snapshot = endpoint
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["replies"][0]["session"], session.as_str());
    assert_eq!(snapshot["replies"][0]["session_id"], "s1");
    assert_eq!(snapshot["replies"][0]["closed"], true);
    assert_eq!(snapshot["replies"][0]["aborted"], false);
    Ok(())
  }

  #[tokio::test]
  async fn scripted_abort_delivers_an_error_session_end() -> anyhow::Result<()>
  {
    let (bus, registry) = subscribed()?;
    let endpoint = MockEndpoint::build(&serde_json::json!({
      "requests": [{
        "model": "gpt-4o",
        "session_id": "s1",
        "messages": [{ "content": "hi" }],
        "session_end": { "abort": 0 },
      }],
    }))?;

    endpoint.inject_requests(&bus, &registry, &Shutdown::new());
    bus.flush_injections(&tokio::runtime::Handle::current());

    let (session, error) = session_end(&bus).await?;
    assert_eq!(error.as_deref(), Some("endpoint session aborted"));
    let snapshot = endpoint
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["replies"][0]["session"], session.as_str());
    assert_eq!(snapshot["replies"][0]["closed"], false);
    assert_eq!(snapshot["replies"][0]["aborted"], true);
    assert_eq!(snapshot["replies"][0]["error"], "endpoint session aborted");
    Ok(())
  }

  #[tokio::test]
  async fn scripted_close_waits_for_n_deltas() -> anyhow::Result<()> {
    let (bus, registry) = subscribed()?;
    let endpoint = MockEndpoint::build(&serde_json::json!({
      "requests": [{
        "model": "gpt-4o",
        "session_id": "s1",
        "messages": [{ "content": "hi" }],
        "session_end": { "close": 1 },
      }],
    }))?;

    // The brain streams one content delta and (deliberately) no terminal; the
    // scripted close then ends the session normally.
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
              return;
            }
          }
          Ok(None) => tokio::task::yield_now().await,
          Err(_) => return,
        }
      }
    });

    endpoint.inject_requests(&bus, &registry, &Shutdown::new());
    bus.flush_injections(&tokio::runtime::Handle::current());
    agent.await?;
    wait_for_replies(&endpoint).await?;

    let (_session, error) = session_end(&bus).await?;
    assert!(error.is_none());
    let snapshot = endpoint
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("expected a snapshot"))?;
    assert_eq!(snapshot["replies"][0]["deltas"][0]["content"], "hello");
    assert_eq!(snapshot["replies"][0]["closed"], true);
    Ok(())
  }
}
