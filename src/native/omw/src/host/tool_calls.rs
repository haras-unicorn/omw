//! Host-side tool-call pumps. Each `tooling.call-tool` call registers an
//! in-flight invocation by UUID in a [`CancelRegistry`]; a pump task on the
//! bridge runtime awaits the `tooling.call_tool` result and delivers a
//! `tool-result` event (or an `error` event on failure) into the requesting
//! agent's inbox, tagged with that UUID. `is-open`/`cancel` go through
//! the registry, which doubles as the cancel signal: dropping an entry's
//! sender wakes its pump's cancel receiver.

use std::sync::Arc;

use serde_json::Value;

use crate::host::bus::MessageBus;
use crate::host::events::{Event, ToolResult};
use crate::host::streams::CancelRegistry;
use crate::tooling::Tooling;

/// Spawn a pump task on `rt` that awaits `tooling.call_tool(name, args)`
/// and delivers its result into `name`'s inbox tagged with `uuid`. A failure
/// is delivered as an [`Event::Error`] and a successful result as an
/// [`Event::ToolResult`]. Cancelling the invocation (via the shared registry)
/// suppresses but does not abort the underlying call.
#[allow(
  clippy::too_many_arguments,
  reason = "aggregating the bridge handles into a struct is left to a pumps refactor"
)]
pub fn spawn_pump(
  tooling: Arc<dyn Tooling>,
  rt: Arc<tokio::runtime::Runtime>,
  bus: Arc<MessageBus>,
  calls: Arc<CancelRegistry>,
  name: String,
  uuid: String,
  tool: String,
  args: Value,
) {
  let mut cancel = calls.open(uuid.clone());
  tracing::info!(agent = %name, uuid = %uuid, tool = %tool, "tool call queued");
  rt.spawn(async move {
    let arguments = args.to_string();
    let result = tooling.call_tool(&tool, args).await;
    tokio::select! {
      biased;

      _ = &mut cancel => {
        tracing::debug!(agent = %name, uuid = %uuid, tool = %tool, "tool call cancelled");
      }
      res = async { result } => {
        match res {
          Ok(result) => {
            tracing::trace!(agent = %name, uuid = %uuid, tool = %tool, result_bytes = result.len(), "tool call delivered");
            bus.deliver(
              &name,
              &uuid,
              Event::ToolResult(ToolResult {
                name: tool,
                arguments,
                result,
              }),
            );
          }
          Err(e) => {
            tracing::error!(agent = %name, uuid = %uuid, tool = %tool, error = %e, "tool call failed");
            bus.deliver(&name,&uuid, Event::Error(e.to_string()));
          }
        }
      }
    }
    calls.remove(&uuid);
  });
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;
  use std::time::Duration;

  use super::*;
  use crate::host::bus::MessageBus;
  use crate::tooling::Tooling;
  use crate::tooling::mock::MockTooling;

  #[test]
  fn pump_delivers_tool_result_tagged_with_uuid() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let tooling: Arc<dyn Tooling> = MockTooling::noop();
    let calls = Arc::new(CancelRegistry::new());
    let uuid = crate::host::bus::new_uuid();

    spawn_pump(
      Arc::clone(&tooling),
      Arc::clone(&rt),
      Arc::clone(&bus),
      Arc::clone(&calls),
      "alice".to_string(),
      uuid.clone(),
      "some-tool".to_string(),
      serde_json::json!({ "a": 1 }),
    );

    let envelope = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(envelope.id, uuid);
    assert_eq!(
      envelope.event,
      Event::ToolResult(ToolResult {
        name: "some-tool".to_string(),
        arguments: r#"{"a":1}"#.to_string(),
        result: String::new(),
      })
    );
    rt.block_on(async {
      tokio::time::sleep(Duration::from_millis(50)).await;
    });
    assert!(!calls.is_open(&uuid));
    Ok(())
  }

  #[test]
  fn pump_delivers_error_when_tool_call_fails() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let calls = Arc::new(CancelRegistry::new());
    let tooling = Arc::new(crate::tooling::mock::FailingTooling);
    let uuid = crate::host::bus::new_uuid();

    spawn_pump(
      tooling,
      Arc::clone(&rt),
      Arc::clone(&bus),
      Arc::clone(&calls),
      "alice".to_string(),
      uuid.clone(),
      "some-tool".to_string(),
      serde_json::Value::Null,
    );

    let envelope = bus.recv("alice", Duration::from_secs(5))?;
    assert_eq!(envelope.id, uuid);
    assert!(matches!(envelope.event, Event::Error(_)));
    rt.block_on(async {
      tokio::time::sleep(Duration::from_millis(50)).await;
    });
    assert!(!calls.is_open(&uuid));
    Ok(())
  }

  #[test]
  fn cancel_suppresses_tool_deliveries() -> anyhow::Result<()> {
    let rt = Arc::new(
      tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?,
    );
    let bus = Arc::new(MessageBus::new());
    let calls = Arc::new(CancelRegistry::new());
    let tooling: Arc<dyn Tooling> = MockTooling::noop();
    let uuid = crate::host::bus::new_uuid();

    spawn_pump(
      Arc::clone(&tooling),
      Arc::clone(&rt),
      Arc::clone(&bus),
      Arc::clone(&calls),
      "alice".to_string(),
      uuid.clone(),
      "some-tool".to_string(),
      serde_json::Value::Null,
    );

    calls.cancel(&uuid);
    rt.block_on(async {
      tokio::time::sleep(Duration::from_millis(50)).await;
    });
    assert_eq!(bus.try_recv("alice")?, None);
    assert!(!calls.is_open(&uuid));
    Ok(())
  }
}
