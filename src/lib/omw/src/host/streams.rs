//! Host-side chat-stream pumps. Each `provider.chat-stream` call opens a stream
//! registered by UUID in a [`StreamRegistry`]; a pump task on the bridge
//! runtime reads the provider's stream and delivers `chat-delta` events into the
//! requesting agent's inbox, then a terminal `chat-end` (or an `error` on
//! failure) event closes it. `is-open`/`cancel` go through the registry, which
//! doubles as the cancel signal: dropping an entry's sender wakes its pump's
//! cancel receiver.

#![allow(
  clippy::empty_line_after_doc_comments,
  reason = "blank lines between doc comments and items are normalized by dev format"
)]

use std::sync::Arc;

use dashmap::DashMap;
use futures_util::StreamExt as _;
use tokio::sync::oneshot;

use crate::host::bus::MessageBus;
use crate::host::events::Event;
use crate::host::trace::{CloseReason, SourceKind, TraceEvent, TraceSender};
use crate::provider::{ChatMessage, Provider};
use crate::tooling::Tool;

/// One open cancellable source. Besides the cancel signal, it remembers how to
/// emit the `closed` lifecycle observation so a cancellation can be made
/// synchronous with the guest call that requested it (see [`StreamRegistry::cancel`]).
#[derive(Debug)]
struct Entry {
  /// Held for its `Drop`: dropping the sender wakes the pump's cancel
  /// receiver. Never read.
  #[allow(
    dead_code,
    reason = "the sender is held only so that dropping it wakes the pump"
  )]
  cancel: oneshot::Sender<()>,
  agent: String,
  source: SourceKind,
  trace: Option<TraceSender>,
}

impl Entry {
  /// Emit the `closed` lifecycle observation for a cancelled entry, if a trace
  /// is attached. Called synchronously from `cancel` / `cancel_all` so the
  /// close is pinned to the operation rather than to a racing pump task.
  fn trace_closed(&self, uuid: &str, reason: CloseReason) {
    if let Some(tx) = &self.trace {
      let _ = tx.send(TraceEvent::Closed {
        agent: self.agent.clone(),
        source: self.source,
        uuid: uuid.to_string(),
        reason,
      });
    }
  }
}

/// Registry of open chat streams, keyed by UUID. Each entry holds the cancel
/// signal for its pump; an entry's presence means the stream is still open.
#[derive(Default)]
pub struct StreamRegistry {
  open: DashMap<String, Entry>,
}

/// The generic cancel-signal registry behind every cancellable host source (chat
/// streams, timers, resource subscriptions). An entry doubles as the cancel
/// signal, and dropping its sender wakes its pump's cancel receiver.
pub type CancelRegistry = StreamRegistry;

impl StreamRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  /// Register a stream and return the cancel receiver its pump waits on. The
  /// entry captures `bus`'s trace tap (if any) so a later `cancel` can emit the
  /// `closed` observation synchronously.
  pub fn open(
    &self,
    bus: &MessageBus,
    uuid: String,
    agent: String,
    source: SourceKind,
  ) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    self.open.insert(
      uuid,
      Entry {
        cancel: tx,
        agent,
        source,
        trace: bus.trace_sender(),
      },
    );
    rx
  }

  /// Whether a chat stream is still open.
  pub fn is_open(&self, uuid: &str) -> bool {
    self.open.contains_key(uuid)
  }

  /// Cancel an open stream by UUID: emits its `closed{cancelled}` observation
  /// synchronously, then drops its cancel signal to wake the pump. The pump
  /// skips its own `closed` emission for a cancellation, so this fires once.
  pub fn cancel(&self, uuid: &str) {
    if let Some((_, entry)) = self.open.remove(uuid) {
      entry.trace_closed(uuid, CloseReason::Cancelled);
    }
  }

  /// Cancel every open stream/timer/subscription at once, waking all pumps.
  /// Used on hot reload so a restarted agent leaves no stale pumps behind. Each
  /// entry's `closed{cancelled}` observation is emitted synchronously.
  pub fn cancel_all(&self) {
    let keys: Vec<String> =
      self.open.iter().map(|entry| entry.key().clone()).collect();
    for uuid in keys {
      if let Some((_, entry)) = self.open.remove(&uuid) {
        entry.trace_closed(&uuid, CloseReason::Cancelled);
      }
    }
  }

  /// Deregister a stream; the pump calls this once it finishes. Silent: a
  /// natural end emits its own `closed` from the pump.
  pub fn remove(&self, uuid: &str) {
    let _ = self.open.remove(uuid);
  }

  /// Test-only poll helper: block up to `timeout` until `uuid` appears
  /// (when `present` is true) or disappears. Returns the final state.
  /// Replaces fixed `sleep(50ms)` + assert shapes that flake on slow CI.
  #[cfg(test)]
  pub fn wait_for(
    &self,
    uuid: &str,
    present: bool,
    timeout: std::time::Duration,
  ) -> bool {
    let start = std::time::Instant::now();
    loop {
      if self.is_open(uuid) == present {
        return true;
      }
      if start.elapsed() >= timeout {
        return self.is_open(uuid) == present;
      }
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
  }
}

