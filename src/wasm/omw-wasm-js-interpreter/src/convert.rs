use boa_engine::{Context, JsArgs as _, JsError, JsNativeError, JsValue};

use crate::omw::omw::{provider, types};

pub(crate) fn js_err(message: impl Into<String>) -> JsError {
  JsNativeError::typ().with_message(message.into()).into()
}

pub(crate) fn string_of(value: &JsValue) -> Option<String> {
  value.as_string().map(|s| s.to_std_string_escaped())
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

fn tool_call_from_json(
  v: &serde_json::Value,
) -> Result<types::ToolCall, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("tool_call must be an object"))?;
  Ok(types::ToolCall {
    id: req_str(obj, "id")?,
    name: req_str(obj, "name")?,
    arguments: req_str(obj, "arguments")?,
  })
}

pub(crate) fn msg_from_json(
  v: &serde_json::Value,
) -> Result<provider::ChatMessage, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("chat message must be an object"))?;
  let role = match obj.get("role").and_then(|r| r.as_str()) {
    Some("system") => provider::Role::System,
    Some("assistant") => provider::Role::Assistant,
    Some("tool") => provider::Role::Tool,
    _ => provider::Role::User,
  };
  Ok(provider::ChatMessage {
    role,
    content: opt_str(obj, "content")?,
    reasoning: opt_str(obj, "reasoning")?,
    tool_call: match obj.get("tool_call") {
      None | Some(serde_json::Value::Null) => None,
      Some(tc) => Some(tool_call_from_json(tc)?),
    },
  })
}

pub(crate) fn tool_from_json(
  v: &serde_json::Value,
) -> Result<provider::Tool, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("tool must be an object"))?;
  let input_schema = match obj.get("input_schema") {
    Some(serde_json::Value::String(s)) => s.clone(),
    _ => "{}".to_string(),
  };
  let output_schema = match obj.get("output_schema") {
    Some(serde_json::Value::String(s)) => Some(s.clone()),
    _ => None,
  };
  Ok(provider::Tool {
    name: req_str(obj, "name")?,
    description: opt_str(obj, "description")?,
    input_schema,
    output_schema,
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

fn role_to_json(role: &provider::Role) -> &'static str {
  match role {
    provider::Role::System => "system",
    provider::Role::User => "user",
    provider::Role::Assistant => "assistant",
    provider::Role::Tool => "tool",
  }
}

fn tool_call_to_json(tc: types::ToolCall) -> serde_json::Value {
  serde_json::json!({
    "id": tc.id,
    "name": tc.name,
    "arguments": tc.arguments,
  })
}

fn chat_message_to_json(m: types::ChatMessage) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert(
    "role".to_string(),
    serde_json::Value::String(role_to_json(&m.role).to_string()),
  );
  if let Some(content) = m.content {
    o.insert("content".to_string(), serde_json::Value::String(content));
  }
  if let Some(reasoning) = m.reasoning {
    o.insert(
      "reasoning".to_string(),
      serde_json::Value::String(reasoning),
    );
  }
  if let Some(tc) = m.tool_call {
    o.insert("tool_call".to_string(), tool_call_to_json(tc));
  }
  serde_json::Value::Object(o)
}

fn tool_to_json(t: types::Tool) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("name".to_string(), serde_json::Value::String(t.name));
  if let Some(description) = t.description {
    o.insert(
      "description".to_string(),
      serde_json::Value::String(description),
    );
  }
  o.insert(
    "input_schema".to_string(),
    serde_json::Value::String(t.input_schema),
  );
  if let Some(output_schema) = t.output_schema {
    o.insert(
      "output_schema".to_string(),
      serde_json::Value::String(output_schema),
    );
  }
  serde_json::Value::Object(o)
}

fn usage_to_json(u: types::Usage) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  if let Some(prompt) = u.prompt_tokens {
    o.insert("prompt_tokens".to_string(), serde_json::Value::from(prompt));
  }
  if let Some(completion) = u.completion_tokens {
    o.insert(
      "completion_tokens".to_string(),
      serde_json::Value::from(completion),
    );
  }
  if let Some(total) = u.total_tokens {
    o.insert("total_tokens".to_string(), serde_json::Value::from(total));
  }
  serde_json::Value::Object(o)
}

fn usage_from_json(v: &serde_json::Value) -> Result<types::Usage, JsError> {
  let obj = v
    .as_object()
    .ok_or_else(|| js_err("usage must be an object"))?;
  fn token(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
  ) -> Result<Option<u64>, JsError> {
    match obj.get(key) {
      None | Some(serde_json::Value::Null) => Ok(None),
      Some(value) => value
        .as_u64()
        .map(Some)
        .ok_or_else(|| js_err(format!("{key} must be a non-negative integer"))),
    }
  }
  Ok(types::Usage {
    prompt_tokens: token(obj, "prompt_tokens")?,
    completion_tokens: token(obj, "completion_tokens")?,
    total_tokens: token(obj, "total_tokens")?,
  })
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
  if let Some(tc) = delta.tool_call {
    o.insert("tool_call".to_string(), tool_call_to_json(tc));
  }
  if let Some(finish_reason) = delta.finish_reason {
    o.insert(
      "finish_reason".to_string(),
      serde_json::Value::String(finish_reason),
    );
  }
  if let Some(usage) = delta.usage {
    o.insert("usage".to_string(), usage_to_json(usage));
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
    tool_call: match obj.get("tool_call") {
      None | Some(serde_json::Value::Null) => None,
      Some(tc) => Some(tool_call_from_json(tc)?),
    },
    finish_reason: opt_str(obj, "finish_reason")?,
    usage: match obj.get("usage") {
      None | Some(serde_json::Value::Null) => None,
      Some(usage) => Some(usage_from_json(usage)?),
    },
  })
}

fn tool_result_to_json(r: types::ToolResult) -> serde_json::Value {
  serde_json::json!({
    "name": r.name,
    "arguments": r.arguments,
    "value": r.value,
  })
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
    "tool_calls".to_string(),
    serde_json::Value::Array(
      r.tool_calls.into_iter().map(tool_call_to_json).collect(),
    ),
  );
  if let Some(finish_reason) = r.finish_reason {
    o.insert(
      "finish_reason".to_string(),
      serde_json::Value::String(finish_reason),
    );
  }
  if let Some(usage) = r.usage {
    o.insert("usage".to_string(), usage_to_json(usage));
  }
  serde_json::Value::Object(o)
}

fn resource_to_json(r: types::ResourceInfo) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("uri".to_string(), serde_json::Value::String(r.uri));
  o.insert("name".to_string(), serde_json::Value::String(r.name));
  if let Some(description) = r.description {
    o.insert(
      "description".to_string(),
      serde_json::Value::String(description),
    );
  }
  if let Some(mime_type) = r.mime_type {
    o.insert(
      "mime_type".to_string(),
      serde_json::Value::String(mime_type),
    );
  }
  serde_json::Value::Object(o)
}

fn resource_content_to_json(c: types::ResourceContent) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("uri".to_string(), serde_json::Value::String(c.uri));
  if let Some(mime_type) = c.mime_type {
    o.insert(
      "mime_type".to_string(),
      serde_json::Value::String(mime_type),
    );
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
    Event::ResourceUpdated(content) => {
      ("resource-updated", resource_content_to_json(content))
    }
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
        o.insert("params".to_string(), serde_json::Value::String(params));
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
