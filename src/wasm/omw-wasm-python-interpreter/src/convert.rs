//! Conversions between the WIT records (opaque JSON strings, typed records)
//! and Python values. Records surface as `types.SimpleNamespace` instances so
//! scripts read them attribute-style (`ev.kind`, `ev.payload.content`), and
//! opaque JSON strings are parsed in place into nested namespaces/lists.

use rustpython_vm::builtins::{PyNone, PyStr, PyStrRef};
use rustpython_vm::function::FuncArgs;
use rustpython_vm::{PyObjectRef, PyResult, VirtualMachine};

use crate::omw::omw::{provider, types};

/// A fresh `types.SimpleNamespace` instance.
pub(crate) fn namespace(vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  let types = vm.import("types", 0)?;
  let namespace = types.get_attr("SimpleNamespace", vm)?;
  namespace.call((), vm)
}

/// Render a `PyStrRef` as an owned Rust string, lossily.
pub(crate) fn str_of(value: &PyStrRef) -> String {
  value.to_string_lossy().into_owned()
}

/// Parse an opaque JSON string into a JSON value, falling back to the raw
/// string when it is not valid JSON.
pub(crate) fn json_or_string(raw: &str) -> serde_json::Value {
  serde_json::from_str(raw)
    .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

/// Convert a JSON value into a Python value: objects become namespaces (for
/// attribute access), arrays become lists, and scalars map to their Python
/// counterparts.
pub(crate) fn json_to_py(
  vm: &VirtualMachine,
  value: &serde_json::Value,
) -> PyResult<PyObjectRef> {
  Ok(match value {
    serde_json::Value::Null => vm.ctx.none(),
    serde_json::Value::Bool(flag) => vm.new_pyobj(*flag),
    serde_json::Value::Number(number) => {
      if let Some(int) = number.as_i64() {
        vm.new_pyobj(int)
      } else if let Some(uint) = number.as_u64() {
        vm.new_pyobj(uint)
      } else {
        vm.new_pyobj(number.as_f64().unwrap_or(0.0))
      }
    }
    serde_json::Value::String(text) => vm.new_pyobj(text.as_str()),
    serde_json::Value::Array(items) => {
      let values: PyResult<Vec<PyObjectRef>> =
        items.iter().map(|item| json_to_py(vm, item)).collect();
      vm.ctx.new_list(values?).into()
    }
    serde_json::Value::Object(map) => {
      let object = namespace(vm)?;
      for (key, item) in map {
        object.set_attr(
          &vm.ctx.new_str(key.as_str()),
          json_to_py(vm, item)?,
          vm,
        )?;
      }
      object
    }
  })
}

/// Serialize any Python value to a JSON value via the stdlib `json.dumps`,
/// so scripts pass plain dicts/lists/scalars. Records (the `SimpleNamespace`
/// values this module hands out) are serialized through their `__dict__` via
/// `default=vars`, so an event or tool call round-trips back into a call.
pub(crate) fn py_to_json(
  vm: &VirtualMachine,
  value: &PyObjectRef,
) -> PyResult<serde_json::Value> {
  let json = vm.import("json", 0)?;
  let dumps = json.get_attr("dumps", vm)?;
  let default = vm.builtins.get_attr("vars", vm)?;
  let func_args = FuncArgs::with_kwargs_names(
    [value.clone(), default].into_iter(),
    ["default".to_string()].into_iter(),
  );
  let text = dumps.call(func_args, vm)?;
  let text = text
    .downcast_ref::<PyStr>()
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_default();
  serde_json::from_str(&text).map_err(|e| vm.new_value_error(e.to_string()))
}

/// Serialize any Python value to a compact JSON string.
pub(crate) fn json_string_of(
  vm: &VirtualMachine,
  value: &PyObjectRef,
) -> PyResult<String> {
  serde_json::to_string(&py_to_json(vm, value)?)
    .map_err(|e| vm.new_value_error(e.to_string()))
}

fn opt_str(
  object: &serde_json::Map<String, serde_json::Value>,
  key: &str,
) -> Option<String> {
  match object.get(key) {
    Some(serde_json::Value::String(text)) => Some(text.clone()),
    _ => None,
  }
}

fn req_str(
  vm: &VirtualMachine,
  object: &serde_json::Map<String, serde_json::Value>,
  key: &str,
) -> PyResult<String> {
  opt_str(object, key)
    .ok_or_else(|| vm.new_type_error(format!("{key} must be a string")))
}

fn opaque_json(value: &serde_json::Value) -> String {
  match value {
    serde_json::Value::String(text) => text.clone(),
    other => {
      serde_json::to_string(other).unwrap_or_else(|_| "null".to_string())
    }
  }
}

fn tool_call_from_json(
  vm: &VirtualMachine,
  value: &serde_json::Value,
) -> PyResult<types::ToolCall> {
  let object = value
    .as_object()
    .ok_or_else(|| vm.new_type_error("toolCall must be an object"))?;
  let arguments = object
    .get("arguments")
    .ok_or_else(|| vm.new_type_error("toolCall missing arguments"))?;
  Ok(types::ToolCall {
    id: req_str(vm, object, "id")?,
    name: req_str(vm, object, "name")?,
    arguments: opaque_json(arguments),
  })
}

fn msg_from_json(
  vm: &VirtualMachine,
  value: &serde_json::Value,
) -> PyResult<provider::ChatMessage> {
  let object = value
    .as_object()
    .ok_or_else(|| vm.new_type_error("chat message must be an object"))?;
  let role = match object.get("role").and_then(|role| role.as_str()) {
    Some("system") => provider::Role::System,
    Some("assistant") => provider::Role::Assistant,
    Some("tool") => provider::Role::Tool,
    _ => provider::Role::User,
  };
  Ok(provider::ChatMessage {
    role,
    content: opt_str(object, "content"),
    reasoning: opt_str(object, "reasoning"),
    tool_call: match object.get("tool_call") {
      None | Some(serde_json::Value::Null) => None,
      Some(value) => Some(tool_call_from_json(vm, value)?),
    },
  })
}

fn tool_from_json(
  vm: &VirtualMachine,
  value: &serde_json::Value,
) -> PyResult<provider::Tool> {
  let object = value
    .as_object()
    .ok_or_else(|| vm.new_type_error("tool must be an object"))?;
  let input_schema = match object.get("input_schema") {
    Some(value) => opaque_json(value),
    None => "{}".to_string(),
  };
  let output_schema = match object.get("output_schema") {
    None | Some(serde_json::Value::Null) => None,
    Some(value) => Some(opaque_json(value)),
  };
  Ok(provider::Tool {
    name: req_str(vm, object, "name")?,
    description: opt_str(object, "description"),
    input_schema,
    output_schema,
  })
}

fn array_of<'a>(
  vm: &VirtualMachine,
  value: &'a serde_json::Value,
  what: &str,
) -> PyResult<&'a Vec<serde_json::Value>> {
  value
    .as_array()
    .ok_or_else(|| vm.new_type_error(format!("{what} must be an array")))
}

