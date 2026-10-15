use boa_engine::{Context, JsArgs as _, JsError, JsNativeError, JsValue};

use crate::omw::omw::{provider, types};

pub(crate) fn js_err(message: impl Into<String>) -> JsError {
  JsNativeError::typ().with_message(message.into()).into()
}

pub(crate) fn string_of(value: &JsValue) -> Option<String> {
  value.as_string().map(|s| s.to_std_string_escaped())
}

/// Parse an opaque JSON string into a JSON value, falling back to the raw
/// string when it is not valid JSON.
pub(crate) fn json_or_string(raw: &str) -> serde_json::Value {
  serde_json::from_str(raw)
    .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

/// Coerce an opaque JSON string field from JS-land into the string the host
/// expects: a string is passed through, any other JSON value is serialized.
fn opaque_json(value: &serde_json::Value) -> Result<String, JsError> {
  match value {
    serde_json::Value::String(s) => Ok(s.clone()),
    other => serde_json::to_string(other).map_err(|e| js_err(e.to_string())),
  }
}

pub(crate) fn str_arg(
  args: &[JsValue],
  index: usize,
  name: &str,
) -> Result<String, JsError> {
  string_of(args.get_or_undefined(index))
    .ok_or_else(|| js_err(format!("{name} must be a string")))
}

pub(crate) fn u64_arg(
  args: &[JsValue],
  index: usize,
  name: &str,
) -> Result<u64, JsError> {
  let n = args
    .get_or_undefined(index)
    .as_number()
    .ok_or_else(|| js_err(format!("{name} must be a number")))?;
  if !n.is_finite() || n < 0.0 {
    return Err(js_err(format!("{name} must be a non-negative integer")));
  }
  Ok(n as u64)
}

pub(crate) fn handle_name(
  this: &JsValue,
  ctx: &mut Context,
) -> Result<String, JsError> {
  let obj = this
    .as_object()
    .ok_or_else(|| js_err("method must be called on a handle"))?;
  let name = obj.get(boa_engine::JsString::from("name"), ctx)?;
  string_of(&name).ok_or_else(|| js_err("handle object missing name"))
}

fn opt_str(
  obj: &serde_json::Map<String, serde_json::Value>,
  key: &str,
) -> Result<Option<String>, JsError> {
  match obj.get(key) {
    None | Some(serde_json::Value::Null) => Ok(None),
    Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
    Some(_) => Err(js_err(format!("{key} must be a string"))),
  }
}

fn req_str(
  obj: &serde_json::Map<String, serde_json::Value>,
  key: &str,
) -> Result<String, JsError> {
  match obj.get(key) {
    Some(serde_json::Value::String(s)) => Ok(s.clone()),
    _ => Err(js_err(format!("{key} must be a string"))),
  }
}

fn opt_bool(
  obj: &serde_json::Map<String, serde_json::Value>,
  key: &str,
) -> Result<Option<bool>, JsError> {
  match obj.get(key) {
    None | Some(serde_json::Value::Null) => Ok(None),
    Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
    Some(_) => Err(js_err(format!("{key} must be a boolean"))),
  }
}

fn tool_annotations_from_json(
  v: &serde_json::Value,
) -> Result<types::ToolAnnotations, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("annotations must be an object"))?;
  Ok(types::ToolAnnotations {
    read_only_hint: opt_bool(obj, "readOnlyHint")?,
    destructive_hint: opt_bool(obj, "destructiveHint")?,
    idempotent_hint: opt_bool(obj, "idempotentHint")?,
    open_world_hint: opt_bool(obj, "openWorldHint")?,
  })
}

fn tool_call_from_json(
  v: &serde_json::Value,
) -> Result<types::ToolCall, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("toolCall must be an object"))?;
  let arguments = obj
    .get("arguments")
    .ok_or_else(|| js_err("toolCall missing arguments"))?;
  Ok(types::ToolCall {
    id: req_str(obj, "id")?,
    name: req_str(obj, "name")?,
    arguments: opaque_json(arguments)?,
  })
}

fn tool_calls_from_json(
  v: Option<&serde_json::Value>,
) -> Result<Vec<types::ToolCall>, JsError> {
  match v {
    None | Some(serde_json::Value::Null) => Ok(Vec::new()),
    Some(value) => {
      let items = value
        .as_array()
        .ok_or_else(|| js_err("toolCalls must be an array"))?;
      items.iter().map(tool_call_from_json).collect()
    }
  }
}

