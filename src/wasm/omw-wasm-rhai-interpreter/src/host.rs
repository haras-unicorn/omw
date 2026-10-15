use rhai_rt::{Array, Blob, Dynamic, EvalAltResult, Map};

use crate::{
  convert::{arguments_to_json, json_to_dynamic, to_error},
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

pub(crate) fn host_whoami() -> Result<String, Box<EvalAltResult>> {
  Ok(host::whoami())
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

/// Read a memory value by key, parsing it as JSON when possible. Unit when
/// absent; otherwise the parsed map/array/scalar, or the raw string when the
/// stored value is not JSON.
pub(crate) fn host_memory_get_as(
  key: &str,
) -> Result<Dynamic, Box<EvalAltResult>> {
  let value: Dynamic = match host::memory_get(key) {
    Some(value) => json_to_dynamic(&value),
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

/// Store a memory value under a key, JSON-encoding it. Errors when the value
/// cannot be serialized.
pub(crate) fn host_memory_set_as(
  key: &str,
  value: Dynamic,
) -> Result<(), Box<EvalAltResult>> {
  let json = crate::convert::dynamic_to_json(&value).map_err(to_error)?;
  host::memory_set(key, &json);
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
    types::Event::ResourceUpdated(contents) => {
      let mut arr = Array::new();
      for c in contents {
        arr.push(resource_content_to_map(c).into());
      }
      ("resource-updated", arr.into())
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
      if let Some(params) = message.params {
        payload.insert("params".into(), json_to_dynamic(&params));
      }
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
/// `reasoning`, `tool_calls` (each a fragment with `index`/`id`/`name`/
/// `arguments`), `finish_reason`, and `usage` off a `"chat-delta"` event's
/// payload.
pub(crate) fn delta_to_map(delta: types::ChatDelta) -> Map {
  let mut m = Map::new();
  if let Some(content) = delta.content {
    m.insert("content".into(), content.into());
  }
  if let Some(reasoning) = delta.reasoning {
    m.insert("reasoning".into(), reasoning.into());
  }
  if !delta.tool_calls.is_empty() {
    let mut arr = Array::new();
    for tc in delta.tool_calls {
      arr.push(tool_call_delta_to_map(tc).into());
    }
    m.insert("tool_calls".into(), arr.into());
  }
  if let Some(finish_reason) = delta.finish_reason {
    m.insert("finish_reason".into(), finish_reason.into());
  }
  if let Some(usage) = delta.usage {
    m.insert("usage".into(), json_to_dynamic(&usage));
  }
  m
}

/// Map a `types::ToolCallDelta` fragment into a rhai map.
pub(crate) fn tool_call_delta_to_map(tc: types::ToolCallDelta) -> Map {
  let mut m = Map::new();
  m.insert("index".into(), i64::from(tc.index).into());
  if let Some(id) = tc.id {
    m.insert("id".into(), id.into());
  }
  if let Some(name) = tc.name {
    m.insert("name".into(), name.into());
  }
  if let Some(arguments) = tc.arguments {
    m.insert("arguments".into(), json_to_dynamic(&arguments));
  }
  m
}

/// Map a `types::ChatDelta` back out of a rhai map (the inverse of
/// [`delta_to_map`]). Reads `content`, `reasoning`, `tool_calls` (an array of
/// fragment maps), `finish_reason`, and `usage`.
pub(crate) fn delta_from_map(delta: &Map) -> Result<types::ChatDelta, String> {
  let content = delta
    .get("content")
    .and_then(|c| c.clone().try_cast::<String>());
  let reasoning = delta
    .get("reasoning")
    .and_then(|r| r.clone().try_cast::<String>());
  let tool_calls = match delta.get("tool_calls") {
    Some(value) => tool_call_deltas_from_dynamic(value.clone())?,
    None => Vec::new(),
  };
  let finish_reason = delta
    .get("finish_reason")
    .and_then(|f| f.clone().try_cast::<String>());
  let usage = match delta.get("usage") {
    Some(value) => Some(usage_from_dynamic(value.clone())?),
    None => None,
  };
  Ok(types::ChatDelta {
    content,
    reasoning,
    tool_calls,
    finish_reason,
    usage,
  })
}

fn tool_call_deltas_from_dynamic(
  value: Dynamic,
) -> Result<Vec<types::ToolCallDelta>, String> {
  let arr = value
    .try_cast::<Array>()
    .ok_or_else(|| "tool_calls must be an array".to_string())?;
  arr.into_iter().map(tool_call_delta_from_dynamic).collect()
}

fn tool_call_delta_from_dynamic(
  value: Dynamic,
) -> Result<types::ToolCallDelta, String> {
  let map = value
    .try_cast::<Map>()
    .ok_or_else(|| "tool_call must be a map".to_string())?;
  let index = map
    .get("index")
    .and_then(|v| v.clone().try_cast::<i64>())
    .and_then(|v| u32::try_from(v).ok())
    .unwrap_or(0);
  let id = map.get("id").and_then(|v| v.clone().try_cast::<String>());
  let name = map.get("name").and_then(|v| v.clone().try_cast::<String>());
  let arguments = map.get("arguments").map(arguments_to_json).transpose()?;
  Ok(types::ToolCallDelta {
    index,
    id,
    name,
    arguments,
  })
}

/// Coerce a rhai usage value into the opaque JSON string the host expects: a
/// string passes through, anything else is serialized.
fn usage_from_dynamic(value: Dynamic) -> Result<String, String> {
  if let Some(text) = value.clone().try_cast::<String>() {
    return Ok(text);
  }
  crate::convert::dynamic_to_json(&value)
}

/// Map a `types::ChatMessage` into a rhai map so scripts can read `role`,
/// `content`, `reasoning`, `tool_calls`, and `tool_call_id` off an
/// `endpoint-message` event's messages.
pub(crate) fn chat_message_to_map(m: types::ChatMessage) -> Map {
  let mut r = Map::new();
  r.insert("role".into(), m.role.into());
  if let Some(content) = m.content {
    r.insert("content".into(), content.into());
  }
  if let Some(reasoning) = m.reasoning {
    r.insert("reasoning".into(), reasoning.into());
  }
  if !m.tool_calls.is_empty() {
    let mut arr = Array::new();
    for tc in m.tool_calls {
      arr.push(tool_call_to_map(tc).into());
    }
    r.insert("tool_calls".into(), arr.into());
  }
  if let Some(tool_call_id) = m.tool_call_id {
    r.insert("tool_call_id".into(), tool_call_id.into());
  }
  r
}

/// Map a `types::Tool` into a rhai map so scripts can read `name`, `title`,
/// `description`, `input_schema`, `output_schema` and `annotations` off an
/// `endpoint-message` event's tools.
pub(crate) fn tool_to_map(t: types::Tool) -> Map {
  let mut m = Map::new();
  if let Some(desc) = t.description {
    m.insert("description".into(), desc.into());
  }
  m.insert("name".into(), t.name.into());
  if let Some(title) = t.title {
    m.insert("title".into(), title.into());
  }
  m.insert("input_schema".into(), json_to_dynamic(&t.input_schema));
  if let Some(output_schema) = t.output_schema {
    m.insert("output_schema".into(), json_to_dynamic(&output_schema));
  }
  if let Some(annotations) = t.annotations {
    m.insert(
      "annotations".into(),
      tool_annotations_to_map(annotations).into(),
    );
  }
  m
}

/// Map `types::ToolAnnotations` into a rhai map with `read_only_hint`,
/// `destructive_hint`, `idempotent_hint` and `open_world_hint`.
pub(crate) fn tool_annotations_to_map(a: types::ToolAnnotations) -> Map {
  let mut m = Map::new();
  if let Some(value) = a.read_only_hint {
    m.insert("read_only_hint".into(), value.into());
  }
  if let Some(value) = a.destructive_hint {
    m.insert("destructive_hint".into(), value.into());
  }
  if let Some(value) = a.idempotent_hint {
    m.insert("idempotent_hint".into(), value.into());
  }
  if let Some(value) = a.open_world_hint {
    m.insert("open_world_hint".into(), value.into());
  }
  m
}

/// Map a `types::ToolCall` into a rhai map so scripts can read `id`/
/// `name`/`arguments` off a tool call. `arguments` is parsed as JSON when
/// possible.
pub(crate) fn tool_call_to_map(tc: types::ToolCall) -> Map {
  let mut m = Map::new();
  m.insert("id".into(), tc.id.into());
  m.insert("name".into(), tc.name.into());
  m.insert("arguments".into(), json_to_dynamic(&tc.arguments));
  m
}
