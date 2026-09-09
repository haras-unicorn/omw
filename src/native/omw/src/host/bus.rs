//! Inter-agent inboxes and the subscription registry that fans messages out to
//! them. Messages only reach an agent if that agent subscribed to the sender;
//! every delivery carries the subscription's UUID so the guest can disambiguate
//! events from different sources on the single inbox.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use kanal::{Receiver, Sender};

use crate::host::events::{EndpointMessage, Event, EventEnvelope};
use crate::provider::ChatMessage;
use crate::tooling::Tool;

/// Lock a `std::sync::Mutex`, recovering the guard on poison (a poisoned lock
/// should never take the whole runtime down).
fn lock<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
  mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// A per-agent channel pair: sender + receiver.
type AgentChannels = (Sender<EventEnvelope>, Receiver<EventEnvelope>);

/// A shared bus of per-agent inboxes plus the subscription registry.
#[derive(Debug)]
pub struct MessageBus {
  inner: Mutex<BusInner>,
  tunables: crate::config::Tunables,
}

impl Default for MessageBus {
  fn default() -> Self {
    Self::with_tunables(crate::config::Tunables::default())
  }
}

/// The mutable state behind the bus lock.
#[derive(Debug, Default)]
struct BusInner {
  /// Per-agent inboxes, created lazily on demand.
  inboxes: HashMap<String, AgentChannels>,
  /// `(subscriber, source) -> subscription uuid`.
  subscriptions: HashMap<(String, String), String>,
  /// `subscription uuid` -> `(subscriber, source)` reverse index, so
  /// `unsubscribe` can remove by handle instead of re-deriving the pair.
  subscriptions_by_uuid: HashMap<String, (String, String)>,
  /// Agent -> active lifecycle subscription uuid (one per run).
  lifecycle: HashMap<String, String>,
  /// Lifecycle `subscription uuid` -> agent reverse index.
  lifecycle_by_uuid: HashMap<String, String>,
  /// Endpoint model name -> `(agent, subscription uuid)`, kept in a
  /// `BTreeMap` so `/v1/models` enumerates models in deterministic order.
  endpoint_by_model: BTreeMap<String, (String, String)>,
  /// Endpoint `subscription uuid` -> `(agent, model)` reverse index, so
  /// `endpoint-unsubscribe` can remove by handle instead of again walking
  /// the model map.
  endpoint_by_uuid: HashMap<String, (String, String)>,
}

impl MessageBus {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn with_tunables(tunables: crate::config::Tunables) -> Self {
    Self {
      inner: Mutex::new(BusInner::default()),
      tunables,
    }
  }

  /// Subscribe `subscriber` to messages from `source`, returning a fresh UUID
  /// handle that matching deliveries are tagged with.
  pub fn subscribe(&self, subscriber: &str, source: &str) -> String {
    let uuid = new_uuid();
    let mut inner = lock(&self.inner);
    let _ = self.channels_locked(&mut inner, subscriber);
    inner
      .subscriptions
      .insert((subscriber.to_string(), source.to_string()), uuid.clone());
    inner
      .subscriptions_by_uuid
      .insert(uuid.clone(), (subscriber.to_string(), source.to_string()));
    tracing::debug!(
      subscriber,
      source,
      uuid = %uuid,
      "agent subscribed to another agent"
    );
    uuid
  }

  /// Remove `subscriber`'s subscription identified by `uuid`, if any. Returns
  /// whether anything was actually removed. A foreign `uuid` handle for another
  /// subscriber is left untouched.
  pub fn unsubscribe(&self, subscriber: &str, uuid: &str) -> bool {
    let mut inner = lock(&self.inner);
    let Some((sub, src)) = inner.subscriptions_by_uuid.remove(uuid) else {
      return false;
    };
    if sub != subscriber {
      inner
        .subscriptions_by_uuid
        .insert(uuid.to_string(), (sub, src));
      return false;
    }
    inner.subscriptions.remove(&(sub, src.clone()));
    tracing::debug!(subscriber, source = %src, uuid = %uuid, "agent unsubscribed from another agent");
    true
  }

