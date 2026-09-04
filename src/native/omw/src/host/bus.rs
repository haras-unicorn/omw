//! Inter-agent inboxes and the subscription registry that fans messages out to
//! them. Messages only reach an agent if that agent subscribed to the sender;
//! every delivery carries the subscription's UUID so the guest can disambiguate
//! events from different sources on the single inbox.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use kanal::{Receiver, Sender};

use crate::host::events::{Event, EventEnvelope};

/// Lock a `std::sync::Mutex`, recovering the guard on poison (a poisoned lock
/// should never take the whole runtime down).
fn lock<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
  mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// A per-agent channel pair: sender + receiver.
type AgentChannels = (Sender<EventEnvelope>, Receiver<EventEnvelope>);

/// A shared bus of per-agent inboxes plus the subscription registry.
#[derive(Debug, Default)]
pub struct MessageBus {
  inner: Mutex<BusInner>,
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
}

impl MessageBus {
  pub fn new() -> Self {
    Self::default()
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

  /// Blocking receive (with a timeout) of the next event from `name`'s inbox.
  pub fn recv(
    &self,
    name: &str,
    timeout: Duration,
  ) -> anyhow::Result<EventEnvelope> {
    let (_, rx) = self.channels(name);
    match rx.recv_timeout(timeout) {
      Ok(envelope) => {
        tracing::trace!(name, "received an event from the inbox");
        Ok(envelope)
      }
      // Empty, closed, or a timeout all read as "no event right now".
      Err(_) => {
        tracing::debug!(name, "inbox recv timed out");
        Err(anyhow::anyhow!("no event available"))
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

  /// Get (creating if missing) the channel pair for `name`.
  fn channels(&self, name: &str) -> AgentChannels {
    let mut inner = lock(&self.inner);
    self.channels_locked(&mut inner, name)
  }

  /// Get (creating if missing) the channel pair for `name`, holding the lock.
  fn channels_locked(&self, inner: &mut BusInner, name: &str) -> AgentChannels {
    inner
      .inboxes
      .entry(name.to_string())
      .or_insert_with(|| kanal::bounded(1024))
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
      bus.recv("ghost", Duration::from_millis(20)).is_err(),
      "recv on an empty inbox should time out"
    );
  }

  #[test]
  fn new_uuid_is_a_valid_v4() -> anyhow::Result<()> {
    let uuid = new_uuid();
    let parsed = uuid::Uuid::parse_str(&uuid)
      .map_err(|_| anyhow::anyhow!("not a valid uuid: {uuid:?}"))?;
    assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
    Ok(())
  }
}
