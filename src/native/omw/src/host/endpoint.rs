//! Host-side endpoint session registry. An endpoint request routed to an
//! agent opens a session: a tagged, buffered channel the agent streams
//! `chat-delta`s back into with `host.endpoint-stream`, non-blocking, and the
//! endpoint's HTTP handler drains. Sessions end exactly once: normally when the
//! agent's reply reaches a terminal `finish-reason`, or abruptly when the
//! client disconnects (receiver drop) or the subscription is cancelled; each
//! fires a single `endpoint-session-end` event into the owning agent's inbox.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc;

use crate::host::bus::MessageBus;
use crate::host::events::{EndpointSessionEnd, Event};
use crate::provider::ChatDelta;

/// The number of deltas a single session's local buffer holds before further
/// `endpoint-stream` calls drop chunks (with a `tracing::warn`). Generous,
/// since a reply streams many small chunks, so the warn path is an emergency
/// lane rather than a throttle.
pub const SESSION_BUFFER: usize = 8192;

/// What an endpoint HTTP handler receives from the session channel.
#[derive(Debug, Clone, PartialEq)]
pub enum Outbound {
  /// One streamed delta of the agent's reply.
  Delta(ChatDelta),
  /// The reply reached a terminal finish reason; end the session normally.
  Close,
}

/// A session as read by the endpoint HTTP handler: the receiver half of the
/// channel plus the metadata needed to tag the terminal `endpoint-session-end`
/// event.
pub struct OpenSession {
  /// UUID of the session.
  pub session: String,
  /// The agent the request was routed to.
  pub agent: String,
  /// The subscription UUID the request arrived under; the terminal
  /// `endpoint-session-end` event is tagged with it.
  pub subscription: String,
  /// Buffered channel receiver draining the agent's streamed deltas.
  pub rx: SessionRx,
}

/// The owned receiver half of a session channel. A `Drop` fires an abrupt
/// session end when the receiver vanishes without the agent's reply having
/// completed normally.
pub struct SessionRx {
  inner: mpsc::Receiver<Outbound>,
  registry: Arc<EndpointRegistry>,
  session: String,
}

impl SessionRx {
  pub async fn recv(&mut self) -> Option<Outbound> {
    self.inner.recv().await
  }

  /// Close the channel without dropping the receiver, so no abort fires.
  /// Test-only: lets `push` hit its `Closed` error path deterministically.
  #[cfg(test)]
  pub fn close(&mut self) {
    self.inner.close();
  }
}

impl Drop for SessionRx {
  fn drop(&mut self) {
    self.registry.abort(&self.session);
  }
}

/// One open endpoint session, keyed by its session UUID.
#[derive(Debug)]
struct Session {
  /// The agent the session belongs to.
  agent: String,
  /// The subscription UUID the session was opened under.
  subscription: String,
  /// The buffered channel sender the agent streams deltas into.
  tx: mpsc::Sender<Outbound>,
}

/// The shared endpoint session registry: created once per process and shared
/// between the HTTP server task and every agent context. All access is
/// synchronous (`Mutex` + `try_send`), so the wasm host calls need no async
/// bridge.
#[derive(Debug)]
pub struct EndpointRegistry {
  bus: Arc<MessageBus>,
  sessions: Mutex<HashMap<String, Session>>,
}

impl EndpointRegistry {
  pub fn new(bus: Arc<MessageBus>) -> Self {
    Self {
      bus,
      sessions: Mutex::new(HashMap::new()),
    }
  }

  /// Open a new session for `agent` under `subscription`, returning the
  /// receiver half plus the session's metadata.
  pub fn open(self: Arc<Self>, agent: &str, subscription: &str) -> OpenSession {
    let session = crate::host::bus::new_uuid();
    let (tx, rx) = mpsc::channel(SESSION_BUFFER);
    let mut guards = self.locked();
    guards.insert(
      session.clone(),
      Session {
        agent: agent.to_string(),
        subscription: subscription.to_string(),
        tx,
      },
    );
    drop(guards);
    OpenSession {
      session: session.clone(),
      agent: agent.to_string(),
      subscription: subscription.to_string(),
      rx: SessionRx {
        inner: rx,
        registry: Arc::clone(&self),
        session,
      },
    }
  }

