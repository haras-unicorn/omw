//! Host-side chat-stream pumps. Each `provider.chat` call opens a stream
//! registered by UUID in a [`StreamRegistry`]; a pump task on the bridge
//! runtime reads the provider's stream and delivers `delta` events into the
//! requesting agent's inbox, then a terminal `stream-end` (or an `error` on
//! failure) event closes it. `is-open`/`cancel` go through the registry, which
//! doubles as the cancel signal: dropping an entry's sender wakes its pump's
//! cancel receiver.

#![allow(
  clippy::empty_line_after_doc_comments,
  reason = "blank lines between doc comments and items are normalized by dev format"
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use futures_util::StreamExt as _;
use tokio::sync::oneshot;

use crate::host::bus::MessageBus;
use crate::host::events::Event;
use crate::provider::{ChatMessage, Provider};
use crate::tooling::Tool;

/// Registry of open chat streams, keyed by UUID. Each entry holds the cancel
/// signal for its pump; an entry's presence means the stream is still open.

#[derive(Default)]
pub struct StreamRegistry {
  open: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl StreamRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  /// Lock the open-stream map, recovering the guard on poison.
  fn locked_open(
    &self,
  ) -> MutexGuard<'_, HashMap<String, oneshot::Sender<()>>> {
    self
      .open
      .lock()
      .unwrap_or_else(|poison| poison.into_inner())
  }

  /// Register a stream and return the cancel receiver its pump waits on.
  pub fn open(&self, uuid: String) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    let mut m = self.locked_open();
    m.insert(uuid, tx);
    rx
  }

  /// Whether a chat stream is still open.

  pub fn is_open(&self, uuid: &str) -> bool {
    self.locked_open().contains_key(uuid)
  }

  /// Cancel an open stream by UUID: drops its cancel signal, waking the pump.

  pub fn cancel(&self, uuid: &str) {
    let _ = self.locked_open().remove(uuid);
  }

  /// Deregister a stream;the pump calls this once it finishes.

  pub fn remove(&self, uuid: &str) {
    let _ = self.locked_open().remove(uuid);
  }
}

#[allow(
  clippy::too_many_arguments,
  reason = "aggregating the bridge handles into a struct is left to a streams refactor"
)]
/// Spawn a pump task on `rt` that drains `provider.chat(...)` and delivers its
/// deltas into `name`'s inbox tagged with `uuid`. The pump runs to a terminal
/// `stream-end` event on natural end (or an `error` event on failure)and then
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
) {
  let mut cancel = streams.open(uuid.clone());
  tracing::info!(agent = %name, uuid = %uuid, model = %model, "chat stream opened");
  rt.spawn(async move {
    let agent = name.clone();
    let mut stream = match provider.chat(&model, messages, tools).await {
      Ok(stream) => stream,
      Err(e) => {
        tracing::error!(agent, uuid = %uuid, error = %e, "chat stream pump failed to open");
        bus.deliver(&name, &uuid, Event::Error(e.to_string()));
        streams.remove(&uuid);
        return;
      }
    };
    loop {
      tokio::select! {
        biased;

        _ = &mut cancel => {
          tracing::debug!(agent, uuid = %uuid, "chat stream pump cancelled");
          break;
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
            bus.deliver(&name,&uuid, Event::Delta(delta))
          }
          Some(Err(e)) => {
            tracing::error!(agent, uuid = %uuid, error = %e, "chat stream pump failed");
            bus.deliver(&name,&uuid, Event::Error(e.to_string()));
            break;
          }
          None => {
            tracing::debug!(agent, uuid = %uuid, "chat stream ended");
            bus.deliver(&name,&uuid, Event::StreamEnd);
            break;
          }
        },
      }
    }
    streams.remove(&uuid);
  });
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;
  use crate::host::bus::MessageBus;

  #[test]
  fn open_is_open_and_cancel_lifecycle() {
    let streams = StreamRegistry::new();
    let mut rx = streams.open("s".to_string());
    assert!(streams.is_open("s"));
    streams.cancel("s");
    assert!(!streams.is_open("s"));
    assert!(rx.try_recv().is_err());
  }

  #[test]
  fn pump_delivers_deltas_then_stream_end() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let streams = Arc::new(StreamRegistry::new());
    let entry = crate::provider::build(
      "mock",
      "mock",
      &serde_json::json!({ "responses": ["Hello", ", world"] }),
    )?;
    let uuid = crate::host::bus::new_uuid();
    spawn_pump(
      entry.provider,
      Arc::clone(&rt),
      Arc::clone(&bus),
      Arc::clone(&streams),
      "alice".to_string(),
      uuid.clone(),
      "mock-model".to_string(),
      Vec::new(),
      Vec::new(),
    );
    let first = bus.recv("alice", Duration::from_secs(5))?;
    match first.event {
      Event::Delta(d) => assert_eq!(d.content.as_deref(), Some("Hello")),
      other => anyhow::bail!("expected a delta, got {other:?}"),
    }
    let second = bus.recv("alice", Duration::from_secs(5))?;
    match second.event {
      Event::Delta(d) => assert_eq!(d.content.as_deref(), Some(", world")),
      other => anyhow::bail!("expected a delta, got {other:?}"),
    }
    let end = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(end.event, Event::StreamEnd);
    rt.block_on(async {
      tokio::time::sleep(Duration::from_millis(50)).await;
    });
    assert!(!streams.is_open(&uuid));
    Ok(())
  }
}
