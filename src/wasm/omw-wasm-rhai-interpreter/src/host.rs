use rhai_rt::{Array, Blob, Dynamic, EvalAltResult, Map};

use crate::{
  convert::to_error,
  omw::omw::{host, types},
  resource_content_to_map, resource_to_map, tool_result_to_map,
};

pub(crate) fn host_log(
  level: &str,
  message: &str,
) -> Result<(), Box<EvalAltResult>> {
  host::log(level, message);
  Ok(())
}

// The time helpers take/return rhai `i64` (rhai's default integer type) and
// convert at the boundary to the WIT `u64`/`s64`, so scripts can pass plain
// integer literals.
pub(crate) fn host_time_now() -> Result<i64, Box<EvalAltResult>> {
  i64::try_from(host::time_now()).map_err(to_error)
}

pub(crate) fn host_time_format(
  ts: i64,
  format: &str,
) -> Result<String, Box<EvalAltResult>> {
  Ok(host::time_format(to_u64(ts)?, format))
}

pub(crate) fn host_wait_until(ts: i64) -> Result<String, Box<EvalAltResult>> {
  host::wait_until(to_u64(ts)?).map_err(to_error)
}

pub(crate) fn host_wait_for(ms: i64) -> Result<String, Box<EvalAltResult>> {
  host::wait_for(to_u64(ms)?).map_err(to_error)
}

pub(crate) fn host_wait_cron(spec: &str) -> Result<String, Box<EvalAltResult>> {
  host::wait_cron(spec).map_err(to_error)
}

pub(crate) fn host_sleep_for(ms: i64) -> Result<(), Box<EvalAltResult>> {
  host::sleep_for(to_u64(ms)?);
  Ok(())
}

pub(crate) fn host_sleep_until(ts: i64) -> Result<(), Box<EvalAltResult>> {
  host::sleep_until(to_u64(ts)?).map_err(to_error)
}

pub(crate) fn host_sleep_cron(spec: &str) -> Result<(), Box<EvalAltResult>> {
  host::sleep_cron(spec).map_err(to_error)
}

pub(crate) fn host_cancel_timer(uuid: &str) -> Result<(), Box<EvalAltResult>> {
  host::cancel_timer(uuid);
  Ok(())
}

pub(crate) fn host_subscribe_agent(
  agent: &str,
) -> Result<String, Box<EvalAltResult>> {
  host::subscribe_agent(agent).map_err(to_error)
}

pub(crate) fn host_unsubscribe_agent(
  uuid: &str,
) -> Result<(), Box<EvalAltResult>> {
  host::unsubscribe_agent(uuid);
  Ok(())
}

pub(crate) fn host_subscribe_lifecycle() -> Result<String, Box<EvalAltResult>> {
  host::subscribe_lifecycle().map_err(to_error)
}

pub(crate) fn host_unsubscribe_lifecycle(
  uuid: &str,
) -> Result<(), Box<EvalAltResult>> {
  host::unsubscribe_lifecycle(uuid);
  Ok(())
}

/// Convert a rhai `i64` to a WIT `u64`, rejecting negatives.
pub(crate) fn to_u64(v: i64) -> Result<u64, Box<EvalAltResult>> {
  u64::try_from(v).map_err(|_| to_error("expected a non-negative integer"))
}

pub(crate) fn host_send_agent(
  agent: &str,
  payload: &str,
) -> Result<(), Box<EvalAltResult>> {
  host::send_agent(agent, payload);
  Ok(())
}

pub(crate) fn host_recv() -> Result<Map, Box<EvalAltResult>> {
  let envelope = host::recv().map_err(to_error)?;
  Ok(envelope_to_map(envelope))
}

pub(crate) fn host_try_recv() -> Result<Dynamic, Box<EvalAltResult>> {
  let envelope = host::try_recv().map_err(to_error)?;
  let value: Dynamic = match envelope {
    Some(envelope) => envelope_to_map(envelope).into(),
    None => ().into(),
  };
  Ok(value)
}

pub(crate) fn host_new_uuid() -> Result<String, Box<EvalAltResult>> {
  Ok(host::new_uuid())
}