fn tool_call_delta_from_json(
  v: &serde_json::Value,
) -> Result<types::ToolCallDelta, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("toolCall must be an object"))?;
  let index = obj
    .get("index")
    .and_then(|n| n.as_u64())
    .ok_or_else(|| js_err("toolCall index must be a non-negative integer"))?;
  let index =
    u32::try_from(index).map_err(|_| js_err("toolCall index out of range"))?;
  let arguments = match obj.get("arguments") {
    None | Some(serde_json::Value::Null) => None,
    Some(value) => Some(opaque_json(value)?),
  };
  Ok(types::ToolCallDelta {
    index,
    id: opt_str(obj, "id")?,
    name: opt_str(obj, "name")?,
    arguments,
  })
}

fn tool_call_deltas_from_json(
  v: Option<&serde_json::Value>,
) -> Result<Vec<types::ToolCallDelta>, JsError> {
  match v {
    None | Some(serde_json::Value::Null) => Ok(Vec::new()),
    Some(value) => {
      let items = value
        .as_array()
        .ok_or_else(|| js_err("toolCalls must be an array"))?;
      items.iter().map(tool_call_delta_from_json).collect()
    }
  }
}

pub(crate) fn msg_from_json(
  v: &serde_json::Value,
) -> Result<provider::ChatMessage, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("chat message must be an object"))?;
  let role = obj
    .get("role")
    .and_then(|r| r.as_str())
    .unwrap_or("user")
    .to_string();
  Ok(provider::ChatMessage {
    role,
    content: opt_str(obj, "content")?,
    reasoning: opt_str(obj, "reasoning")?,
    tool_calls: tool_calls_from_json(obj.get("toolCalls"))?,
    tool_call_id: opt_str(obj, "toolCallId")?,
  })
}

pub(crate) fn tool_from_json(
  v: &serde_json::Value,
) -> Result<provider::Tool, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("tool must be an object"))?;
  let input_schema = match obj.get("inputSchema") {
    Some(value) => opaque_json(value)?,
    None => "{}".to_string(),
  };
  let output_schema = match obj.get("outputSchema") {
    None | Some(serde_json::Value::Null) => None,
    Some(value) => Some(opaque_json(value)?),
  };
  let annotations = match obj.get("annotations") {
    None | Some(serde_json::Value::Null) => None,
    Some(value) => Some(tool_annotations_from_json(value)?),
  };
  Ok(provider::Tool {
    name: req_str(obj, "name")?,
    title: opt_str(obj, "title")?,
    description: opt_str(obj, "description")?,
    input_schema,
    output_schema,
    annotations,
  })
}

fn json_of(
  value: &JsValue,
  ctx: &mut Context,
  name: &str,
) -> Result<serde_json::Value, JsError> {
  value
    .to_json(ctx)?
    .ok_or_else(|| js_err(format!("{name} must be given")))
}

pub(crate) fn messages_from_js(
  value: &JsValue,
  ctx: &mut Context,
) -> Result<Vec<provider::ChatMessage>, JsError> {
  let json = json_of(value, ctx, "messages")?;
  let items = json
    .as_array()
    .ok_or_else(|| js_err("messages must be an array"))?;
  items.iter().map(msg_from_json).collect()
}

pub(crate) fn tools_from_js(
  value: &JsValue,
  ctx: &mut Context,
) -> Result<Vec<provider::Tool>, JsError> {
  let json = json_of(value, ctx, "tools")?;
  let items = json
    .as_array()
    .ok_or_else(|| js_err("tools must be an array"))?;
  items.iter().map(tool_from_json).collect()
}

/// Serialize optional JS params into the opaque JSON string the host expects.
/// Accepts an object (serialized to JSON) or an already-JSON string (passed
/// through, e.g. an `endpoint-message`'s `params`). `undefined`/`null`/empty
/// is `None`.
pub(crate) fn params_from_js(
  value: &JsValue,
  ctx: &mut Context,
) -> Result<Option<String>, JsError> {
  if value.is_undefined() || value.is_null() {
    return Ok(None);
  }
  if let Some(text) = string_of(value) {
    return Ok(if text.trim().is_empty() {
      None
    } else {
      Some(text)
    });
  }
  let json = value
    .to_json(ctx)?
    .ok_or_else(|| js_err("params must be an object or JSON string"))?;
  serde_json::to_string(&json)
    .map(Some)
    .map_err(|e| js_err(e.to_string()))
}

