use boa_engine::{
  Context, JsNativeError, JsResult, JsValue, NativeFunction, js_string,
};

use crate::convert::{
  handle_name, js_err, json_string_from_js, str_arg, value_from_json,
};

pub(crate) fn tooling_get(
  _this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = str_arg(args, 0, "name")?;
  crate::omw::omw::tooling::get(&name)
    .map_err(|e| JsNativeError::typ().with_message(e))?;
  tooling_handle(&name, ctx)
}

fn method(
  obj: &boa_engine::JsObject,
  ctx: &mut Context,
  prop: &str,
  f: boa_engine::native_function::NativeFunctionPointer,
) -> JsResult<()> {
  let fun = NativeFunction::from_fn_ptr(f).to_js_function(ctx.realm());
  obj.set(js_string!(prop), fun, false, ctx)?;
  Ok(())
}

fn tooling_handle(name: &str, ctx: &mut Context) -> JsResult<JsValue> {
  let json = serde_json::json!({ "name": name });
  let handle = value_from_json(&json, ctx)?;
  let obj = handle
    .as_object()
    .ok_or_else(|| js_err("failed to build tooling handle"))?;
  method(&obj, ctx, "listTools", tooling_list_tools)?;
  method(&obj, ctx, "callTool", tooling_call_tool)?;
  method(&obj, ctx, "callToolBlocking", tooling_call_tool_blocking)?;
  method(&obj, ctx, "isOpen", tooling_is_open)?;
  method(&obj, ctx, "cancel", tooling_cancel)?;
  method(&obj, ctx, "kind", tooling_kind)?;
  method(&obj, ctx, "listResources", tooling_list_resources)?;
  method(&obj, ctx, "readResource", tooling_read_resource)?;
  method(
    &obj,
    ctx,
    "subscribeResourceList",
    tooling_subscribe_resource_list,
  )?;
  method(&obj, ctx, "subscribeResource", tooling_subscribe_resource)?;
  method(
    &obj,
    ctx,
    "unsubscribeResourceList",
    tooling_unsubscribe_resource_list,
  )?;
  method(
    &obj,
    ctx,
    "unsubscribeResource",
    tooling_unsubscribe_resource,
  )?;
  Ok(handle)
}

fn tool_to_json(t: crate::omw::omw::types::Tool) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("name".to_string(), serde_json::Value::String(t.name));
  if let Some(description) = t.description {
    o.insert(
      "description".to_string(),
      serde_json::Value::String(description),
    );
  }
  o.insert(
    "inputSchema".to_string(),
    serde_json::Value::String(t.input_schema.clone()),
  );
  o.insert(
    "input_schema".to_string(),
    serde_json::Value::String(t.input_schema),
  );
  serde_json::Value::Object(o)
}

fn tooling_list_tools(
  this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let tools = crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .list_tools()
    .map_err(js_err)?;
  let json =
    serde_json::Value::Array(tools.into_iter().map(tool_to_json).collect());
  value_from_json(&json, ctx)
}

fn tooling_call_tool(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  use boa_engine::JsArgs as _;
  let name = handle_name(this, ctx)?;
  let t = crate::omw::omw::tooling::get(&name).map_err(js_err)?;
  let tool = str_arg(args, 0, "tool")?;
  let arguments = json_string_from_js(args.get_or_undefined(1), ctx)?;
  let id = t.call_tool(&tool, &arguments).map_err(js_err)?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

fn tool_result_to_json(
  r: crate::omw::omw::types::ToolResult,
) -> serde_json::Value {
  serde_json::json!({
    "name": r.name,
    "arguments": r.arguments,
    "value": r.value,
  })
}

fn tooling_call_tool_blocking(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  use boa_engine::JsArgs as _;
  let name = handle_name(this, ctx)?;
  let t = crate::omw::omw::tooling::get(&name).map_err(js_err)?;
  let tool = str_arg(args, 0, "tool")?;
  let arguments = json_string_from_js(args.get_or_undefined(1), ctx)?;
  let result = t.call_tool_blocking(&tool, &arguments).map_err(js_err)?;
  value_from_json(&tool_result_to_json(result), ctx)
}

fn tooling_is_open(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let uuid = str_arg(args, 0, "uuid")?;
  Ok(JsValue::from(
    crate::omw::omw::tooling::get(&name)
      .map_err(js_err)?
      .is_open(&uuid),
  ))
}

fn tooling_cancel(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let uuid = str_arg(args, 0, "uuid")?;
  crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .cancel(&uuid);
  Ok(JsValue::undefined())
}

fn tooling_kind(
  this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let kind = crate::omw::omw::tooling::get(&name).map_err(js_err)?.kind();
  Ok(JsValue::from(js_string!(kind.as_str())))
}

fn resource_to_json(
  r: crate::omw::omw::types::ResourceInfo,
) -> serde_json::Value {
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
      "mimeType".to_string(),
      serde_json::Value::String(mime_type.clone()),
    );
    o.insert(
      "mime_type".to_string(),
      serde_json::Value::String(mime_type),
    );
  }
  serde_json::Value::Object(o)
}

fn resource_content_to_json(
  c: crate::omw::omw::types::ResourceContent,
) -> serde_json::Value {
  let mut o = serde_json::Map::new();
  o.insert("uri".to_string(), serde_json::Value::String(c.uri));
  if let Some(mime_type) = c.mime_type {
    o.insert(
      "mimeType".to_string(),
      serde_json::Value::String(mime_type.clone()),
    );
    o.insert(
      "mime_type".to_string(),
      serde_json::Value::String(mime_type),
    );
  }
  o.insert("content".to_string(), serde_json::Value::String(c.content));
  serde_json::Value::Object(o)
}

fn tooling_list_resources(
  this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let resources = crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .list_resources()
    .map_err(js_err)?;
  let json = serde_json::Value::Array(
    resources.into_iter().map(resource_to_json).collect(),
  );
  value_from_json(&json, ctx)
}

fn tooling_read_resource(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let uri = str_arg(args, 0, "uri")?;
  let content = crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .read_resource(&uri)
    .map_err(js_err)?;
  value_from_json(&resource_content_to_json(content), ctx)
}

fn tooling_subscribe_resource_list(
  this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let id = crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .subscribe_resource_list()
    .map_err(js_err)?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

fn tooling_subscribe_resource(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let uri = str_arg(args, 0, "uri")?;
  let id = crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .subscribe_resource(&uri)
    .map_err(js_err)?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

fn tooling_unsubscribe_resource_list(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let uuid = str_arg(args, 0, "uuid")?;
  crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .unsubscribe_resource_list(&uuid);
  Ok(JsValue::undefined())
}

fn tooling_unsubscribe_resource(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let uuid = str_arg(args, 0, "uuid")?;
  crate::omw::omw::tooling::get(&name)
    .map_err(js_err)?
    .unsubscribe_resource(&uuid);
  Ok(JsValue::undefined())
}