/// Encode raw bytes (a rhai blob) as standard padded base64.
pub(crate) fn host_base64_encode(
  bytes: Blob,
) -> Result<String, Box<EvalAltResult>> {
  Ok(host::base64_encode(&bytes))
}

/// Decode standard padded base64 back to raw bytes (a rhai blob).
pub(crate) fn host_base64_decode(
  data: &str,
) -> Result<Blob, Box<EvalAltResult>> {
  host::base64_decode(data).map_err(to_error)
}

pub(crate) fn host_memory_get(
  key: &str,
) -> Result<Dynamic, Box<EvalAltResult>> {
  let value: Dynamic = match host::memory_get(key) {
    Some(value) => value.into(),
    None => ().into(),
  };
  Ok(value)
}

pub(crate) fn host_memory_set(
  key: &str,
  value: &str,
) -> Result<(), Box<EvalAltResult>> {
  host::memory_set(key, value);
  Ok(())
}

pub(crate) fn host_memory_remove(
  key: &str,
) -> Result<bool, Box<EvalAltResult>> {
  Ok(host::memory_remove(key))
}

/// Subscribe this agent to the endpoint under the model name `model`. Returns
/// the subscription UUID; inbound endpoint requests arrive as
/// `"endpoint-message"` events tagged with it.
pub(crate) fn host_subscribe_endpoint(
  model: &str,
) -> Result<String, Box<EvalAltResult>> {
  host::subscribe_endpoint(model).map_err(to_error)
}

/// Unsubscribe by a UUID that `subscribe_endpoint` returned; the model is
/// dropped and every in-flight session of that subscription is terminated.
pub(crate) fn host_unsubscribe_endpoint(
  uuid: &str,
) -> Result<(), Box<EvalAltResult>> {
  host::unsubscribe_endpoint(uuid);
  Ok(())
}

/// Stream one `chat-delta` map to the endpoint session identified by `session`,
/// non-blocking. A map carrying a `finish_reason` ends the session.
pub(crate) fn host_stream_endpoint(
  session: &str,
  delta: Map,
) -> Result<(), Box<EvalAltResult>> {
  let delta = delta_from_map(&delta).map_err(to_error)?;
  host::stream_endpoint(session, &delta).map_err(to_error)
}

/// Map a `host::EventEnvelope` into a rhai map `#{ id, kind, payload }` so
/// scripts can inspect `e.id` and match events against their handles.
pub(crate) fn envelope_to_map(envelope: host::EventEnvelope) -> Map {
  let mut m = Map::new();
  m.insert("id".into(), envelope.id.into());
  let (kind, payload): (&str, Dynamic) = match envelope.event {
    types::Event::Message(payload) => ("message", payload.into()),
    types::Event::Error(message) => ("error", message.into()),
    types::Event::Timer => ("timer", ().into()),
    types::Event::Reload => ("reload", ().into()),
    types::Event::Shutdown => ("shutdown", ().into()),
    types::Event::ChatDelta(delta) => {
      ("chat-delta", delta_to_map(delta).into())
    }
    types::Event::ChatEnd => ("chat-end", ().into()),
    types::Event::ToolResult(result) => {
      ("tool-result", tool_result_to_map(result).into())
    }
    types::Event::ResourceListUpdated(resources) => {
      let mut arr = Array::new();
      for r in resources {
        arr.push(resource_to_map(r).into());
      }
      ("resource-list-updated", arr.into())
    }
    types::Event::ResourceUpdated(content) => {
      ("resource-updated", resource_content_to_map(content).into())
    }
    types::Event::EndpointMessage(message) => {
      let mut payload = Map::new();
      payload.insert("session".into(), message.session.into());
      let mut messages = Array::new();
      for m in message.messages {
        messages.push(chat_message_to_map(m).into());
      }
      payload.insert("messages".into(), messages.into());
      let mut tools = Array::new();
      for t in message.tools {
        tools.push(tool_to_map(t).into());
      }
      payload.insert("tools".into(), tools.into());
      ("endpoint-message", payload.into())
    }
    types::Event::EndpointSessionEnd(end) => {
      let mut payload = Map::new();
      payload.insert("session".into(), end.session.into());
      if let Some(error) = end.error {
        payload.insert("error".into(), error.into());
      }
      ("endpoint-session-end", payload.into())
    }
  };
  m.insert("kind".into(), kind.into());
  m.insert("payload".into(), payload);
  m
}