/// Parse a Python list of message dicts into WIT chat messages.
pub(crate) fn messages_from_py(
  vm: &VirtualMachine,
  value: &PyObjectRef,
) -> PyResult<Vec<provider::ChatMessage>> {
  let json = py_to_json(vm, value)?;
  array_of(vm, &json, "messages")?
    .iter()
    .map(|item| msg_from_json(vm, item))
    .collect()
}

/// Parse a Python list of tool dicts into WIT tools.
pub(crate) fn tools_from_py(
  vm: &VirtualMachine,
  value: &PyObjectRef,
) -> PyResult<Vec<provider::Tool>> {
  let json = py_to_json(vm, value)?;
  array_of(vm, &json, "tools")?
    .iter()
    .map(|item| tool_from_json(vm, item))
    .collect()
}

/// Serialize optional Python params into the opaque JSON string the host
/// expects. A string is passed through; anything else is serialized.
pub(crate) fn params_from_py(
  vm: &VirtualMachine,
  value: Option<&PyObjectRef>,
) -> PyResult<Option<String>> {
  let Some(value) = value else {
    return Ok(None);
  };
  if value.downcast_ref::<PyNone>().is_some() {
    return Ok(None);
  }
  if let Some(text) = value.downcast_ref::<PyStr>() {
    let text = text.to_string_lossy();
    return Ok(if text.trim().is_empty() {
      None
    } else {
      Some(text.into_owned())
    });
  }
  serde_json::to_string(&py_to_json(vm, value)?)
    .map(Some)
    .map_err(|e| vm.new_value_error(e.to_string()))
}

fn usage_from_json(
  vm: &VirtualMachine,
  value: &serde_json::Value,
) -> PyResult<types::Usage> {
  let object = value
    .as_object()
    .ok_or_else(|| vm.new_type_error("usage must be an object"))?;
  fn token(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
  ) -> Option<u64> {
    object.get(key).and_then(|value| value.as_u64())
  }
  Ok(types::Usage {
    prompt_tokens: token(object, "prompt_tokens"),
    completion_tokens: token(object, "completion_tokens"),
    total_tokens: token(object, "total_tokens"),
  })
}