pub(crate) fn json_string_from_js(
  value: &JsValue,
  ctx: &mut Context,
) -> Result<String, JsError> {
  let json = json_of(value, ctx, "arguments")?;
  serde_json::to_string(&json).map_err(|e| js_err(e.to_string()))
}

pub(crate) fn bytes_from_js(
  value: &JsValue,
  ctx: &mut Context,
) -> Result<Vec<u8>, JsError> {
  let json = json_of(value, ctx, "bytes")?;
  let items = json
    .as_array()
    .ok_or_else(|| js_err("bytes must be an array"))?;
  items
    .iter()
    .map(|n| {
      let n = n.as_f64().ok_or_else(|| js_err("byte must be a number"))?;
      if !(0.0..=255.0).contains(&n) || n.fract() != 0.0 {
        return Err(js_err("byte out of range 0..=255"));
      }
      Ok(n as u8)
    })
    .collect()
}

pub(crate) fn bytes_to_js(
  bytes: &[u8],
  ctx: &mut Context,
) -> Result<JsValue, JsError> {
  let json = serde_json::Value::Array(
    bytes.iter().map(|b| serde_json::Value::from(*b)).collect(),
  );
  JsValue::from_json(&json, ctx)
}

pub(crate) fn value_from_json(
  json: &serde_json::Value,
  ctx: &mut Context,
) -> Result<JsValue, JsError> {
  JsValue::from_json(json, ctx)
}

fn tool_call_to_json(tc: types::ToolCall) -> serde_json::Value {
  serde_json::json!({
    "id": tc.id,
    "name": tc.name,
    "arguments": json_or_string(&tc.arguments),
  })
}

fn tool_call_delta_to_json(tc: types::ToolCallDelta) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("index".to_string(), serde_json::Value::from(tc.index));
  if let Some(id) = tc.id {
    o.insert("id".to_string(), serde_json::Value::String(id));
  }
  if let Some(name) = tc.name {
    o.insert("name".to_string(), serde_json::Value::String(name));
  }
  if let Some(arguments) = tc.arguments {
    o.insert("arguments".to_string(), json_or_string(&arguments));
  }
  serde_json::Value::Object(o)
}

fn chat_message_to_json(m: types::ChatMessage) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("role".to_string(), serde_json::Value::String(m.role));
  if let Some(content) = m.content {
    o.insert("content".to_string(), serde_json::Value::String(content));
  }
  if let Some(reasoning) = m.reasoning {
    o.insert(
      "reasoning".to_string(),
      serde_json::Value::String(reasoning),
    );
  }
  if !m.tool_calls.is_empty() {
    o.insert(
      "toolCalls".to_string(),
      serde_json::Value::Array(
        m.tool_calls.into_iter().map(tool_call_to_json).collect(),
      ),
    );
  }
  if let Some(tool_call_id) = m.tool_call_id {
    o.insert(
      "toolCallId".to_string(),
      serde_json::Value::String(tool_call_id),
    );
  }
  serde_json::Value::Object(o)
}

pub(crate) fn tool_annotations_to_json(
  a: types::ToolAnnotations,
) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  if let Some(read_only) = a.read_only_hint {
    o.insert(
      "readOnlyHint".to_string(),
      serde_json::Value::Bool(read_only),
    );
  }
  if let Some(destructive) = a.destructive_hint {
    o.insert(
      "destructiveHint".to_string(),
      serde_json::Value::Bool(destructive),
    );
  }
  if let Some(idempotent) = a.idempotent_hint {
    o.insert(
      "idempotentHint".to_string(),
      serde_json::Value::Bool(idempotent),
    );
  }
  if let Some(open_world) = a.open_world_hint {
    o.insert(
      "openWorldHint".to_string(),
      serde_json::Value::Bool(open_world),
    );
  }
  serde_json::Value::Object(o)
}