/// Map a `types::ChatDelta` into a rhai map so scripts can read `content`,
/// `tool_call` and `finish_reason` off a `"chat-delta"` event's payload.
pub(crate) fn delta_to_map(delta: types::ChatDelta) -> Map {
  let mut m = Map::new();
  if let Some(content) = delta.content {
    m.insert("content".into(), content.into());
  }
  if let Some(tc) = delta.tool_call {
    let mut t = Map::new();
    t.insert("id".into(), tc.id.into());
    t.insert("name".into(), tc.name.into());
    t.insert("arguments".into(), tc.arguments.into());
    m.insert("tool_call".into(), t.into());
  }
  if let Some(finish_reason) = delta.finish_reason {
    m.insert("finish_reason".into(), finish_reason.into());
  }
  m
}

/// Map a `types::ChatDelta` back out of a rhai map (the inverse of
/// [`delta_to_map`]). Reads `content`, `tool_call` (a map with `id`/
/// `name`/`arguments`), and `finish_reason` off it.
pub(crate) fn delta_from_map(delta: &Map) -> Result<types::ChatDelta, String> {
  let content = delta
    .get("content")
    .and_then(|c| c.clone().try_cast::<String>());
  let tool_call = match delta.get("tool_call") {
    Some(tc) => {
      let tc = tc
        .clone()
        .try_cast::<Map>()
        .ok_or_else(|| "tool_call must be a map".to_string())?;
      let id = tc
        .get("id")
        .and_then(|v| v.clone().try_cast::<String>())
        .ok_or_else(|| "tool_call missing id".to_string())?;
      let name = tc
        .get("name")
        .and_then(|v| v.clone().try_cast::<String>())
        .ok_or_else(|| "tool_call missing name".to_string())?;
      let arguments = tc
        .get("arguments")
        .and_then(|v| v.clone().try_cast::<String>())
        .ok_or_else(|| "tool_call missing arguments".to_string())?;
      Some(types::ToolCall {
        id,
        name,
        arguments,
      })
    }
    None => None,
  };
  let finish_reason = delta
    .get("finish_reason")
    .and_then(|f| f.clone().try_cast::<String>());
  Ok(types::ChatDelta {
    content,
    tool_call,
    finish_reason,
  })
}

/// Map a `types::ChatMessage` into a rhai map so scripts can read `role`,
/// `content`,and `tool_call` off an `endpoint-message` event's messages.
pub(crate) fn chat_message_to_map(m: types::ChatMessage) -> Map {
  let mut r = Map::new();
  if let Some(content) = m.content {
    r.insert("content".into(), content.into());
  }
  if let Some(tc) = m.tool_call {
    r.insert("tool_call".into(), tool_call_to_map(tc).into());
  }
  let role = match m.role {
    types::Role::System => "system",
    types::Role::User => "user",
    types::Role::Assistant => "assistant",
    types::Role::Tool => "tool",
  };
  r.insert("role".into(), role.into());
  r
}

/// Map a `types::Tool` into a rhai map so scripts can read `name`,
/// `description`,and `input_schema` off an `endpoint-message` event's tools.
pub(crate) fn tool_to_map(t: types::Tool) -> Map {
  let mut m = Map::new();
  if let Some(desc) = t.description {
    m.insert("description".into(), desc.into());
  }
  m.insert("name".into(), t.name.into());
  m.insert("input_schema".into(), t.input_schema.into());
  m
}

/// Map a `types::ToolCall` into a rhai map so scripts can read `id`/
/// `name`/`arguments` off a tool call.
pub(crate) fn tool_call_to_map(tc: types::ToolCall) -> Map {
  let mut m = Map::new();
  m.insert("id".into(), tc.id.into());
  m.insert("name".into(), tc.name.into());
  m.insert("arguments".into(), tc.arguments.into());
  m
}
