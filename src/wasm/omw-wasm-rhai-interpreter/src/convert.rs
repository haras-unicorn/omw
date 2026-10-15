use rhai_rt::{Array, Dynamic, EvalAltResult, FnPtr, Map, Position};

use crate::omw::omw::{provider, types};

/// A method function pointer stored on a handle map.
pub(crate) fn method(name: &str) -> Result<FnPtr, Box<EvalAltResult>> {
  FnPtr::new(name).map_err(to_error)
}

/// Read the configured name back out of a handle map.
pub(crate) fn handle_name(handle: &Map) -> Result<String, Box<EvalAltResult>> {
  handle
    .get("name")
    .and_then(|n| n.clone().try_cast::<String>())
    .ok_or_else(|| to_error("handle map missing name"))
}

pub(crate) fn msg_from_dynamic(
  d: Dynamic,
) -> Result<provider::ChatMessage, String> {
  let map = d
    .try_cast::<Map>()
    .ok_or_else(|| "chat message must be a map".to_string())?;
  let role = map
    .get("role")
    .and_then(|r| r.clone().try_cast::<String>())
    .unwrap_or_else(|| "user".to_string());
  let content = map
    .get("content")
    .and_then(|c| c.clone().try_cast::<String>());
  let reasoning = map
    .get("reasoning")
    .and_then(|r| r.clone().try_cast::<String>());
  let tool_calls = match map.get("tool_calls") {
    Some(value) => tool_calls_from_dynamic(value.clone())?,
    None => Vec::new(),
  };
  let tool_call_id = map
    .get("tool_call_id")
    .and_then(|v| v.clone().try_cast::<String>());
  Ok(provider::ChatMessage {
    role,
    content,
    reasoning,
    tool_calls,
    tool_call_id,
  })
}

fn tool_calls_from_dynamic(
  value: Dynamic,
) -> Result<Vec<provider::ToolCall>, String> {
  let arr = value
    .try_cast::<Array>()
    .ok_or_else(|| "tool_calls must be an array".to_string())?;
  arr.into_iter().map(tool_call_from_dynamic).collect()
}

fn tool_call_from_dynamic(
  value: Dynamic,
) -> Result<provider::ToolCall, String> {
  let tc = value
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
    .map(arguments_to_json)
    .transpose()?
    .ok_or_else(|| "tool_call missing arguments".to_string())?;
  Ok(provider::ToolCall {
    id,
    name,
    arguments,
  })
}

pub(crate) fn tool_from_dynamic(d: Dynamic) -> Result<provider::Tool, String> {
  let map = d
    .try_cast::<Map>()
    .ok_or_else(|| "tool must be a map".to_string())?;
  let name = map
    .get("name")
    .and_then(|n| n.clone().try_cast::<String>())
    .ok_or_else(|| "tool missing name".to_string())?;
  let title = map
    .get("title")
    .and_then(|t| t.clone().try_cast::<String>());
  let description = map
    .get("description")
    .and_then(|d| d.clone().try_cast::<String>());
  let input_schema = match map.get("input_schema") {
    Some(schema) => arguments_to_json(schema)?,
    None => "{}".to_string(),
  };
  let output_schema = match map.get("output_schema") {
    Some(schema) => Some(arguments_to_json(schema)?),
    None => None,
  };
  let annotations = match map.get("annotations") {
    Some(value) => Some(tool_annotations_from_dynamic(value.clone())?),
    None => None,
  };
  Ok(provider::Tool {
    name,
    title,
    description,
    input_schema,
    output_schema,
    annotations,
  })
}

fn tool_annotations_from_dynamic(
  value: Dynamic,
) -> Result<types::ToolAnnotations, String> {
  let map = value
    .try_cast::<Map>()
    .ok_or_else(|| "tool annotations must be a map".to_string())?;
  let read_only_hint = map
    .get("read_only_hint")
    .and_then(|v| v.clone().try_cast::<bool>());
  let destructive_hint = map
    .get("destructive_hint")
    .and_then(|v| v.clone().try_cast::<bool>());
  let idempotent_hint = map
    .get("idempotent_hint")
    .and_then(|v| v.clone().try_cast::<bool>());
  let open_world_hint = map
    .get("open_world_hint")
    .and_then(|v| v.clone().try_cast::<bool>());
  Ok(types::ToolAnnotations {
    read_only_hint,
    destructive_hint,
    idempotent_hint,
    open_world_hint,
  })
}

pub(crate) fn to_error(e: impl std::fmt::Display) -> Box<EvalAltResult> {
  Box::new(EvalAltResult::ErrorRuntime(
    e.to_string().into(),
    Position::NONE,
  ))
}

/// Parse a JSON string into a rhai value, falling back to the raw string when
/// it is not valid JSON.
pub(crate) fn json_to_dynamic(raw: &str) -> Dynamic {
  match serde_json::from_str::<serde_json::Value>(raw) {
    Ok(json) => rhai_rt::serde::to_dynamic(json).unwrap_or_else(|_| raw.into()),
    Err(_) => raw.into(),
  }
}

/// Coerce a rhai value into the opaque JSON string the host expects: a string
/// is passed through verbatim, anything else is JSON-serialized.
pub(crate) fn arguments_to_json(value: &Dynamic) -> Result<String, String> {
  if let Some(text) = value.clone().try_cast::<String>() {
    return Ok(text);
  }
  dynamic_to_json(value)
}

pub(crate) fn json_from_map(map: &Map) -> Result<String, String> {
  let mut parts = Vec::new();
  for (k, v) in map {
    parts.push(format!(
      "{}:{}",
      json_quote(k.as_str()),
      dynamic_to_json(v)?
    ));
  }
  Ok(format!("{{{}}}", parts.join(",")))
}

/// Serialize a rhai value as a JSON fragment, properly escaping strings.
pub(crate) fn dynamic_to_json(v: &Dynamic) -> Result<String, String> {
  if v.is_unit() {
    Ok("null".to_string())
  } else if let Some(s) = v.clone().try_cast::<String>() {
    Ok(json_quote(&s))
  } else if let Some(i) = v.clone().try_cast::<i64>() {
    Ok(i.to_string())
  } else if let Some(f) = v.clone().try_cast::<f64>() {
    Ok(f.to_string())
  } else if let Some(b) = v.clone().try_cast::<bool>() {
    Ok(b.to_string())
  } else if let Some(arr) = v.clone().try_cast::<Array>() {
    let items = arr
      .iter()
      .map(dynamic_to_json)
      .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("[{}]", items.join(",")))
  } else if let Some(map) = v.clone().try_cast::<Map>() {
    let mut parts = Vec::new();
    for (k, val) in &map {
      parts.push(format!(
        "{}:{}",
        json_quote(k.as_str()),
        dynamic_to_json(val)?
      ));
    }
    Ok(format!("{{{}}}", parts.join(",")))
  } else {
    Ok(json_quote(&v.to_string()))
  }
}

/// Quote a string as a JSON string literal, escaping special characters.
pub(crate) fn json_quote(s: &str) -> String {
  let mut out = String::from("\"");
  for c in s.chars() {
    match c {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
      c => out.push(c),
    }
  }
  out.push('"');
  out
}