pub(crate) fn resource_annotations_to_json(
  a: types::ResourceAnnotations,
) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert(
    "audience".to_string(),
    serde_json::Value::Array(
      a.audience
        .into_iter()
        .map(serde_json::Value::String)
        .collect(),
    ),
  );
  if let Some(priority) = a.priority
    && let Some(n) = serde_json::Number::from_f64(priority)
  {
    o.insert("priority".to_string(), serde_json::Value::Number(n));
  }
  if let Some(last_modified) = a.last_modified {
    o.insert(
      "lastModified".to_string(),
      serde_json::Value::String(last_modified),
    );
  }
  serde_json::Value::Object(o)
}

pub(crate) fn tool_to_json(t: types::Tool) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("name".to_string(), serde_json::Value::String(t.name));
  if let Some(title) = t.title {
    o.insert("title".to_string(), serde_json::Value::String(title));
  }
  if let Some(description) = t.description {
    o.insert(
      "description".to_string(),
      serde_json::Value::String(description),
    );
  }
  o.insert("inputSchema".to_string(), json_or_string(&t.input_schema));
  if let Some(output_schema) = t.output_schema {
    o.insert("outputSchema".to_string(), json_or_string(&output_schema));
  }
  if let Some(annotations) = t.annotations {
    o.insert(
      "annotations".to_string(),
      tool_annotations_to_json(annotations),
    );
  }
  serde_json::Value::Object(o)
}

/// Coerce an opaque JSON field from JS-land into the string the host expects:
/// a string is passed through, any other JSON value is serialized.
fn usage_from_value(
  v: Option<&serde_json::Value>,
) -> Result<Option<String>, JsError> {
  match v {
    None | Some(serde_json::Value::Null) => Ok(None),
    Some(value) => Ok(Some(opaque_json(value)?)),
  }
}

fn delta_to_json(delta: types::ChatDelta) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  if let Some(content) = delta.content {
    o.insert("content".to_string(), serde_json::Value::String(content));
  }
  if let Some(reasoning) = delta.reasoning {
    o.insert(
      "reasoning".to_string(),
      serde_json::Value::String(reasoning),
    );
  }
  if !delta.tool_calls.is_empty() {
    o.insert(
      "toolCalls".to_string(),
      serde_json::Value::Array(
        delta
          .tool_calls
          .into_iter()
          .map(tool_call_delta_to_json)
          .collect(),
      ),
    );
  }
  if let Some(finish_reason) = delta.finish_reason {
    o.insert(
      "finishReason".to_string(),
      serde_json::Value::String(finish_reason),
    );
  }
  if let Some(usage) = delta.usage {
    o.insert("usage".to_string(), json_or_string(&usage));
  }
  serde_json::Value::Object(o)
}

pub(crate) fn delta_from_js(
  value: &JsValue,
  ctx: &mut Context,
) -> Result<types::ChatDelta, JsError> {
  let json = json_of(value, ctx, "delta")?;
  let obj = json
    .as_object()
    .ok_or_else(|| js_err("delta must be an object"))?;
  Ok(types::ChatDelta {
    content: opt_str(obj, "content")?,
    reasoning: opt_str(obj, "reasoning")?,
    tool_calls: tool_call_deltas_from_json(obj.get("toolCalls"))?,
    finish_reason: opt_str(obj, "finishReason")?,
    usage: usage_from_value(obj.get("usage"))?,
  })
}

pub(crate) fn tool_result_to_json(r: types::ToolResult) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("name".to_string(), serde_json::Value::String(r.name));
  o.insert("arguments".to_string(), json_or_string(&r.arguments));
  o.insert("content".to_string(), json_or_string(&r.content));
  if let Some(structured) = r.structured_content {
    o.insert("structuredContent".to_string(), json_or_string(&structured));
  }
  o.insert("isError".to_string(), serde_json::Value::Bool(r.is_error));
  serde_json::Value::Object(o)
}

