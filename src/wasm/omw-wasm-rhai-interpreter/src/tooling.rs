use rhai_rt::{Array, EvalAltResult, Map};

use crate::{
  convert::{handle_name, json_from_map, json_to_dynamic, method, to_error},
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
    "list_resource_templates".into(),
    method("tooling_list_resource_templates")?.into(),
  );
  m.insert(
    "subscribe_resource_list".into(),
    method("tooling_subscribe_resource_list")?.into(),
  );
  m.insert(
    "subscribe_resource".into(),
    method("tooling_subscribe_resource")?.into(),
  );
  m.insert(
    "unsubscribe_resource_list".into(),
    method("tooling_unsubscribe_resource_list")?.into(),
  );
  m.insert(
    "unsubscribe_resource".into(),
    method("tooling_unsubscribe_resource")?.into(),
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
    arr.push(crate::host::tool_to_map(t).into());
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
) -> Result<Array, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let contents = tooling::get(&name)
    .map_err(to_error)?
    .read_resource(uri)
    .map_err(to_error)?;
  let mut arr = Array::new();
  for c in contents {
    arr.push(resource_content_to_map(c).into());
  }
  Ok(arr)
}

pub(crate) fn tooling_list_resource_templates(
  handle: Map,
) -> Result<Array, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let templates = tooling::get(&name)
    .map_err(to_error)?
    .list_resource_templates()
    .map_err(to_error)?;
  let mut arr = Array::new();
  for t in templates {
    arr.push(resource_template_to_map(t).into());
  }
  Ok(arr)
}

/// Map one `types::ResourceInfo` into a rhai map so scripts can read `uri`,
/// `name`, `title`, `description`, `mime_type`, `size` and `annotations` off
/// a resource.
pub(crate) fn resource_to_map(r: types::ResourceInfo) -> Map {
  let mut m = Map::new();
  m.insert("uri".into(), r.uri.into());
  m.insert("name".into(), r.name.into());
  if let Some(title) = r.title {
    m.insert("title".into(), title.into());
  }
  if let Some(desc) = r.description {
    m.insert("description".into(), desc.into());
  }
  if let Some(mime) = r.mime_type {
    m.insert("mime_type".into(), mime.into());
  }
  if let Some(size) = r.size.and_then(|s| i64::try_from(s).ok()) {
    m.insert("size".into(), size.into());
  }
  if let Some(annotations) = r.annotations {
    m.insert(
      "annotations".into(),
      resource_annotations_to_map(annotations).into(),
    );
  }
  m
}

/// Map `types::ResourceAnnotations` into a rhai map with `audience`,
/// `priority` and `last_modified`.
pub(crate) fn resource_annotations_to_map(
  a: types::ResourceAnnotations,
) -> Map {
  let mut m = Map::new();
  let mut audience = Array::new();
  for role in a.audience {
    audience.push(role.into());
  }
  m.insert("audience".into(), audience.into());
  if let Some(priority) = a.priority {
    m.insert("priority".into(), priority.into());
  }
  if let Some(last_modified) = a.last_modified {
    m.insert("last_modified".into(), last_modified.into());
  }
  m
}

/// Map one `types::ResourceTemplate` into a rhai map so scripts can read
/// `uri_template`, `name`, `title`, `description` and `mime_type` off a
/// resource template.
pub(crate) fn resource_template_to_map(t: types::ResourceTemplate) -> Map {
  let mut m = Map::new();
  m.insert("uri_template".into(), t.uri_template.into());
  m.insert("name".into(), t.name.into());
  if let Some(title) = t.title {
    m.insert("title".into(), title.into());
  }
  if let Some(desc) = t.description {
    m.insert("description".into(), desc.into());
  }
  if let Some(mime) = t.mime_type {
    m.insert("mime_type".into(), mime.into());
  }
  m
}

/// Map one `types::ToolResult` into a rhai map so scripts can read `name`,
/// `arguments`, `content`, `structured_content` and `is_error` off a tool-call
/// result. `arguments`/`content`/`structured_content` are parsed as JSON when
/// possible.
pub(crate) fn tool_result_to_map(r: types::ToolResult) -> Map {
  let mut m = Map::new();
  m.insert("name".into(), r.name.into());
  m.insert("arguments".into(), json_to_dynamic(&r.arguments));
  m.insert("content".into(), json_to_dynamic(&r.content));
  if let Some(structured) = r.structured_content {
    m.insert("structured_content".into(), json_to_dynamic(&structured));
  }
  m.insert("is_error".into(), r.is_error.into());
  m
}

/// Map a `types::ChatResult` into a rhai map so scripts can read `content`,
/// `reasoning`, `tool_calls`, `finish_reason`, and `usage` off a blocking
/// `chat` result.
pub(crate) fn chat_result_to_map(r: types::ChatResult) -> Map {
  let mut m = Map::new();
  if let Some(content) = r.content {
    m.insert("content".into(), content.into());
  }
  if let Some(reasoning) = r.reasoning {
    m.insert("reasoning".into(), reasoning.into());
  }
  let mut calls = Array::new();
  for tc in r.tool_calls {
    calls.push(crate::host::tool_call_to_map(tc).into());
  }
  m.insert("tool_calls".into(), calls.into());
  if let Some(finish_reason) = r.finish_reason {
    m.insert("finish_reason".into(), finish_reason.into());
  }
  if let Some(usage) = r.usage {
    m.insert("usage".into(), json_to_dynamic(&usage));
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