#[allow(
  clippy::too_many_arguments,
  reason = "aggregating the bridge handles into a struct is left to a streams refactor"
)]
/// Spawn a pump task on `rt` that drains `provider.chat_stream(...)` and delivers its
/// deltas into `name`'s inbox tagged with `uuid`. The pump runs to a terminal
/// `chat-end` event on natural end (or an `error` event on failure) and then
/// deregisters its stream.
pub fn spawn_pump(
  provider: Arc<dyn Provider>,
  rt: Arc<tokio::runtime::Runtime>,
  bus: Arc<MessageBus>,
  streams: Arc<StreamRegistry>,
  name: String,
  uuid: String,
  model: String,
  messages: Vec<ChatMessage>,
  tools: Vec<Tool>,
  params: Option<serde_json::Value>,
) {
  let mut cancel =
    streams.open(&bus, uuid.clone(), name.clone(), SourceKind::ChatStream);
  tracing::info!(agent = %name, uuid = %uuid, model = %model, "chat stream opened");
  bus.trace_event(TraceEvent::Opened {
    agent: name.clone(),
    source: SourceKind::ChatStream,
    uuid: uuid.clone(),
  });
  rt.spawn(async move {
    let agent = name.clone();
    let mut stream =
      match provider.chat_stream(&model, messages, tools, params).await {
      Ok(stream) => stream,
      Err(e) => {
        tracing::error!(agent, uuid = %uuid, error = %e, "chat stream pump failed to open");
        bus.deliver(&name, &uuid, Event::Error(e.to_string()));
        streams.remove(&uuid);
        bus.trace_event(TraceEvent::Closed {
          agent: name.clone(),
          source: SourceKind::ChatStream,
          uuid: uuid.clone(),
          reason: CloseReason::Failed,
        });
        return;
      }
    };
    let reason = loop {
      tokio::select! {
        biased;

        _ = &mut cancel => {
          tracing::debug!(agent, uuid = %uuid, "chat stream pump cancelled");
          break CloseReason::Cancelled;
        }

        next = stream.next() => match next {
          Some(Ok(delta)) => {
            tracing::trace!(
              agent,
              uuid = %uuid,
              content_len = delta.content.as_ref().map_or(0, String::len),
              tool_call = delta.tool_call.as_ref().map(|t| t.name.as_str()),
              finish_reason = delta.finish_reason.as_deref(),
              "chat delta delivered"
            );
            bus.deliver(&name,&uuid, Event::ChatDelta(delta))
          }
          Some(Err(e)) => {
            tracing::error!(agent, uuid = %uuid, error = %e, "chat stream pump failed");
            bus.deliver(&name,&uuid, Event::Error(e.to_string()));
            break CloseReason::Failed;
          }
          None => {
            tracing::debug!(agent, uuid = %uuid, "chat stream ended");
            bus.deliver(&name,&uuid, Event::ChatEnd);
            break CloseReason::Ended;
          }
        },
      }
    };
    streams.remove(&uuid);
    if reason != CloseReason::Cancelled {
      bus.trace_event(TraceEvent::Closed {
        agent: name.clone(),
        source: SourceKind::ChatStream,
        uuid: uuid.clone(),
        reason,
      });
    }
  });
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;
  use crate::host::bus::MessageBus;

  #[test]
  fn open_is_open_and_cancel_lifecycle() {
    let bus = MessageBus::new();
    let streams = StreamRegistry::new();
    let mut rx =
      streams.open(&bus, "s".to_string(), "a".to_string(), SourceKind::Timer);
    assert!(streams.is_open("s"));
    streams.cancel("s");
    assert!(!streams.is_open("s"));
    assert!(rx.try_recv().is_err());
  }

  #[test]
  fn cancel_all_wakes_every_pump() {
    let bus = MessageBus::new();
    let streams = StreamRegistry::new();
    let mut first =
      streams.open(&bus, "a".to_string(), "a".to_string(), SourceKind::Timer);
    let mut second =
      streams.open(&bus, "b".to_string(), "a".to_string(), SourceKind::Timer);
    assert!(streams.is_open("a"));
    assert!(streams.is_open("b"));
    streams.cancel_all();
    assert!(!streams.is_open("a"));
    assert!(!streams.is_open("b"));
    assert!(first.try_recv().is_err());
    assert!(second.try_recv().is_err());
  }

  #[test]
  fn cancel_emits_closed_synchronously_in_cancel_order() -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let bus = MessageBus::with_trace(crate::config::Tunables::default(), tx);
    let streams = StreamRegistry::new();
    let _a = streams.open(
      &bus,
      "a".to_string(),
      "alice".to_string(),
      SourceKind::ResourceList,
    );
    let _b = streams.open(
      &bus,
      "b".to_string(),
      "alice".to_string(),
      SourceKind::Resource,
    );
    // Both closes are emitted inside their `cancel` call, in call order — not
    // from the pump tasks they wake, which the scheduler could reorder.
    streams.cancel("a");
    streams.cancel("b");
    let first = rx.try_recv().map_err(|e| anyhow::anyhow!("{e}"))?;
    let second = rx.try_recv().map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(
      first,
      TraceEvent::Closed {
        agent: "alice".to_string(),
        source: SourceKind::ResourceList,
        uuid: "a".to_string(),
        reason: CloseReason::Cancelled,
      }
    );
    assert_eq!(
      second,
      TraceEvent::Closed {
        agent: "alice".to_string(),
        source: SourceKind::Resource,
        uuid: "b".to_string(),
        reason: CloseReason::Cancelled,
      }
    );
    Ok(())
  }

  #[test]
  fn pump_delivers_deltas_then_chat_end() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let streams = Arc::new(StreamRegistry::new());
    let entry = crate::provider::Registry::default().build(
      "mock",
      "mock",
      &serde_json::json!({ "turns": [{ "content": "Hello, world" }] }),
    )?;
    let uuid = crate::host::bus::new_uuid();
    spawn_pump(
      Arc::clone(entry.inner()),
      Arc::clone(&rt),
      Arc::clone(&bus),
      Arc::clone(&streams),
      "alice".to_string(),
      uuid.clone(),
      "mock-model".to_string(),
      Vec::new(),
      Vec::new(),
      None,
    );
    let first = bus.recv("alice", Duration::from_secs(5))?;
    match first.event {
      Event::ChatDelta(d) => {
        assert_eq!(d.content.as_deref(), Some("Hello, world"));
      }
      other => anyhow::bail!("expected a delta, got {other:?}"),
    }
    let end = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(end.event, Event::ChatEnd);
    assert!(
      streams.wait_for(&uuid, false, Duration::from_secs(5)),
      "stream should deregister after ChatEnd"
    );
    Ok(())
  }

  #[test]
  fn pump_emits_host_opened_and_closed_trace_events() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let bus = Arc::new(MessageBus::with_trace(
      crate::config::Tunables::default(),
      tx,
    ));
    let streams = Arc::new(StreamRegistry::new());
    let entry = crate::provider::Registry::default().build(
      "mock",
      "mock",
      &serde_json::json!({ "turns": [{ "content": "hi" }] }),
    )?;
    let uuid = crate::host::bus::new_uuid();
    spawn_pump(
      Arc::clone(entry.inner()),
      Arc::clone(&rt),
      Arc::clone(&bus),
      Arc::clone(&streams),
      "alice".to_string(),
      uuid.clone(),
      "mock-model".to_string(),
      Vec::new(),
      Vec::new(),
      None,
    );

    let mut opened = false;
    let mut closed = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && !closed {
      match rx.try_recv() {
        Ok(TraceEvent::Opened {
          source: SourceKind::ChatStream,
          ..
        }) => opened = true,
        Ok(TraceEvent::Closed {
          source: SourceKind::ChatStream,
          reason: CloseReason::Ended,
          ..
        }) => closed = true,
        Ok(_) => {}
        Err(_) => std::thread::sleep(Duration::from_millis(5)),
      }
    }
    assert!(opened, "the chat stream should trace a Opened");
    assert!(closed, "the chat stream should trace a Closed");
    Ok(())
  }
}
