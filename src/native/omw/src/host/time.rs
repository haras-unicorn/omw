//! Host-side time types and the timer registry. Timestamps are `u64`
//! milliseconds since the Unix epoch ("ticks"). A `wait-*` registers a timer on
//! the bridge runtime: a task sleeps until its deadline then pushes an
//! `Event::Timer` (tagged with the wait's UUID on the envelope) into the
//! requesting agent's inbox.

use std::str::FromStr as _;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};

use crate::host::bus::MessageBus;
use crate::host::events::Event;

/// Milliseconds between the Unix epoch and `now`.
pub fn now_ticks() -> u64 {
  u64::try_from(now_ms()).unwrap_or_default()
}

/// Signed milliseconds since the Unix epoch.
fn now_ms() -> i64 {
  chrono::Utc::now().timestamp_millis()
}

/// Add `ms` milliseconds to a timestamp, saturating on overflow.
pub fn add(ts: u64, ms: u64) -> u64 {
  ts.saturating_add(ms)
}

/// Subtract `ms` milliseconds from a timestamp, saturating at zero.
pub fn sub(ts: u64, ms: u64) -> u64 {
  ts.saturating_sub(ms)
}

/// Milliseconds between `a` and `b` (signed; `a - b`).
pub fn diff(a: u64, b: u64) -> i64 {
  let a = i128::from(a);
  let b = i128::from(b);
  a.saturating_sub(b)
    .clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Format a timestamp using a strftime-style format string via `chrono`.
pub fn format(ts: u64, format: &str) -> String {
  match chrono_from_ticks(ts) {
    Some(dt) => dt.format(format).to_string(),
    None => String::new(),
  }
}

fn chrono_from_ticks(ts: u64) -> Option<chrono::DateTime<Utc>> {
  let ms = i64::try_from(ts).ok()?;
  Utc.timestamp_millis_opt(ms).single()
}

/// Wait until a timestamp fires, pushing a `timer` event tagged with `uuid`
/// into `name`'s inbox. Errors synchronously if `ts <= now`.
pub fn wait_timestamp(
  bus: &Arc<MessageBus>,
  rt: &Arc<tokio::runtime::Runtime>,
  name: &str,
  uuid: &str,
  ts: u64,
) -> Result<(), String> {
  let now = now_ticks();
  if ts <= now {
    tracing::warn!(agent = %name, uuid, ts, now, "wait-timestamp rejected a past tick");
    return Err("timestamp must be in the future".to_string());
  }
  let delay = Duration::from_millis(ts.saturating_sub(now));
  schedule(
    bus.clone(),
    rt.clone(),
    name.to_string(),
    uuid.to_string(),
    delay,
  );
  Ok(())
}

/// Wait for `ms` milliseconds, pushing a `timer` event tagged with `uuid` into
/// `name`'s inbox.
pub fn wait_duration(
  bus: &Arc<MessageBus>,
  rt: &Arc<tokio::runtime::Runtime>,
  name: &str,
  uuid: &str,
  ms: u64,
) {
  let delay = Duration::from_millis(ms);
  schedule(
    bus.clone(),
    rt.clone(),
    name.to_string(),
    uuid.to_string(),
    delay,
  );
}

/// Wait until the next fire of a cron spec, pushing a `timer` event tagged with
/// `uuid` into `name`'s inbox.
pub fn wait_cron(
  bus: &Arc<MessageBus>,
  rt: &Arc<tokio::runtime::Runtime>,
  name: &str,
  uuid: &str,
  spec: &str,
) -> Result<(), String> {
  let schedule_ = cron::Schedule::from_str(spec)
    .map_err(|e| format!("invalid cron spec {spec:?}: {e}"))?;
  let now = Utc::now();
  let next = schedule_
    .after(&now)
    .next()
    .ok_or_else(|| "cron schedule yields no future fire time".to_string())?;
  let delay = next
    .signed_duration_since(now)
    .to_std()
    .map_err(|e| e.to_string())?;
  schedule(
    bus.clone(),
    rt.clone(),
    name.to_string(),
    uuid.to_string(),
    delay,
  );
  Ok(())
}

/// Spawn a task on the bridge runtime that sleeps `delay` then delivers a
/// `timer` event tagged with `uuid` to `name`'s inbox.
fn schedule(
  bus: Arc<MessageBus>,
  rt: Arc<tokio::runtime::Runtime>,
  name: String,
  uuid: String,
  delay: Duration,
) {
  tracing::debug!(
    agent = %name,
    uuid = %uuid,
    delay_ms = delay.as_millis(),
    "timer registered"
  );
  rt.spawn(async move {
    tokio::time::sleep(delay).await;
    tracing::trace!(agent = %name, uuid = %uuid, "timer fired");
    bus.deliver(&name, &uuid, Event::Timer);
  });
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn add_saturates_on_overflow() {
    assert_eq!(add(1, 2), 3);
    assert_eq!(add(u64::MAX, 1), u64::MAX);
    assert_eq!(add(u64::MAX, u64::MAX), u64::MAX);
  }

  #[test]
  fn sub_saturates_at_zero() {
    assert_eq!(sub(5, 3), 2);
    assert_eq!(sub(3, 5), 0);
    assert_eq!(sub(3, u64::MAX), 0);
  }

  #[test]
  fn diff_is_signed_and_absolute() {
    assert_eq!(diff(5, 3), 2);
    assert_eq!(diff(3, 5), -2);
    assert_eq!(diff(u64::MAX, 0), i64::MAX);
  }

  #[test]
  fn format_renders_a_known_timestamp() {
    // 1970-01-01T00:00:00.123Z in UTC.
    let formatted = format(123, "%Y-%m-%dT%H:%M:%S%.3fZ");
    assert_eq!(formatted, "1970-01-01T00:00:00.123Z");
  }

  #[test]
  fn wait_timestamp_rejects_the_past() -> anyhow::Result<()> {
    let rt = Arc::new(tokio::runtime::Builder::new_current_thread().build()?);
    let bus = Arc::new(MessageBus::new());
    let err = wait_timestamp(&bus, &rt, "alice", "uuid", 1)
      .err()
      .ok_or_else(|| anyhow::anyhow!("expected an error"))?;
    assert!(err.contains("future"), "{err}");
    Ok(())
  }
}