pub(crate) fn chat_result_to_json(r: types::ChatResult) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  if let Some(content) = r.content {
    o.insert("content".to_string(), serde_json::Value::String(content));
  }
  if let Some(reasoning) = r.reasoning {
    o.insert(
      "reasoning".to_string(),
      serde_json::Value::String(reasoning),
    );
  }
  o.insert(
    "toolCalls".to_string(),
    serde_json::Value::Array(
      r.tool_calls.into_iter().map(tool_call_to_json).collect(),
    ),
  );
  if let Some(finish_reason) = r.finish_reason {
    o.insert(
      "finishReason".to_string(),
      serde_json::Value::String(finish_reason),
    );
  }
  if let Some(usage) = r.usage {
    o.insert("usage".to_string(), json_or_string(&usage));
  }
  serde_json::Value::Object(o)
}

pub(crate) fn resource_to_json(r: types::ResourceInfo) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("uri".to_string(), serde_json::Value::String(r.uri));
  o.insert("name".to_string(), serde_json::Value::String(r.name));
  if let Some(title) = r.title {
    o.insert("title".to_string(), serde_json::Value::String(title));
  }
  if let Some(description) = r.description {
    o.insert(
      "description".to_string(),
      serde_json::Value::String(description),
    );
  }
  if let Some(mime_type) = r.mime_type {
    o.insert("mimeType".to_string(), serde_json::Value::String(mime_type));
  }
  if let Some(size) = r.size {
    o.insert("size".to_string(), serde_json::Value::from(size));
  }
  if let Some(annotations) = r.annotations {
    o.insert(
      "annotations".to_string(),
      resource_annotations_to_json(annotations),
    );
  }
  serde_json::Value::Object(o)
}

pub(crate) fn resource_content_to_json(
  c: types::ResourceContent,
) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("uri".to_string(), serde_json::Value::String(c.uri));
  if let Some(mime_type) = c.mime_type {
    o.insert("mimeType".to_string(), serde_json::Value::String(mime_type));
  }
  o.insert("content".to_string(), serde_json::Value::String(c.content));
  serde_json::Value::Object(o)
}

pub(crate) fn envelope_to_js(
  envelope: crate::omw::omw::host::EventEnvelope,
  ctx: &mut Context,
) -> Result<JsValue, JsError> {
  use crate::omw::omw::types::Event;
  let (kind, payload): (&str, serde_json::Value) = match envelope.event {
    Event::Message(payload) => ("message", serde_json::Value::String(payload)),
    Event::Error(message) => ("error", serde_json::Value::String(message)),
    Event::Timer => ("timer", serde_json::Value::Null),
    Event::Reload => ("reload", serde_json::Value::Null),
    Event::Shutdown => ("shutdown", serde_json::Value::Null),
    Event::ChatDelta(delta) => ("chat-delta", delta_to_json(delta)),
    Event::ChatEnd => ("chat-end", serde_json::Value::Null),
    Event::ToolResult(result) => ("tool-result", tool_result_to_json(result)),
    Event::ResourceListUpdated(resources) => (
      "resource-list-updated",
      serde_json::Value::Array(
        resources.into_iter().map(resource_to_json).collect(),
      ),
    ),
    Event::ResourceUpdated(contents) => (
      "resource-updated",
      serde_json::Value::Array(
        contents.into_iter().map(resource_content_to_json).collect(),
      ),
    ),
    Event::EndpointMessage(message) => {
      let messages = message.messages.into_iter().map(chat_message_to_json);
      let tools = message.tools.into_iter().map(tool_to_json);
      let mut o = serde_json::Map::new();
      o.insert(
        "session".to_string(),
        serde_json::Value::String(message.session),
      );
      o.insert(
        "messages".to_string(),
        serde_json::Value::Array(messages.collect()),
      );
      o.insert(
        "tools".to_string(),
        serde_json::Value::Array(tools.collect()),
      );
      if let Some(params) = message.params {
        o.insert("params".to_string(), json_or_string(&params));
      }
      ("endpoint-message", serde_json::Value::Object(o))
    }
    Event::EndpointSessionEnd(end) => {
      let mut o = serde_json::Map::new();
      o.insert(
        "session".to_string(),
        serde_json::Value::String(end.session),
      );
      if let Some(error) = end.error {
        o.insert("error".to_string(), serde_json::Value::String(error));
      }
      ("endpoint-session-end", serde_json::Value::Object(o))
    }
  };
  let json = serde_json::json!({
    "id": envelope.id,
    "kind": kind,
    "payload": payload,
  });
  JsValue::from_json(&json, ctx)
}