  /// Stream one delta into the session's buffer, non-blocking. Errors if the
  /// session is unknown, belongs to another agent, or has already ended
  /// (terminal finish, abort, or unsubscribe); errors if the receiver is
  /// gone. A full buffer only warns and drops the chunk (8192 emergency
  /// lane). A terminal `finish-reason` delta ends the session normally: the
  /// entry is removed, and a `Close` is queued so the HTTP handler can flush
  /// the reply.
  pub fn push(
    &self,
    agent: &str,
    session: &str,
    delta: ChatDelta,
  ) -> Result<(), String> {
    let terminal = delta.finish_reason.is_some();
    let tx = {
      let mut guards = self.locked();
      let Some(entry) = guards.get_mut(session) else {
        return Err("unknown endpoint session".to_string());
      };
      if entry.agent != agent {
        return Err("endpoint session belongs to another agent".to_string());
      }
      entry.tx.clone()
    };
    if terminal {
      // Surface the terminal `finish-reason` chunk itself first, then Close so
      // the handler can flush the reply-and-end shape OpenAI clients expect.
      match tx.try_send(Outbound::Delta(delta)) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Closed(_)) => {
          return Err("endpoint session receiver closed".to_string());
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
          tracing::warn!(
            agent,
            session = %session,
            "endpoint session buffer full; terminal delta dropped"
          );
        }
      }
      let mut guards = self.locked();
      let _ = guards.remove(session);
      drop(guards);
      match tx.try_send(Outbound::Close) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Closed(_)) => {
          return Err("endpoint session receiver closed".to_string());
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
          tracing::warn!(
            agent,
            session = %session,
            "endpoint session buffer full; close marker dropped"
          );
        }
      }
    } else {
      match tx.try_send(Outbound::Delta(delta)) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Closed(_)) => {
          return Err("endpoint session receiver closed".to_string());
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
          tracing::warn!(
            agent,
            session = %session,
            "endpoint session buffer full; delta dropped"
          );
        }
      }
    }
    Ok(())
  }

  /// Remove a session without firing any `endpoint-session-end` event. Used
  /// when the server opened a session but never routed it (e.g. a route
  /// miss), so the agent never saw an `endpoint-message` and must not get
  /// an end for an unknown session.
  pub fn remove_silent(&self, session: &str) {
    let mut guards = self.locked();
    guards.remove(session);
  }

  /// Abruptly end a session (e.g. the endpoint client disconnected, dropping
  /// the receiver half). Fires a single `endpoint-session-end` with an error;
  /// idempotent once the entry is removed, so it never double-fires.
  pub fn abort(&self, session: &str) {
    let mut guards = self.locked();
    let Some(entry) = guards.remove(session) else {
      return;
    };
    drop(guards);
    drop(entry.tx);
    tracing::debug!(
      agent = %entry.agent,
      session = %session,
      "endpoint session aborted"
    );
    self.deliver_ended(
      &entry.agent,
      &entry.subscription,
      session,
      Some("endpoint session aborted".to_string()),
    );
  }

  /// Abruptly end every session opened under `subscription` (e.g. the agent
  /// unsubscribed from the endpoint). Each fires a single
  /// `endpoint-session-end` with an error.
  pub fn cancel_subscription(&self, subscription: &str) {
    let mut guards = self.locked();
    let ids: Vec<String> = guards
      .iter()
      .filter(|(_, s)| s.subscription == *subscription)
      .map(|(id, _)| id.clone())
      .collect();
    let mut aborted: Vec<(String, String)> = Vec::new();
    for id in ids {
      let Some(entry) = guards.remove(&id) else {
        continue;
      };
      aborted.push((entry.agent, id));
    }
    drop(guards);
    for (agent, id) in aborted {
      tracing::debug!(
        agent = %agent,
        session = %id,
        subscription = %subscription,
        "endpoint session terminated by unsubscribe"
      );
      self.deliver_ended(
        &agent,
        subscription,
        &id,
        Some(format!(
          "endpoint subscription {subscription:?} unsubscribed"
        )),
      );
    }
  }

  /// Deliver the terminal `endpoint-session-end` event for a session into the
  /// owning agent's inbox, tagged with the subscription UUID. `error` is
  /// absent on a normal completion and present when the session was
  /// interrupted.
  pub fn deliver_ended(
    &self,
    agent: &str,
    subscription: &str,
    session: &str,
    error: Option<String>,
  ) {
    self.bus.deliver(
      agent,
      subscription,
      Event::EndpointSessionEnd(EndpointSessionEnd {
        session: session.to_string(),
        error,
      }),
    );
  }

  fn locked(&self) -> MutexGuard<'_, HashMap<String, Session>> {
    self
      .sessions
      .lock()
      .unwrap_or_else(|poison| poison.into_inner())
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use super::*;
  use crate::host::bus::MessageBus;
  use crate::provider::ChatDelta;

  fn busy_registry() -> (Arc<MessageBus>, Arc<EndpointRegistry>) {
    let bus = Arc::new(MessageBus::new());
    (Arc::clone(&bus), Arc::new(EndpointRegistry::new(bus)))
  }

  fn delta(
    content: Option<String>,
    finish_reason: Option<String>,
  ) -> ChatDelta {
    ChatDelta {
      content,
      tool_call: None,
      finish_reason,
    }
  }

  #[tokio::test]
  async fn push_streams_delta_then_terminal_close_and_removes_session()
  -> anyhow::Result<()> {
    let (bus, registry) = busy_registry();
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let mut open = registry.clone().open("alice", &sub);
    registry
      .push("alice", &open.session, delta(Some("hi".to_string()), None))
      .map_err(|e| anyhow::anyhow!(e))?;
    assert_eq!(
      open.rx.recv().await,
      Some(Outbound::Delta(delta(Some("hi".to_string()), None)))
    );
    registry
      .push(
        "alice",
        &open.session,
        delta(None, Some("stop".to_string())),
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    assert_eq!(
      open.rx.recv().await,
      Some(Outbound::Delta(delta(None, Some("stop".to_string()))))
    );
    assert_eq!(open.rx.recv().await, Some(Outbound::Close));
    // After the terminal chunk the session is gone, so further pushes error.
    assert_eq!(
      registry.push("alice", &open.session, delta(None, None)),
      Err("unknown endpoint session".to_string()),
    );
    Ok(())
  }

  #[test]
  fn push_rejects_unknown_session_and_foreign_agent() -> anyhow::Result<()> {
    let (bus, registry) = busy_registry();
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let open = registry.clone().open("alice", &sub);
    assert_eq!(
      registry.push("alice", "no-such-session", delta(None, None)),
      Err("unknown endpoint session".to_string()),
    );
    assert_eq!(
      registry.push("bob", &open.session, delta(None, None)),
      Err("endpoint session belongs to another agent".to_string()),
    );
    Ok(())
  }

  #[tokio::test]
  async fn push_errors_when_receiver_closed() -> anyhow::Result<()> {
    let (bus, registry) = busy_registry();
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let mut open = registry.clone().open("alice", &sub);
    open.rx.close();
    assert_eq!(
      registry.push("alice", &open.session, delta(None, None)),
      Err("endpoint session receiver closed".to_string()),
    );
    assert_eq!(
      registry.push(
        "alice",
        &open.session,
        delta(None, Some("stop".to_string()))
      ),
      Err("endpoint session receiver closed".to_string()),
    );
    Ok(())
  }

  #[test]
  fn push_after_receiver_drop_reads_as_unknown_since_drop_aborts()
  -> anyhow::Result<()> {
    let (bus, registry) = busy_registry();
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
    match envelope.event {
      Event::EndpointSessionEnd(end) => {
        assert_eq!(end.session, session);
        assert_eq!(end.error.as_deref(), Some("endpoint session aborted"));
      }
      other => assert!(false, "unexpected event: {other:?}"),
    }
    // Abort is idempotent: once removed, the entry never fires again.
    registry.abort(&session);
    assert!(bus.try_recv("alice")?.is_none());
    Ok(())
  }

  #[test]
  fn cancel_subscription_ends_every_session_of_that_subscription()
  -> anyhow::Result<()> {
    let (bus, registry) = busy_registry();
    let sub = bus
      .endpoint_subscribe("alice", "m1".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let sub2 = bus
      .endpoint_subscribe("alice", "m2".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let open1 = registry.clone().open("alice", &sub);
    let open2 = registry.clone().open("alice", &sub);
    let _open3 = registry.clone().open("alice", &sub2);
    registry.cancel_subscription(&sub);
    let mut sessions = Vec::new();
    let envelope = bus
      .try_recv("alice")?
      .ok_or_else(|| anyhow::anyhow!("expected an endpoint-session-end"))?;
    if let Event::EndpointSessionEnd(end) = envelope.event {
      sessions.push(end.session);
    }
    let envelope = bus.try_recv("alice")?.ok_or_else(|| {
      anyhow::anyhow!("expected a second endpoint-session-end")
    })?;
    if let Event::EndpointSessionEnd(end) = envelope.event {
      sessions.push(end.session);
    }
    sessions.sort();
    let mut expected = vec![open1.session, open2.session];
    expected.sort();
    assert_eq!(sessions, expected);
    // The session under the other subscription is untouched.
    assert!(bus.try_recv("alice")?.is_none());
    Ok(())
  }
}