/// Parse a Python dict into a WIT chat delta.
pub(crate) fn delta_from_py(
  vm: &VirtualMachine,
  value: &PyObjectRef,
) -> PyResult<types::ChatDelta> {
  let json = py_to_json(vm, value)?;
  let object = json
    .as_object()
    .ok_or_else(|| vm.new_type_error("delta must be an object"))?;
  Ok(types::ChatDelta {
    content: opt_str(object, "content"),
    reasoning: opt_str(object, "reasoning"),
    tool_call: match object.get("tool_call") {
      None | Some(serde_json::Value::Null) => None,
      Some(value) => Some(tool_call_from_json(vm, value)?),
    },
    finish_reason: opt_str(object, "finish_reason"),
    usage: match object.get("usage") {
      None | Some(serde_json::Value::Null) => None,
      Some(value) => Some(usage_from_json(vm, value)?),
    },
  })
}

fn role_to_str(role: &provider::Role) -> &'static str {
  match role {
    provider::Role::System => "system",
    provider::Role::User => "user",
    provider::Role::Assistant => "assistant",
    provider::Role::Tool => "tool",
  }
}

fn tool_call_to_json(call: types::ToolCall) -> serde_json::Value {
  serde_json::json!({
    "id": call.id,
    "name": call.name,
    "arguments": json_or_string(&call.arguments),
  })
}

fn chat_message_to_json(message: types::ChatMessage) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  object.insert(
    "role".to_string(),
    serde_json::Value::String(role_to_str(&message.role).to_string()),
  );
  if let Some(content) = message.content {
    object.insert("content".to_string(), serde_json::Value::String(content));
  }
  if let Some(reasoning) = message.reasoning {
    object.insert(
      "reasoning".to_string(),
      serde_json::Value::String(reasoning),
    );
  }
  if let Some(call) = message.tool_call {
    object.insert("tool_call".to_string(), tool_call_to_json(call));
  }
  serde_json::Value::Object(object)
}

fn tool_to_json(tool: types::Tool) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  object.insert("name".to_string(), serde_json::Value::String(tool.name));
  if let Some(description) = tool.description {
    object.insert(
      "description".to_string(),
      serde_json::Value::String(description),
    );
  }
  object.insert(
    "input_schema".to_string(),
    json_or_string(&tool.input_schema),
  );
  if let Some(output_schema) = tool.output_schema {
    object.insert("output_schema".to_string(), json_or_string(&output_schema));
  }
  serde_json::Value::Object(object)
}

fn usage_to_json(usage: types::Usage) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  if let Some(prompt) = usage.prompt_tokens {
    object.insert("prompt_tokens".to_string(), serde_json::Value::from(prompt));
  }
  if let Some(completion) = usage.completion_tokens {
    object.insert(
      "completion_tokens".to_string(),
      serde_json::Value::from(completion),
    );
  }
  if let Some(total) = usage.total_tokens {
    object.insert("total_tokens".to_string(), serde_json::Value::from(total));
  }
  serde_json::Value::Object(object)
}

fn delta_to_json(delta: types::ChatDelta) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  if let Some(content) = delta.content {
    object.insert("content".to_string(), serde_json::Value::String(content));
  }
  if let Some(reasoning) = delta.reasoning {
    object.insert(
      "reasoning".to_string(),
      serde_json::Value::String(reasoning),
    );
  }
  if let Some(call) = delta.tool_call {
    object.insert("tool_call".to_string(), tool_call_to_json(call));
  }
  if let Some(finish_reason) = delta.finish_reason {
    object.insert(
      "finish_reason".to_string(),
      serde_json::Value::String(finish_reason),
    );
  }
  if let Some(usage) = delta.usage {
    object.insert("usage".to_string(), usage_to_json(usage));
  }
  serde_json::Value::Object(object)
}

fn tool_result_to_json(result: types::ToolResult) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  object.insert("name".to_string(), serde_json::Value::String(result.name));
  object.insert("arguments".to_string(), json_or_string(&result.arguments));
  object.insert("content".to_string(), json_or_string(&result.content));
  if let Some(structured) = result.structured_content {
    object.insert(
      "structured_content".to_string(),
      json_or_string(&structured),
    );
  }
  serde_json::Value::Object(object)
}

fn resource_to_json(resource: types::ResourceInfo) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  object.insert("uri".to_string(), serde_json::Value::String(resource.uri));
  object.insert("name".to_string(), serde_json::Value::String(resource.name));
  if let Some(description) = resource.description {
    object.insert(
      "description".to_string(),
      serde_json::Value::String(description),
    );
  }
  if let Some(mime_type) = resource.mime_type {
    object.insert(
      "mime_type".to_string(),
      serde_json::Value::String(mime_type),
    );
  }
  serde_json::Value::Object(object)
}

fn resource_content_to_json(
  content: types::ResourceContent,
) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  object.insert("uri".to_string(), serde_json::Value::String(content.uri));
  if let Some(mime_type) = content.mime_type {
    object.insert(
      "mime_type".to_string(),
      serde_json::Value::String(mime_type),
    );
  }
  object.insert(
    "content".to_string(),
    serde_json::Value::String(content.content),
  );
  serde_json::Value::Object(object)
}