  /// Subscribe `agent` to lifecycle events, returning a fresh UUID handle
  /// that `reload` / `shutdown` / reload-failure `error` events are tagged
  /// with. Errors on a second subscribe (one lifecycle subscription per run).
  pub fn lifecycle_subscribe(&self, agent: &str) -> Result<String, String> {
    let mut inner = lock(&self.inner);
    if inner.lifecycle.contains_key(agent) {
      return Err("lifecycle already subscribed".to_string());
    }
    let uuid = new_uuid();
    inner.lifecycle.insert(agent.to_string(), uuid.clone());
    inner
      .lifecycle_by_uuid
      .insert(uuid.clone(), agent.to_string());
    tracing::debug!(agent, uuid = %uuid, "agent subscribed to lifecycle events");
    Ok(uuid)
  }

  /// Remove `agent`'s lifecycle subscription identified by `uuid`, if it is
  /// theirs. A foreign or unknown `uuid` is a no-op returning false.
  pub fn lifecycle_unsubscribe(&self, agent: &str, uuid: &str) -> bool {
    let mut inner = lock(&self.inner);
    let Some(owner) = inner.lifecycle_by_uuid.get(uuid).cloned() else {
      return false;
    };
    if owner != agent {
      return false;
    }
    inner.lifecycle_by_uuid.remove(uuid);
    inner.lifecycle.remove(agent);
    tracing::debug!(agent, uuid = %uuid, "agent unsubscribed from lifecycle events");
    true
  }

  /// The active lifecycle UUID for `agent`, if subscribed.
  pub fn lifecycle_of(&self, agent: &str) -> Option<String> {
    let inner = lock(&self.inner);
    inner.lifecycle.get(agent).cloned()
  }

  /// Subscribe `agent` to inbound endpoint requests under the model name
  /// `model`, returning a fresh UUID handle that routing deliveries are tagged
  /// with. Errors if `model` is already taken by any agent.
  pub fn endpoint_subscribe(
    &self,
    agent: &str,
    model: String,
  ) -> Result<String, String> {
    let mut inner = lock(&self.inner);
    if inner.endpoint_by_model.contains_key(&model) {
      tracing::debug!(agent,endpoint_model = %model, "endpoint model already subscribed");
      return Err(format!("model {model:?} is already subscribed"));
    }
    let uuid = new_uuid();
    inner
      .endpoint_by_model
      .insert(model.clone(), (agent.to_string(), uuid.clone()));
    inner
      .endpoint_by_uuid
      .insert(uuid.clone(), (agent.to_string(), model.clone()));
    tracing::info!(
      agent,
      endpoint_model = %model,
      uuid = %uuid,
      "agent subscribed to the endpoint"
    );
    Ok(uuid)
  }

  /// Remove `agent`'s endpoint subscription identified by `uuid`, if any.
  /// Returns the model name that was unsubscribed, or `None` when `uuid` is
  /// unknown or belongs to another agent.
  pub fn endpoint_unsubscribe(
    &self,
    agent: &str,
    uuid: &str,
  ) -> Option<String> {
    let mut inner = lock(&self.inner);
    let (sub, model) = inner.endpoint_by_uuid.remove(uuid)?;
    if sub != agent {
      inner
        .endpoint_by_uuid
        .insert(uuid.to_string(), (sub, model));
      return None;
    }
    inner.endpoint_by_model.remove(&model);
    tracing::info!(
      agent,
      endpoint_model = %model,
      uuid = %uuid,
      "agent unsubscribed from the endpoint"
    );
    Some(model)
  }

  /// Every currently subscribed endpoint model name, in sorted order, for
  /// the `/v1/models` listing.
  pub fn endpoint_models(&self) -> Vec<String> {
    let inner = lock(&self.inner);
    inner.endpoint_by_model.keys().cloned().collect()
  }

  /// Look up which agent owns an endpoint model, without delivering anything.
  /// Returns the `(agent, uuid)` pair the server needs to open a session
  /// before routing the request into the inbox.
  pub fn endpoint_lookup(&self, model: &str) -> Option<(String, String)> {
    let inner = lock(&self.inner);
    inner.endpoint_by_model.get(model).cloned()
  }

