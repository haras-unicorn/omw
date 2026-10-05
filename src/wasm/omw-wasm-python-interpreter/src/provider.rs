//! The `omw.provider` namespace: name-addressed provider handles whose methods
//! mirror the WIT `provider` interface.

use rustpython_vm::builtins::PyStrRef;
use rustpython_vm::function::OptionalArg;
use rustpython_vm::{PyObjectRef, PyResult, VirtualMachine};

use crate::convert;

pub(crate) fn provider_get(
  name: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let name = convert::str_of(&name);
  crate::omw::omw::provider::get(&name).map_err(|e| vm.new_type_error(e))?;
  provider_handle(vm, &name)
}

fn provider_handle(vm: &VirtualMachine, name: &str) -> PyResult<PyObjectRef> {
  let handle = convert::namespace(vm)?;
  handle.set_attr("name", vm.new_pyobj(name), vm)?;

  let chat = {
    let name = name.to_owned();
    vm.new_function(
      "chat",
      move |model: PyStrRef,
            messages: PyObjectRef,
            tools: PyObjectRef,
            params: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine| {
        let params = params.into_option();
        provider_chat(&name, &model, &messages, &tools, params.as_ref(), vm)
      },
    )
  };
  handle.set_attr("chat", chat, vm)?;

  let chat_stream = {
    let name = name.to_owned();
    vm.new_function(
      "chat_stream",
      move |model: PyStrRef,
            messages: PyObjectRef,
            tools: PyObjectRef,
            params: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine| {
        let params = params.into_option();
        provider_chat_stream(
          &name,
          &model,
          &messages,
          &tools,
          params.as_ref(),
          vm,
        )
      },
    )
  };
  handle.set_attr("chat_stream", chat_stream, vm)?;

  let is_open = {
    let name = name.to_owned();
    vm.new_function("is_open", move |uuid: PyStrRef, vm: &VirtualMachine| {
      provider_is_open(&name, &uuid, vm)
    })
  };
  handle.set_attr("is_open", is_open, vm)?;

  let cancel = {
    let name = name.to_owned();
    vm.new_function("cancel", move |uuid: PyStrRef, vm: &VirtualMachine| {
      provider_cancel(&name, &uuid, vm)
    })
  };
  handle.set_attr("cancel", cancel, vm)?;

  let list_models = {
    let name = name.to_owned();
    vm.new_function("list_models", move |vm: &VirtualMachine| {
      provider_list_models(&name, vm)
    })
  };
  handle.set_attr("list_models", list_models, vm)?;

  let kind = {
    let name = name.to_owned();
    vm.new_function("kind", move |vm: &VirtualMachine| provider_kind(&name, vm))
  };
  handle.set_attr("kind", kind, vm)?;

  Ok(handle)
}

fn provider_chat(
  name: &str,
  model: &PyStrRef,
  messages: &PyObjectRef,
  tools: &PyObjectRef,
  params: Option<&PyObjectRef>,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let provider = crate::omw::omw::provider::get(name)
    .map_err(|e| vm.new_runtime_error(e))?;
  let model = convert::str_of(model);
  let messages = convert::messages_from_py(vm, messages)?;
  let tools = convert::tools_from_py(vm, tools)?;
  let params = convert::params_from_py(vm, params)?;
  let result = provider
    .chat(&model, &messages, &tools, params.as_deref())
    .map_err(|e| vm.new_runtime_error(e))?;
  convert::chat_result_to_py(vm, result)
}

fn provider_chat_stream(
  name: &str,
  model: &PyStrRef,
  messages: &PyObjectRef,
  tools: &PyObjectRef,
  params: Option<&PyObjectRef>,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let provider = crate::omw::omw::provider::get(name)
    .map_err(|e| vm.new_runtime_error(e))?;
  let model = convert::str_of(model);
  let messages = convert::messages_from_py(vm, messages)?;
  let tools = convert::tools_from_py(vm, tools)?;
  let params = convert::params_from_py(vm, params)?;
  let id = provider
    .chat_stream(&model, &messages, &tools, params.as_deref())
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

fn provider_is_open(
  name: &str,
  uuid: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let provider = crate::omw::omw::provider::get(name)
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(provider.is_open(&convert::str_of(uuid))))
}

fn provider_cancel(
  name: &str,
  uuid: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<()> {
  let provider = crate::omw::omw::provider::get(name)
    .map_err(|e| vm.new_runtime_error(e))?;
  provider.cancel(&convert::str_of(uuid));
  Ok(())
}

fn provider_list_models(
  name: &str,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let provider = crate::omw::omw::provider::get(name)
    .map_err(|e| vm.new_runtime_error(e))?;
  let models = provider
    .list_models()
    .map_err(|e| vm.new_runtime_error(e))?;
  let values: Vec<PyObjectRef> = models
    .iter()
    .map(|model| vm.new_pyobj(model.as_str()))
    .collect();
  Ok(vm.ctx.new_list(values).into())
}

fn provider_kind(name: &str, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  let provider = crate::omw::omw::provider::get(name)
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(provider.kind()))
}
