use boa_engine::{
  Context, JsArgs as _, JsNativeError, JsResult, JsValue, NativeFunction,
  js_string,
};

use crate::convert::{
  bytes_from_js, bytes_to_js, delta_from_js, envelope_to_js, str_arg, u64_arg,
};

pub(crate) fn host_log(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let level = str_arg(args, 0, "level")?;
  let message = str_arg(args, 1, "message")?;
  crate::omw::omw::host::log(&level, &message);
  Ok(JsValue::undefined())
}

pub(crate) fn host_time_now(
  _this: &JsValue,
  _args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  Ok(JsValue::from(crate::omw::omw::host::time_now()))
}

pub(crate) fn host_time_format(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let ts = u64_arg(args, 0, "ts")?;
  let format = str_arg(args, 1, "format")?;
  Ok(JsValue::from(js_string!(
    crate::omw::omw::host::time_format(ts, &format).as_str()
  )))
}

pub(crate) fn host_wait_until(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let ts = u64_arg(args, 0, "ts")?;
  let id = crate::omw::omw::host::wait_until(ts)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

pub(crate) fn host_wait_for(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let ms = u64_arg(args, 0, "ms")?;
  let id = crate::omw::omw::host::wait_for(ms)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

pub(crate) fn host_wait_cron(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let spec = str_arg(args, 0, "spec")?;
  let id = crate::omw::omw::host::wait_cron(&spec)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

pub(crate) fn host_sleep_for(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let ms = u64_arg(args, 0, "ms")?;
  crate::omw::omw::host::sleep_for(ms);
  Ok(JsValue::undefined())
}

pub(crate) fn host_sleep_until(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let ts = u64_arg(args, 0, "ts")?;
  crate::omw::omw::host::sleep_until(ts)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::undefined())
}

pub(crate) fn host_sleep_cron(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let spec = str_arg(args, 0, "spec")?;
  crate::omw::omw::host::sleep_cron(&spec)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::undefined())
}

pub(crate) fn host_cancel_timer(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let uuid = str_arg(args, 0, "uuid")?;
  crate::omw::omw::host::cancel_timer(&uuid);
  Ok(JsValue::undefined())
}

pub(crate) fn host_subscribe_agent(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let agent = str_arg(args, 0, "agent")?;
  let id = crate::omw::omw::host::subscribe_agent(&agent)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

pub(crate) fn host_unsubscribe_agent(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let uuid = str_arg(args, 0, "uuid")?;
  crate::omw::omw::host::unsubscribe_agent(&uuid);
  Ok(JsValue::undefined())
}

pub(crate) fn host_subscribe_lifecycle(
  _this: &JsValue,
  _args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let id = crate::omw::omw::host::subscribe_lifecycle()
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

pub(crate) fn host_unsubscribe_lifecycle(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let uuid = str_arg(args, 0, "uuid")?;
  crate::omw::omw::host::unsubscribe_lifecycle(&uuid);
  Ok(JsValue::undefined())
}

pub(crate) fn host_send_agent(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let agent = str_arg(args, 0, "agent")?;
  let payload = str_arg(args, 1, "payload")?;
  crate::omw::omw::host::send_agent(&agent, &payload);
  Ok(JsValue::undefined())
}

pub(crate) fn host_recv(
  _this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let envelope = crate::omw::omw::host::recv()
    .map_err(|e| JsNativeError::error().with_message(e))?;
  envelope_to_js(envelope, ctx)
}

pub(crate) fn host_try_recv(
  _this: &JsValue,
  _args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let envelope = crate::omw::omw::host::try_recv()
    .map_err(|e| JsNativeError::error().with_message(e))?;
  match envelope {
    Some(envelope) => envelope_to_js(envelope, ctx),
    None => Ok(JsValue::undefined()),
  }
}

pub(crate) fn host_new_uuid(
  _this: &JsValue,
  _args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  Ok(JsValue::from(js_string!(
    crate::omw::omw::host::new_uuid().as_str()
  )))
}

pub(crate) fn host_base64_encode(
  _this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let bytes = bytes_from_js(args.get_or_undefined(0), ctx)?;
  Ok(JsValue::from(js_string!(
    crate::omw::omw::host::base64_encode(&bytes).as_str()
  )))
}

pub(crate) fn host_base64_decode(
  _this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let data = str_arg(args, 0, "data")?;
  let bytes = crate::omw::omw::host::base64_decode(&data)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  bytes_to_js(&bytes, ctx)
}

pub(crate) fn host_memory_get(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let key = str_arg(args, 0, "key")?;
  match crate::omw::omw::host::memory_get(&key) {
    Some(value) => Ok(JsValue::from(js_string!(value.as_str()))),
    None => Ok(JsValue::undefined()),
  }
}

pub(crate) fn host_memory_set(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let key = str_arg(args, 0, "key")?;
  let value = str_arg(args, 1, "value")?;
  crate::omw::omw::host::memory_set(&key, &value);
  Ok(JsValue::undefined())
}

pub(crate) fn host_memory_remove(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let key = str_arg(args, 0, "key")?;
  Ok(JsValue::from(crate::omw::omw::host::memory_remove(&key)))
}

pub(crate) fn host_subscribe_endpoint(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let model = str_arg(args, 0, "model")?;
  let id = crate::omw::omw::host::subscribe_endpoint(&model)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::from(js_string!(id.as_str())))
}

pub(crate) fn host_unsubscribe_endpoint(
  _this: &JsValue,
  args: &[JsValue],
  _: &mut Context,
) -> JsResult<JsValue> {
  let uuid = str_arg(args, 0, "uuid")?;
  crate::omw::omw::host::unsubscribe_endpoint(&uuid);
  Ok(JsValue::undefined())
}

pub(crate) fn host_stream_endpoint(
  _this: &JsValue,
  args: &[JsValue],
  ctx: &mut Context,
) -> JsResult<JsValue> {
  let session = str_arg(args, 0, "session")?;
  let delta = delta_from_js(args.get_or_undefined(1), ctx)?;
  crate::omw::omw::host::stream_endpoint(&session, &delta)
    .map_err(|e| JsNativeError::error().with_message(e))?;
  Ok(JsValue::undefined())
}

pub(crate) fn register_host(
  host: &boa_engine::JsObject,
  ctx: &mut Context,
) -> JsResult<()> {
  fn set(
    host: &boa_engine::JsObject,
    ctx: &mut Context,
    prop: &str,
    f: boa_engine::native_function::NativeFunctionPointer,
  ) -> JsResult<()> {
    let fun = NativeFunction::from_fn_ptr(f).to_js_function(ctx.realm());
    host.set(js_string!(prop), fun, false, ctx)?;
    Ok(())
  }
  set(host, ctx, "log", host_log)?;
  set(host, ctx, "timeNow", host_time_now)?;
  set(host, ctx, "timeFormat", host_time_format)?;
  set(host, ctx, "waitUntil", host_wait_until)?;
  set(host, ctx, "waitFor", host_wait_for)?;
  set(host, ctx, "waitCron", host_wait_cron)?;
  set(host, ctx, "sleepFor", host_sleep_for)?;
  set(host, ctx, "sleepUntil", host_sleep_until)?;
  set(host, ctx, "sleepCron", host_sleep_cron)?;
  set(host, ctx, "cancelTimer", host_cancel_timer)?;
  set(host, ctx, "subscribeAgent", host_subscribe_agent)?;
  set(host, ctx, "unsubscribeAgent", host_unsubscribe_agent)?;
  set(host, ctx, "subscribeLifecycle", host_subscribe_lifecycle)?;
  set(
    host,
    ctx,
    "unsubscribeLifecycle",
    host_unsubscribe_lifecycle,
  )?;
  set(host, ctx, "sendAgent", host_send_agent)?;
  set(host, ctx, "recv", host_recv)?;
  set(host, ctx, "tryRecv", host_try_recv)?;
  set(host, ctx, "newUuid", host_new_uuid)?;
  set(host, ctx, "base64Encode", host_base64_encode)?;
  set(host, ctx, "base64Decode", host_base64_decode)?;
  set(host, ctx, "memoryGet", host_memory_get)?;
  set(host, ctx, "memorySet", host_memory_set)?;
  set(host, ctx, "memoryRemove", host_memory_remove)?;
  set(host, ctx, "subscribeEndpoint", host_subscribe_endpoint)?;
  set(host, ctx, "unsubscribeEndpoint", host_unsubscribe_endpoint)?;
  set(host, ctx, "streamEndpoint", host_stream_endpoint)?;
  Ok(())
}
