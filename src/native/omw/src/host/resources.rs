//! Host-side resource-subscription pumps. Each `tooling.subscribe-*` call
//! (list or single) yields a [`ResourceNotification`] stream; a pump task on
//! the bridge runtime drains it and delivers `resource-changed` /
//! `resource-updated` events into the requesting agent's inbox, tagged with the
//! subscription's UUID.

use std::sync::Arc;

use futures_util::stream::{BoxStream, StreamExt as _};

use crate::host::bus::MessageBus;
use crate::host::events::Event;
use crate::tooling::ResourceNotification;

/// Spawn a pump task on `rt` that drains `stream` and delivers resource
/// notifications into `name`'s inbox tagged with `uuid`; the pump exits when
/// the stream ends or an error arrives.
pub fn spawn_pump(
  rt: Arc<tokio::runtime::Runtime>,
  bus: Arc<MessageBus>,
  name: String,
  uuid: String,
  mut stream: BoxStream<'static, Result<ResourceNotification, String>>,
) {
  rt.spawn(async move {
    tracing::info!(agent = %name, uuid = %uuid, "resource subscription opened");
    while let Some(item) = stream.next().await {
      match item {
        Ok(ResourceNotification::ListChanged) => {
          tracing::trace!(agent = %name, uuid = %uuid, "resource list changed");
          bus.deliver(&name, &uuid, Event::ResourceChanged);
        }
        Ok(ResourceNotification::Updated { .. }) => {
          tracing::trace!(agent = %name, uuid = %uuid, "resource updated");
          bus.deliver(&name, &uuid, Event::ResourceUpdated);
        }
        Err(e) => {
          tracing::error!(agent = %name, uuid = %uuid, error = %e, "resource subscription failed");
          bus.deliver(&name, &uuid, Event::Error(e.to_string()));
          return;
        }
      }
    }
    tracing::debug!(agent = %name, uuid = %uuid, "resource subscription ended");
  });
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;
  use crate::host::bus::MessageBus;
  use crate::tooling::Tooling;
  use crate::tooling::mock::MockTooling;

  #[test]
  fn pump_delivers_resource_updated_tagged_with_uuid() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let tooling = MockTooling::noop();
    let uuid = crate::host::bus::new_uuid();

    let stream = rt.block_on(tooling.subscribe_resource("file:///a"))?;
    spawn_pump(
      Arc::clone(&rt),
      Arc::clone(&bus),
      "alice".to_string(),
      uuid.clone(),
      stream,
    );

    let envelope = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(envelope.id, uuid);
    assert_eq!(envelope.event, Event::ResourceUpdated);
    Ok(())
  }
}