  /// Route an inbound endpoint request for `model` into the owning agent's
  /// inbox, tagged with that agent's subscription UUID. The message only
  /// lands if `model` is currently subscribed. Returns the `(agent, uuid)`
  /// pair so the caller can tag the matching session.
  pub fn endpoint_route(
    &self,
    model: &str,
    session: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> Result<(String, String), String> {
    let mut inner = lock(&self.inner);
    let Some((agent, uuid)) = inner.endpoint_by_model.get(model).cloned()
    else {
      tracing::debug!(endpoint_model = %model, "endpoint route miss");
      return Err(format!("no such subscribed endpoint model {model:?}"));
    };
    let envelope = EventEnvelope {
      id: uuid.clone(),
      event: Event::EndpointMessage(EndpointMessage {
        session: session.to_string(),
        messages,
        tools,
      }),
    };
    let (tx, _) = self.channels_locked(&mut inner, &agent);
    if tx.send(envelope).is_err() {
      tracing::warn!(agent,endpoint_model = %model, "failed to deliver an endpoint message to an inbox");
    }
    tracing::debug!(
      agent,
      endpoint_model = %model,
      uuid = %uuid,
      "endpoint message routed"
    );
    Ok((agent, uuid))
  }

  /// Deliver `payload` from `caller` to `dest`'s inbox, tagged with the UUID of
  /// `dest`'s subscription to `caller`. If `dest` never subscribed to `caller`,
  /// nothing is delivered.
  pub fn send(&self, caller: &str, dest: &str, payload: String) {
    let mut inner = lock(&self.inner);
    let delivered = if let Some(uuid) = inner
      .subscriptions
      .get(&(dest.to_string(), caller.to_string()))
    {
      let envelope = EventEnvelope {
        id: uuid.clone(),
        event: Event::Message(payload),
      };
      let (tx, _) = self.channels_locked(&mut inner, dest);
      if tx.send(envelope).is_err() {
        tracing::warn!(caller, dest, "failed to deliver a message to an inbox");
      }
      true
    } else {
      false
    };
    tracing::debug!(caller, dest, delivered, "agent sent a message");
  }

  /// Deliver an `event` tagged with `uuid` directly into `name`'s inbox,
  /// bypassing the subscription registry. Used by I/O pumps (timers, chat
  /// streams, resource notifications) that hold their own handle.
  pub fn deliver(&self, name: &str, uuid: &str, event: Event) {
    let mut inner = lock(&self.inner);
    let envelope = EventEnvelope {
      id: uuid.to_string(),
      event,
    };
    let (tx, _) = self.channels_locked(&mut inner, name);
    if tx.send(envelope).is_err() {
      tracing::warn!(name, uuid, "failed to deliver an event to an inbox");
    }
  }

  /// Blocking receive (with a timeout) of the next event from `name`'s
  /// inbox. Takes an optional shutdown predicate (checked roughly every
  /// slice, see `tunables.recv_slice_ms`) that aborts the wait early — for
  /// example a hot-reload request. Slices only return what is already queued,
  /// so pending events stay in the inbox for whoever runs next.
  pub fn recv(
    &self,
    name: &str,
    timeout: Duration,
  ) -> anyhow::Result<EventEnvelope> {
    self.recv_while(name, timeout, || false)
  }

  /// [`recv`](Self::recv) with an early-abort predicate: when `should_stop`
  /// returns true the wait aborts with a `"stop requested"` error, leaving
  /// queued events untouched.
  pub fn recv_while(
    &self,
    name: &str,
    timeout: Duration,
    should_stop: impl Fn() -> bool,
  ) -> anyhow::Result<EventEnvelope> {
    let (_, rx) = self.channels(name);
    let start = std::time::Instant::now();
    let slice = self.tunables.recv_slice();
    loop {
      if should_stop() {
        return Err(anyhow::anyhow!("stop requested"));
      }
      let elapsed = start.elapsed();
      if elapsed >= timeout {
        tracing::debug!(name, "inbox recv timed out");
        return Err(anyhow::anyhow!("no event available"));
      }
      let remaining = timeout.checked_sub(elapsed).unwrap_or(slice);
      match rx.recv_timeout(remaining.min(slice)) {
        Ok(envelope) => {
          tracing::trace!(name, "received an event from the inbox");
          return Ok(envelope);
        }
        // Empty, closed, or a timeout all read as "no event right now".
        Err(_) => {
          if should_stop() {
            return Err(anyhow::anyhow!("stop requested"));
          }
        }
      }
    }
  }

  /// Non-blocking poll of the next event from `name`'s inbox.
  pub fn try_recv(&self, name: &str) -> anyhow::Result<Option<EventEnvelope>> {
    let (_, rx) = self.channels(name);
    match rx.try_recv() {
      Ok(Some(envelope)) => {
        tracing::trace!(name, "polled an event from the inbox");
        Ok(Some(envelope))
      }
      // Empty, closed, or disconnected all read as "nothing right now".
      Ok(None) | Err(_) => Ok(None),
    }
  }

  /// Drop queued `Reload`/`Shutdown` system events from `name`'s inbox,
  /// requeueing anything else in order. The flag-abort in `recv_while`
  /// leaves the delivered system event queued, so without this the next
  /// iteration's first `recv` would spuriously exit. Ordering across the
  /// drain is best-effort: concurrent pumps may deliver while draining.
  /// Stale lifecycle `error`s (delivered on the previous run's lifecycle
  /// UUID) are dropped as well, so a restarted brain never observes a
  /// reload failure from before its run.
  pub fn drain_system(&self, name: &str) -> anyhow::Result<()> {
    let lifecycle = self.lifecycle_of(name);
    let mut kept: Vec<EventEnvelope> = Vec::new();
    while let Some(envelope) = self.try_recv(name)? {
      match &envelope.event {
        Event::Reload | Event::Shutdown => {}
        Event::Error(_) => {
          if Some(envelope.id.as_str()) == lifecycle.as_deref() {
            continue;
          }
          kept.push(envelope);
        }
        _ => kept.push(envelope),
      }
    }
    // Lifecycle handles are one-shot per run; the next iteration must
    // re-subscribe. Clear after capturing the old UUID for stale-error
    // filtering above.
    {
      let mut inner = lock(&self.inner);
      if let Some(uuid) = inner.lifecycle.remove(name) {
        inner.lifecycle_by_uuid.remove(&uuid);
      }
    }
    if kept.is_empty() {
      return Ok(());
    }
    let mut inner = lock(&self.inner);
    let (tx, _) = self.channels_locked(&mut inner, name);
    for envelope in kept {
      if tx.send(envelope).is_err() {
        tracing::warn!(name, "failed to requeue an event after a drain");
        break;
      }
    }
    Ok(())
  }

  /// Get (creating if missing) the channel pair for `name`.
  fn channels(&self, name: &str) -> AgentChannels {
    let mut inner = lock(&self.inner);
    self.channels_locked(&mut inner, name)
  }

  /// Get (creating if missing) the channel pair for `name`, holding the lock.
  fn channels_locked(&self, inner: &mut BusInner, name: &str) -> AgentChannels {
    let bound = self.tunables.inbox_bound;
    inner
      .inboxes
      .entry(name.to_string())
      .or_insert_with(|| kanal::bounded(bound))
      .clone()
  }
}

/// A fresh v4 UUID string, used for every subscription/handle.
pub fn new_uuid() -> String {
  uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn subscribe_then_send_delivers_a_tagged_message() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let sub = bus.subscribe("alice", "bob");
    bus.send("bob", "alice", "hello".to_string());
    let envelope = bus
      .try_recv("alice")?
      .unwrap_or_else(|| panic!("expected a delivered envelope"));
    assert_eq!(envelope.id, sub);
    assert_eq!(envelope.event, Event::Message("hello".to_string()));
    Ok(())
  }

