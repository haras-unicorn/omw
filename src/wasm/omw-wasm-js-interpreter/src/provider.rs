use boa_engine::{
  Context, JsArgs as _, JsNativeError, JsResult, JsValue, NativeFunction,
  js_string,
};

use crate::convert::{
  chat_result_to_json, handle_name, js_err, messages_from_js, str_arg,
  tools_from_js, value_from_json,
};

pub(crate) fn provider_get(
  _this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = str_arg(args, 0, "name")?;
  crate::omw::omw::provider::get(&name)
    .map_err(|e| JsNativeError::typ().with_message(e))?;
  provider_handle(&name, ctx)
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

pub(crate) fn provider_handle(
  name: &str,
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let json = serde_json::json!({ "name": name });
  let handle = value_from_json(&json, ctx)?;
  let obj = handle
    .as_object()
    .ok_or_else(|| js_err("failed to build provider handle"))?;
  method(&obj, ctx, "chat", provider_chat)?;
  method(&obj, ctx, "chatStream", provider_chat_stream)?;
  method(&obj, ctx, "isOpen", provider_is_open)?;
  method(&obj, ctx, "cancel", provider_cancel)?;
  method(&obj, ctx, "listModels", provider_list_models)?;
  method(&obj, ctx, "kind", provider_kind)?;
  Ok(handle)
}

fn provider_chat(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let p = crate::omw::omw::provider::get(&name).map_err(js_err)?;
  let model = str_arg(args, 0, "model")?;
  let messages = messages_from_js(args.get_or_undefined(1), ctx)?;
  let tools = tools_from_js(args.get_or_undefined(2), ctx)?;
  let result = p.chat(&model, &messages, &tools).map_err(js_err)?;
  value_from_json(&chat_result_to_json(result), ctx)
}

fn provider_chat_stream(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let p = crate::omw::omw::provider::get(&name).map_err(js_err)?;
  let model = str_arg(args, 0, "model")?;
  let messages = messages_from_js(args.get_or_undefined(1), ctx)?;
  let tools = tools_from_js(args.get_or_undefined(2), ctx)?;
  let id = p.chat_stream(&model, &messages, &tools).map_err(js_err)?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

fn provider_is_open(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let p = crate::omw::omw::provider::get(&name).map_err(js_err)?;
  let uuid = str_arg(args, 0, "uuid")?;
  Ok(JsValue::from(p.is_open(&uuid)))
}

fn provider_cancel(
  this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let p = crate::omw::omw::provider::get(&name).map_err(js_err)?;
  let uuid = str_arg(args, 0, "uuid")?;
  p.cancel(&uuid);
  Ok(JsValue::undefined())
}

fn provider_list_models(
  this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let p = crate::omw::omw::provider::get(&name).map_err(js_err)?;
  let models = p.list_models();
  let json = serde_json::Value::Array(
    models.into_iter().map(serde_json::Value::String).collect(),
  );
  value_from_json(&json, ctx)
}

fn provider_kind(
  this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let name = handle_name(this, ctx)?;
  let p = crate::omw::omw::provider::get(&name).map_err(js_err)?;
  Ok(JsValue::from(js_string!(p.kind().as_str())))
}