/// Convert a chat result into a namespace.
pub(crate) fn chat_result_to_py(
  vm: &VirtualMachine,
  result: types::ChatResult,
) -> PyResult<PyObjectRef> {
  let mut object = serde_json::Map::new();
  if let Some(content) = result.content {
    object.insert("content".to_string(), serde_json::Value::String(content));
  }
  if let Some(reasoning) = result.reasoning {
    object.insert(
      "reasoning".to_string(),
      serde_json::Value::String(reasoning),
    );
  }
  object.insert(
    "tool_calls".to_string(),
    serde_json::Value::Array(
      result
        .tool_calls
        .into_iter()
        .map(tool_call_to_json)
        .collect(),
    ),
  );
  if let Some(finish_reason) = result.finish_reason {
    object.insert(
      "finish_reason".to_string(),
      serde_json::Value::String(finish_reason),
    );
  }
  if let Some(usage) = result.usage {
    object.insert("usage".to_string(), usage_to_json(usage));
  }
  json_to_py(vm, &serde_json::Value::Object(object))
}

/// Convert a tool result into a namespace.
pub(crate) fn tool_result_to_py(
  vm: &VirtualMachine,
  result: types::ToolResult,
) -> PyResult<PyObjectRef> {
  json_to_py(vm, &tool_result_to_json(result))
}

/// Convert a tool into a namespace.
pub(crate) fn tool_to_py(
  vm: &VirtualMachine,
  tool: types::Tool,
) -> PyResult<PyObjectRef> {
  json_to_py(vm, &tool_to_json(tool))
}

/// Convert a resource info into a namespace.
pub(crate) fn resource_to_py(
  vm: &VirtualMachine,
  resource: types::ResourceInfo,
) -> PyResult<PyObjectRef> {
  json_to_py(vm, &resource_to_json(resource))
}

/// Convert a resource content into a namespace.
pub(crate) fn resource_content_to_py(
  vm: &VirtualMachine,
  content: types::ResourceContent,
) -> PyResult<PyObjectRef> {
  json_to_py(vm, &resource_content_to_json(content))
}

/// Convert an event envelope into a namespace `{ id, kind, payload }`.
pub(crate) fn envelope_to_py(
  vm: &VirtualMachine,
  envelope: types::EventEnvelope,
) -> PyResult<PyObjectRef> {
  let (kind, payload): (&str, serde_json::Value) = match envelope.event {
    types::Event::Message(payload) => {
      ("message", serde_json::Value::String(payload))
    }
    types::Event::Error(message) => {
      ("error", serde_json::Value::String(message))
    }
    types::Event::Timer => ("timer", serde_json::Value::Null),
    types::Event::Reload => ("reload", serde_json::Value::Null),
    types::Event::Shutdown => ("shutdown", serde_json::Value::Null),
    types::Event::ChatDelta(delta) => ("chat-delta", delta_to_json(delta)),
    types::Event::ChatEnd => ("chat-end", serde_json::Value::Null),
    types::Event::ToolResult(result) => {
      ("tool-result", tool_result_to_json(result))
    }
    types::Event::ResourceListUpdated(resources) => (
      "resource-list-updated",
      serde_json::Value::Array(
        resources.into_iter().map(resource_to_json).collect(),
      ),
    ),
    types::Event::ResourceUpdated(content) => {
      ("resource-updated", resource_content_to_json(content))
    }
    types::Event::EndpointMessage(message) => {
      let messages = message.messages.into_iter().map(chat_message_to_json);
      let tools = message.tools.into_iter().map(tool_to_json);
      let mut object = serde_json::Map::new();
      object.insert(
        "session".to_string(),
        serde_json::Value::String(message.session),
      );
      object.insert(
        "messages".to_string(),
        serde_json::Value::Array(messages.collect()),
      );
      object.insert(
        "tools".to_string(),
        serde_json::Value::Array(tools.collect()),
      );
      if let Some(params) = message.params {
        object.insert("params".to_string(), json_or_string(&params));
      }
      ("endpoint-message", serde_json::Value::Object(object))
    }
    types::Event::EndpointSessionEnd(end) => {
      let mut object = serde_json::Map::new();
      object.insert(
        "session".to_string(),
        serde_json::Value::String(end.session),
      );
      if let Some(error) = end.error {
        object.insert("error".to_string(), serde_json::Value::String(error));
      }
      ("endpoint-session-end", serde_json::Value::Object(object))
    }
  };
  let json = serde_json::json!({
    "id": envelope.id,
    "kind": kind,
    "payload": payload,
  });
  json_to_py(vm, &json)
}
