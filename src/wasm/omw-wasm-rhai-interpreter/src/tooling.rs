use rhai_rt::{Array, EvalAltResult, Map};

use crate::{
  convert::{handle_name, json_from_map, method, to_error},
  omw::omw::{tooling, types},
};

/// Look up a tooling by name and return an object map with its methods.
pub(crate) fn tooling_get(name: &str) -> Result<Map, Box<EvalAltResult>> {
  tooling::get(name).map_err(to_error)?;
  let mut m = Map::new();
  m.insert("name".into(), name.into());
  m.insert("list_tools".into(), method("tooling_list_tools")?.into());
  m.insert("call_tool".into(), method("tooling_call_tool")?.into());
  m.insert(
    "call_tool_blocking".into(),
    method("tooling_call_tool_blocking")?.into(),
  );
  m.insert("is_open".into(), method("tooling_is_open")?.into());
  m.insert("cancel".into(), method("tooling_cancel")?.into());
  m.insert("kind".into(), method("tooling_kind")?.into());
  m.insert(
    "list_resources".into(),
    method("tooling_list_resources")?.into(),
  );
  m.insert(
    "read_resource".into(),
    method("tooling_read_resource")?.into(),
  );
  m.insert(
    "subscribe_resource_list".into(),
    method("tooling_subscribe_resource_list")?.into(),
  );
  m.insert(
    "subscribe_resource".into(),
    method("tooling_subscribe_resource")?.into(),
  );
  Ok(m)
}

pub(crate) fn tooling_list_tools(
  handle: Map,
) -> Result<Array, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let tools = tooling::get(&name)
    .map_err(to_error)?
    .list_tools()
    .map_err(to_error)?;
  let mut arr = Array::new();
  for t in tools {
    let mut m = Map::new();
    m.insert("name".into(), t.name.into());
    if let Some(desc) = t.description {
      m.insert("description".into(), desc.into());
    }
    m.insert("input_schema".into(), t.input_schema.into());
    arr.push(m.into());
  }
  Ok(arr)
}

pub(crate) fn tooling_call_tool(
  handle: Map,
  tool: &str,
  args: Map,
) -> Result<String, Box<EvalAltResult>> {
  let instance = handle_name(&handle)?;
  let t = tooling::get(&instance).map_err(to_error)?;
  let arguments = json_from_map(&args).map_err(to_error)?;
  t.call_tool(tool, &arguments).map_err(to_error)
}

pub(crate) fn tooling_call_tool_blocking(
  handle: Map,
  tool: &str,
  args: Map,
) -> Result<Map, Box<EvalAltResult>> {
  let instance = handle_name(&handle)?;
  let t = tooling::get(&instance).map_err(to_error)?;
  let arguments = json_from_map(&args).map_err(to_error)?;
  let result = t.call_tool_blocking(tool, &arguments).map_err(to_error)?;
  Ok(tool_result_to_map(result))
}

pub(crate) fn tooling_is_open(
  handle: Map,
  uuid: &str,
) -> Result<bool, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  Ok(tooling::get(&name).map_err(to_error)?.is_open(uuid))
}

pub(crate) fn tooling_cancel(
  handle: Map,
  uuid: &str,
) -> Result<(), Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  tooling::get(&name).map_err(to_error)?.cancel(uuid);
  Ok(())
}

pub(crate) fn tooling_kind(handle: Map) -> Result<String, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  Ok(tooling::get(&name).map_err(to_error)?.kind())
}

pub(crate) fn tooling_list_resources(
  handle: Map,
) -> Result<Array, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let resources = tooling::get(&name)
    .map_err(to_error)?
    .list_resources()
    .map_err(to_error)?;
  let mut arr = Array::new();
  for r in resources {
    arr.push(resource_to_map(r).into());
  }
  Ok(arr)
}

pub(crate) fn tooling_read_resource(
  handle: Map,
  uri: &str,
) -> Result<Map, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let content = tooling::get(&name)
    .map_err(to_error)?
    .read_resource(uri)
    .map_err(to_error)?;
  Ok(resource_content_to_map(content))
}

/// Map one `types::ResourceInfo` into a rhai map so scripts can read `uri`,
/// `name`, `description`, and `mime_type` off a resource.
pub(crate) fn resource_to_map(r: types::ResourceInfo) -> Map {
  let mut m = Map::new();
  m.insert("uri".into(), r.uri.into());
  m.insert("name".into(), r.name.into());
  if let Some(desc) = r.description {
    m.insert("description".into(), desc.into());
  }
  if let Some(mime) = r.mime_type {
    m.insert("mime_type".into(), mime.into());
  }
  m
}

/// Map one `types::ToolResult` into a rhai map so scripts can read `name`,
/// `arguments` and `value` off a tool-call result.
pub(crate) fn tool_result_to_map(r: types::ToolResult) -> Map {
  let mut m = Map::new();
  m.insert("name".into(), r.name.into());
  m.insert("arguments".into(), r.arguments.into());
  m.insert("value".into(), r.value.into());
  m
}

/// Map a `types::ChatResult` into a rhai map so scripts can read `content`,
/// `tool_calls`, and `finish_reason` off a blocking `chat` result.
pub(crate) fn chat_result_to_map(r: types::ChatResult) -> Map {
  let mut m = Map::new();
  if let Some(content) = r.content {
    m.insert("content".into(), content.into());
  }
  let mut calls = Array::new();
  for tc in r.tool_calls {
    let mut t = Map::new();
    t.insert("id".into(), tc.id.into());
    t.insert("name".into(), tc.name.into());
    t.insert("arguments".into(), tc.arguments.into());
    calls.push(t.into());
  }
  m.insert("tool_calls".into(), calls.into());
  if let Some(finish_reason) = r.finish_reason {
    m.insert("finish_reason".into(), finish_reason.into());
  }
  m
}

/// Map one `types::ResourceContent` into a rhai map so scripts can read `uri`,
/// `mime_type` and `content` off a `resource-updated` event.
pub(crate) fn resource_content_to_map(c: types::ResourceContent) -> Map {
  let mut m = Map::new();
  m.insert("uri".into(), c.uri.into());
  if let Some(mime) = c.mime_type {
    m.insert("mime_type".into(), mime.into());
  }
  m.insert("content".into(), c.content.into());
  m
}

pub(crate) fn tooling_subscribe_resource_list(
  handle: Map,
) -> Result<String, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  tooling::get(&name)
    .map_err(to_error)?
    .subscribe_resource_list()
    .map_err(to_error)
}

pub(crate) fn tooling_subscribe_resource(
  handle: Map,
  uri: &str,
) -> Result<String, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  tooling::get(&name)
    .map_err(to_error)?
    .subscribe_resource(uri)
    .map_err(to_error)
}

pub(crate) fn tooling_unsubscribe_resource_list(
  handle: Map,
  uuid: &str,
) -> Result<(), Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  tooling::get(&name)
    .map_err(to_error)?
    .unsubscribe_resource_list(uuid);
  Ok(())
}

pub(crate) fn tooling_unsubscribe_resource(
  handle: Map,
  uuid: &str,
) -> Result<(), Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  tooling::get(&name)
    .map_err(to_error)?
    .unsubscribe_resource(uuid);
  Ok(())
}
