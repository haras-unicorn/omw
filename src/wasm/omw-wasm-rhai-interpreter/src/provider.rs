use rhai_rt::{Array, Dynamic, EvalAltResult, Map};

use crate::{
  convert::{
    handle_name, json_from_map, method, msg_from_dynamic, to_error,
    tool_from_dynamic,
  },
  omw::omw::provider,
  tooling::chat_result_to_map,
};

/// Look up a provider by name and return an object map with its methods.
pub(crate) fn provider_get(name: &str) -> Result<Map, Box<EvalAltResult>> {
  provider::get(name).map_err(to_error)?;
  let mut m = Map::new();
  m.insert("name".into(), name.into());
  m.insert("chat".into(), method("provider_chat")?.into());
  m.insert("chat_stream".into(), method("provider_chat_stream")?.into());
  m.insert("is_open".into(), method("provider_is_open")?.into());
  m.insert("cancel".into(), method("provider_cancel")?.into());
  m.insert("list_models".into(), method("provider_list_models")?.into());
  m.insert("kind".into(), method("provider_kind")?.into());
  Ok(m)
}

/// Serialize optional rhai params into the opaque JSON string the host
/// expects. Accepts a map (serialized to JSON) or an already-JSON string
/// (passed through, e.g. an `endpoint-message`'s `params`). Unit/empty is
/// `None`.
pub(crate) fn params_to_json(
  params: &Dynamic,
) -> Result<Option<String>, Box<EvalAltResult>> {
  if params.is_unit() {
    return Ok(None);
  }
  if let Some(text) = params.clone().try_cast::<String>() {
    return Ok(if text.trim().is_empty() {
      None
    } else {
      Some(text)
    });
  }
  if let Some(map) = params.clone().try_cast::<Map>() {
    if map.is_empty() {
      return Ok(None);
    }
    return json_from_map(&map).map(Some).map_err(to_error);
  }
  Err(to_error("params must be a map or a JSON string"))
}

pub(crate) fn provider_chat_stream(
  handle: Map,
  model: &str,
  messages: Array,
  tools: Array,
) -> Result<String, Box<EvalAltResult>> {
  provider_chat_stream_with_params(handle, model, messages, tools, ().into())
}

pub(crate) fn provider_chat_stream_with_params(
  handle: Map,
  model: &str,
  messages: Array,
  tools: Array,
  params: Dynamic,
) -> Result<String, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let p = provider::get(&name).map_err(to_error)?;
  let msgs = messages
    .into_iter()
    .map(msg_from_dynamic)
    .collect::<Result<Vec<_>, _>>()
    .map_err(to_error)?;
  let tls = tools
    .into_iter()
    .map(tool_from_dynamic)
    .collect::<Result<Vec<_>, _>>()
    .map_err(to_error)?;
  let params = params_to_json(&params)?;
  p.chat_stream(model, &msgs, &tls, params.as_deref())
    .map_err(to_error)
}

pub(crate) fn provider_chat(
  handle: Map,
  model: &str,
  messages: Array,
  tools: Array,
) -> Result<Map, Box<EvalAltResult>> {
  provider_chat_with_params(handle, model, messages, tools, ().into())
}

pub(crate) fn provider_chat_with_params(
  handle: Map,
  model: &str,
  messages: Array,
  tools: Array,
  params: Dynamic,
) -> Result<Map, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let p = provider::get(&name).map_err(to_error)?;
  let msgs = messages
    .into_iter()
    .map(msg_from_dynamic)
    .collect::<Result<Vec<_>, _>>()
    .map_err(to_error)?;
  let tls = tools
    .into_iter()
    .map(tool_from_dynamic)
    .collect::<Result<Vec<_>, _>>()
    .map_err(to_error)?;
  let params = params_to_json(&params)?;
  let result = p.chat(model, &msgs, &tls, params.as_deref())?;
  Ok(chat_result_to_map(result))
}

pub(crate) fn provider_is_open(
  handle: Map,
  uuid: &str,
) -> Result<bool, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  Ok(provider::get(&name).map_err(to_error)?.is_open(uuid))
}

pub(crate) fn provider_cancel(
  handle: Map,
  uuid: &str,
) -> Result<(), Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  provider::get(&name).map_err(to_error)?.cancel(uuid);
  Ok(())
}

pub(crate) fn provider_list_models(
  handle: Map,
) -> Result<Array, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  let models = provider::get(&name)
    .map_err(to_error)?
    .list_models()
    .map_err(to_error)?;
  Ok(models.into_iter().map(|m| m.into()).collect())
}

pub(crate) fn provider_kind(handle: Map) -> Result<String, Box<EvalAltResult>> {
  let name = handle_name(&handle)?;
  Ok(provider::get(&name).map_err(to_error)?.kind())
}