  #[test]
  fn non_subscribed_sender_delivers_nothing() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    bus.subscribe("alice", "bob");
    bus.send("charlie", "alice", "hello".to_string());
    assert_eq!(bus.try_recv("alice")?, None);
    Ok(())
  }

  #[test]
  fn send_fans_out_to_multiple_subscribers() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let sub1 = bus.subscribe("alice", "bob");
    let sub2 = bus.subscribe("carol", "bob");
    bus.send("bob", "alice", "x".to_string());
    bus.send("bob", "carol", "y".to_string());

    let one = bus
      .try_recv("alice")?
      .unwrap_or_else(|| panic!("expected a delivered envelope"));
    let two = bus
      .try_recv("carol")?
      .unwrap_or_else(|| panic!("expected a delivered envelope"));
    assert_eq!(one.id, sub1);
    assert_eq!(two.id, sub2);
    assert_eq!(one.event, Event::Message("x".to_string()));
    assert_eq!(two.event, Event::Message("y".to_string()));
    Ok(())
  }

  #[test]
  fn unsubscribe_stops_deliveries() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let sub = bus.subscribe("alice", "bob");
    assert!(bus.unsubscribe("alice", &sub));
    bus.send("bob", "alice", "hello".to_string());
    assert_eq!(bus.try_recv("alice")?, None);
    Ok(())
  }

  #[test]
  fn unsubscribe_rejects_foreign_handles() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let sub = bus.subscribe("alice", "bob");
    assert!(!bus.unsubscribe("carol", &sub));
    bus.send("bob", "alice", "hello".to_string());
    let envelope = bus
      .try_recv("alice")?
      .unwrap_or_else(|| panic!("expected a delivered envelope"));
    assert_eq!(envelope.id, sub);
    Ok(())
  }

  #[test]
  fn recv_respects_timeout() {
    let bus = MessageBus::new();
    assert!(
      bus.recv("ghost", Duration::from_secs(1)).is_err(),
      "recv on an empty inbox should time out"
    );
  }

  #[test]
  fn recv_while_aborts_early_on_stop_leaving_queued_events()
  -> anyhow::Result<()> {
    let bus = MessageBus::new();
    bus.send("nobody", "alice", "queued".to_string());
    // No subscription, so nothing is queued yet; stop immediately instead.
    let error = bus
      .recv_while("alice", Duration::from_secs(60), || true)
      .unwrap_err();
    assert_eq!(error.to_string(), "stop requested");

    let sub = bus.subscribe("alice", "bob");
    bus.send("bob", "alice", "hello".to_string());
    let error = bus
      .recv_while("alice", Duration::from_secs(60), || true)
      .unwrap_err();
    assert_eq!(error.to_string(), "stop requested");
    // The abort must not drain the inbox: the event is still there.
    let envelope = bus
      .try_recv("alice")?
      .ok_or_else(|| anyhow::anyhow!("expected the queued event"))?;
    assert_eq!(envelope.id, sub);
    assert_eq!(envelope.event, Event::Message("hello".to_string()));
    Ok(())
  }

  #[test]
  fn drain_system_drops_reload_and_shutdown_keeping_order() -> anyhow::Result<()>
  {
    let bus = MessageBus::new();
    let first = bus.subscribe("alice", "bob");
    bus.send("bob", "alice", "one".to_string());
    bus.deliver("alice", &new_uuid(), Event::Reload);
    bus.send("bob", "alice", "two".to_string());
    bus.deliver("alice", &new_uuid(), Event::Shutdown);
    bus.send("bob", "alice", "three".to_string());
    bus.drain_system("alice")?;
    for expected in ["one", "two", "three"] {
      let envelope = bus
        .try_recv("alice")?
        .ok_or_else(|| anyhow::anyhow!("expected {expected:?}"))?;
      assert_eq!(envelope.id, first);
      assert_eq!(envelope.event, Event::Message(expected.to_string()));
    }
    assert_eq!(bus.try_recv("alice")?, None);
    Ok(())
  }

  #[test]
  fn lifecycle_subscribe_returns_a_handle_and_rejects_a_second()
  -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let uuid = bus
      .lifecycle_subscribe("alice")
      .map_err(|e| anyhow::anyhow!(e))?;
    assert_eq!(bus.lifecycle_of("alice"), Some(uuid.clone()));
    assert!(bus.lifecycle_subscribe("alice").is_err());
    assert!(bus.lifecycle_unsubscribe("alice", &uuid));
    assert_eq!(bus.lifecycle_of("alice"), None);
    Ok(())
  }

  #[test]
  fn lifecycle_unsubscribe_rejects_foreign_handles() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let uuid = bus
      .lifecycle_subscribe("alice")
      .map_err(|e| anyhow::anyhow!(e))?;
    assert!(!bus.lifecycle_unsubscribe("carol", &uuid));
    assert_eq!(bus.lifecycle_of("alice"), Some(uuid));
    Ok(())
  }

  #[test]
  fn drain_system_drops_lifecycle_errors() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let lifecycle = bus
      .lifecycle_subscribe("alice")
      .map_err(|e| anyhow::anyhow!(e))?;
    bus.deliver("alice", &lifecycle, Event::Error("bad edit".to_string()));
    bus.deliver("alice", &new_uuid(), Event::Error("other".to_string()));
    bus.drain_system("alice")?;
    let envelope = bus
      .try_recv("alice")?
      .ok_or_else(|| anyhow::anyhow!("expected the non-lifecycle error"))?;
    assert_eq!(envelope.event, Event::Error("other".to_string()));
    assert_eq!(bus.try_recv("alice")?, None);
    assert_eq!(bus.lifecycle_of("alice"), None);
    Ok(())
  }

  #[test]
  fn new_uuid_is_a_valid_v4() -> anyhow::Result<()> {
    let uuid = new_uuid();
    let parsed = uuid::Uuid::parse_str(&uuid)
      .map_err(|_| anyhow::anyhow!("not a valid uuid: {uuid:?}"))?;
    assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
    Ok(())
  }

  #[test]
  fn endpoint_subscribe_then_route_delivers_tagged_endpoint_message()
  -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let (agent, uuid) = bus
      .endpoint_route(
        "gpt-4o",
        "session-1",
        vec![ChatMessage {
          role: crate::provider::Role::User,
          content: Some("hi".to_string()),
          tool_call: None,
        }],
        Vec::new(),
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    assert_eq!(agent, "alice");
    assert_eq!(uuid, sub);
    let envelope = bus.try_recv("alice")?.ok_or_else(|| {
      anyhow::anyhow!("expected a delivered endpoint message")
    })?;
    assert_eq!(envelope.id, sub);
    match envelope.event {
      Event::EndpointMessage(mgs) => {
        assert_eq!(mgs.session, "session-1");
        assert_eq!(mgs.messages.len(), 1);
        assert!(mgs.tools.is_empty());
      }
      other => return Err(anyhow::anyhow!("unexpected event: {other:?}")),
    }
    Ok(())
  }

  #[test]
  fn endpoint_subscribe_rejects_duplicate_models() {
    let bus = MessageBus::new();
    bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .unwrap();
    let err = bus
      .endpoint_subscribe("bob", "gpt-4o".to_string())
      .unwrap_err();
    assert!(err.contains("already subscribed"));
  }

  #[test]
  fn endpoint_unsubscribe_removes_model_and_route_misses() -> anyhow::Result<()>
  {
    let bus = MessageBus::new();
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let model = bus
      .endpoint_unsubscribe("alice", &sub)
      .ok_or_else(|| anyhow::anyhow!("expected a model back"))?;
    assert_eq!(model, "gpt-4o");
    assert!(bus.endpoint_models().is_empty());
    assert!(bus.endpoint_lookup("gpt-4o").is_none());
    Ok(())
  }

  #[test]
  fn endpoint_unsubscribe_rejects_foreign_handles() -> anyhow::Result<()> {
    let bus = MessageBus::new();
    let sub = bus
      .endpoint_subscribe("alice", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    assert!(bus.endpoint_unsubscribe("carol", &sub).is_none());
    assert_eq!(
      bus.endpoint_lookup("gpt-4o").map(|p| p.0),
      Some("alice".to_string())
    );
    Ok(())
  }

  #[test]
  fn endpoint_models_lists_models_in_sorted_order() {
    let bus = MessageBus::new();
    bus.endpoint_subscribe("alice", "zeta".to_string()).unwrap();
    bus.endpoint_subscribe("bob", "alpha".to_string()).unwrap();
    assert_eq!(bus.endpoint_models(), vec!["alpha", "zeta"]);
  }
}
