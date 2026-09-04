//! Host-side resource-subscription pumps. Each `tooling.subscribe-*` call
//! (list or single) yields a [`ResourceNotification`] stream; a pump task on
//! the bridge runtime drains it and delivers `resource-list-updated` /
//! `resource-updated` events into the requesting agent's inbox, tagged with the
//! subscription's UUID. A `resource-list-updated` event carries the freshly
//! fetched resource list, obtained with `tooling.list_resources` on the bridge
//! runtime.

use std::sync::Arc;

use futures_util::stream::{BoxStream, StreamExt as _};

use crate::host::bus::MessageBus;
use crate::host::events::Event;
use crate::host::streams::CancelRegistry;
use crate::tooling::{ResourceNotification, Tooling};

/// Spawn a pump task on `rt` that drains `stream` and delivers resource
/// notifications into `name`'s inbox tagged with `uuid`; the pump exits when
/// the stream ends or an error arrives.
///
/// A `ListChanged` notification triggers a `tooling.list_resources` fetch on
/// the bridge runtime whose result becomes the `resource-list-updated` event
/// payload; a fetch failure is delivered as an [`Event::Error`] while the pump
/// keeps running.
pub fn spawn_pump(
  subs: Arc<CancelRegistry>,
  rt: Arc<tokio::runtime::Runtime>,
  bus: Arc<MessageBus>,
  name: String,
  uuid: String,
  tooling: Arc<dyn Tooling>,
  mut stream: BoxStream<'static, Result<ResourceNotification, String>>,
) {
  let mut cancel = subs.open(uuid.clone());
  rt.spawn(async move {
    tracing::info!(agent = %name, uuid = %uuid, "resource subscription opened");
    loop {
      tokio::select! {
        biased;

        _ =&mut cancel => {
          tracing::debug!(agent = %name, uuid = %uuid, "resource subscription cancelled");
          return;
        }
        item = stream.next() => match item {
          Some(Ok(ResourceNotification::ListChanged)) => {
            tracing::trace!(agent = %name, uuid = %uuid, "resource list changed");
            match tooling.list_resources().await {
              Ok(resources) => {
                bus.deliver(&name,&uuid, Event::ResourceListUpdated(resources));
              }
              Err(e) => {
                tracing::warn!(agent = %name, uuid = %uuid, error = %e, "failed to fetch the resource list");
                bus.deliver(&name,&uuid, Event::Error(e.to_string()));
              }
            }
          }
          Some(Ok(ResourceNotification::Updated { uri })) => {
            tracing::trace!(agent = %name, uuid = %uuid, uri = %uri, "resource updated");
            match tooling.read_resource(&uri).await {
              Ok(content) => {
                bus.deliver(&name, &uuid, Event::ResourceUpdated(content));
              }
              Err(e) => {
                tracing::warn!(agent = %name, uuid = %uuid, uri = %uri, error = %e, "failed to read the updated resource");
                bus.deliver(&name, &uuid, Event::Error(e.to_string()));
              }
            }
          }
          Some(Err(e)) => {
            tracing::error!(agent = %name, uuid = %uuid, error = %e, "resource subscription failed");
            bus.deliver(&name,&uuid, Event::Error(e.to_string()));
            subs.remove(&uuid);
            return;
          }
          None => {
            tracing::debug!(agent = %name, uuid = %uuid, "resource subscription ended");
            subs.remove(&uuid);
            return;
          }
        }
      }
    }
  });
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;
  use crate::host::bus::MessageBus;
  use crate::tooling::mock::MockTooling;
  use crate::tooling::{ResourceContent, ResourceInfo, Tooling};

  #[test]
  fn pump_delivers_resource_updated_with_content_tagged_with_uuid()
  -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let tooling: Arc<dyn Tooling> =
      MockTooling::with_resource_content("file:///a", "hello");
    let uuid = crate::host::bus::new_uuid();

    let stream = rt.block_on(tooling.subscribe_resource("file:///a"))?;
    spawn_pump(
      Arc::new(CancelRegistry::new()),
      Arc::clone(&rt),
      Arc::clone(&bus),
      "alice".to_string(),
      uuid.clone(),
      Arc::clone(&tooling),
      stream,
    );

    let envelope = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(envelope.id, uuid);
    assert_eq!(
      envelope.event,
      Event::ResourceUpdated(ResourceContent {
        uri: "file:///a".to_string(),
        mime_type: Some("text/plain".to_string()),
        content: "hello".to_string(),
      })
    );
    Ok(())
  }

  #[test]
  fn pump_delivers_error_when_resource_read_fails() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let subs = Arc::new(CancelRegistry::new());
    let tooling: Arc<dyn Tooling> = MockTooling::noop();
    let uuid = crate::host::bus::new_uuid();

    let stream = rt.block_on(tooling.subscribe_resource("file:///a"))?;
    spawn_pump(
      Arc::clone(&subs),
      Arc::clone(&rt),
      Arc::clone(&bus),
      "alice".to_string(),
      uuid.clone(),
      Arc::clone(&tooling),
      stream,
    );

    let envelope = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(envelope.id, uuid);
    assert!(matches!(envelope.event, Event::Error(_)));

    // The read failure is recoverable: the subscription stays alive.
    assert!(subs.is_open(&uuid));
    Ok(())
  }

  #[test]
  fn pump_delivers_resource_list_updated_with_resources() -> anyhow::Result<()>
  {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let resources = vec![ResourceInfo {
      uri: "file:///a".to_string(),
      name: "a".to_string(),
      description: None,
      mime_type: None,
    }];
    let tooling: Arc<dyn Tooling> =
      MockTooling::with_resources(resources.clone());
    let uuid = crate::host::bus::new_uuid();

    let stream = rt.block_on(tooling.subscribe_resource_list())?;
    spawn_pump(
      Arc::new(CancelRegistry::new()),
      Arc::clone(&rt),
      Arc::clone(&bus),
      "alice".to_string(),
      uuid.clone(),
      Arc::clone(&tooling),
      stream,
    );

    let envelope = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(envelope.id, uuid);
    assert_eq!(envelope.event, Event::ResourceListUpdated(resources));
    Ok(())
  }

  #[test]
  fn cancel_suppresses_resource_deliveries() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let subs = Arc::new(CancelRegistry::new());
    let tooling: Arc<dyn Tooling> = MockTooling::noop();
    let uuid = crate::host::bus::new_uuid();

    let stream = rt.block_on(tooling.subscribe_resource("file:///a"))?;
    spawn_pump(
      Arc::clone(&subs),
      Arc::clone(&rt),
      Arc::clone(&bus),
      "alice".to_string(),
      uuid.clone(),
      Arc::clone(&tooling),
      stream,
    );
    subs.cancel(&uuid);
    rt.block_on(async {
      tokio::time::sleep(Duration::from_millis(50)).await;
    });
    assert_eq!(bus.try_recv("alice")?, None);
    Ok(())
  }
}
