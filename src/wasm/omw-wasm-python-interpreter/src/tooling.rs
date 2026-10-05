//! The `omw.tooling` namespace: name-addressed tooling handles whose methods
//! mirror the WIT `tooling` interface.

use rustpython_vm::builtins::PyStrRef;
use rustpython_vm::{PyObjectRef, PyResult, VirtualMachine};

use crate::convert;

pub(crate) fn tooling_get(
  name: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let name = convert::str_of(&name);
  crate::omw::omw::tooling::get(&name).map_err(|e| vm.new_type_error(e))?;
  tooling_handle(vm, &name)
}

fn tooling_handle(vm: &VirtualMachine, name: &str) -> PyResult<PyObjectRef> {
  let handle = convert::namespace(vm)?;
  handle.set_attr("name", vm.new_pyobj(name), vm)?;

  let list_tools = {
    let name = name.to_owned();
    vm.new_function("list_tools", move |vm: &VirtualMachine| {
      tooling_list_tools(&name, vm)
    })
  };
  handle.set_attr("list_tools", list_tools, vm)?;

  let call_tool = {
    let name = name.to_owned();
    vm.new_function(
      "call_tool",
      move |tool: PyStrRef, arguments: PyObjectRef, vm: &VirtualMachine| {
        tooling_call_tool(&name, &tool, &arguments, vm)
      },
    )
  };
  handle.set_attr("call_tool", call_tool, vm)?;

  let call_tool_blocking = {
    let name = name.to_owned();
    vm.new_function(
      "call_tool_blocking",
      move |tool: PyStrRef, arguments: PyObjectRef, vm: &VirtualMachine| {
        tooling_call_tool_blocking(&name, &tool, &arguments, vm)
      },
    )
  };
  handle.set_attr("call_tool_blocking", call_tool_blocking, vm)?;

  let is_open = {
    let name = name.to_owned();
    vm.new_function("is_open", move |uuid: PyStrRef, vm: &VirtualMachine| {
      tooling_is_open(&name, &uuid, vm)
    })
  };
  handle.set_attr("is_open", is_open, vm)?;

  let cancel = {
    let name = name.to_owned();
    vm.new_function("cancel", move |uuid: PyStrRef, vm: &VirtualMachine| {
      tooling_cancel(&name, &uuid, vm)
    })
  };
  handle.set_attr("cancel", cancel, vm)?;

  let kind = {
    let name = name.to_owned();
    vm.new_function("kind", move |vm: &VirtualMachine| tooling_kind(&name, vm))
  };
  handle.set_attr("kind", kind, vm)?;

  let list_resources = {
    let name = name.to_owned();
    vm.new_function("list_resources", move |vm: &VirtualMachine| {
      tooling_list_resources(&name, vm)
    })
  };
  handle.set_attr("list_resources", list_resources, vm)?;

  let read_resource = {
    let name = name.to_owned();
    vm.new_function(
      "read_resource",
      move |uri: PyStrRef, vm: &VirtualMachine| {
        tooling_read_resource(&name, &uri, vm)
      },
    )
  };
  handle.set_attr("read_resource", read_resource, vm)?;

  let subscribe_resource_list = {
    let name = name.to_owned();
    vm.new_function("subscribe_resource_list", move |vm: &VirtualMachine| {
      tooling_subscribe_resource_list(&name, vm)
    })
  };
  handle.set_attr("subscribe_resource_list", subscribe_resource_list, vm)?;

  let subscribe_resource = {
    let name = name.to_owned();
    vm.new_function(
      "subscribe_resource",
      move |uri: PyStrRef, vm: &VirtualMachine| {
        tooling_subscribe_resource(&name, &uri, vm)
      },
    )
  };
  handle.set_attr("subscribe_resource", subscribe_resource, vm)?;

  let unsubscribe_resource_list = {
    let name = name.to_owned();
    vm.new_function(
      "unsubscribe_resource_list",
      move |uuid: PyStrRef, vm: &VirtualMachine| {
        tooling_unsubscribe_resource_list(&name, &uuid, vm)
      },
    )
  };
  handle.set_attr(
    "unsubscribe_resource_list",
    unsubscribe_resource_list,
    vm,
  )?;

  let unsubscribe_resource = {
    let name = name.to_owned();
    vm.new_function(
      "unsubscribe_resource",
      move |uuid: PyStrRef, vm: &VirtualMachine| {
        tooling_unsubscribe_resource(&name, &uuid, vm)
      },
    )
  };
  handle.set_attr("unsubscribe_resource", unsubscribe_resource, vm)?;

  Ok(handle)
}

fn tooling_list_tools(
  name: &str,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let tools = crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .list_tools()
    .map_err(|e| vm.new_runtime_error(e))?;
  let values: PyResult<Vec<PyObjectRef>> = tools
    .into_iter()
    .map(|tool| convert::tool_to_py(vm, tool))
    .collect();
  Ok(vm.ctx.new_list(values?).into())
}

fn tooling_call_tool(
  name: &str,
  tool: &PyStrRef,
  arguments: &PyObjectRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let tooling =
    crate::omw::omw::tooling::get(name).map_err(|e| vm.new_runtime_error(e))?;
  let arguments = convert::json_string_of(vm, arguments)?;
  let id = tooling
    .call_tool(&convert::str_of(tool), &arguments)
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

fn tooling_call_tool_blocking(
  name: &str,
  tool: &PyStrRef,
  arguments: &PyObjectRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let tooling =
    crate::omw::omw::tooling::get(name).map_err(|e| vm.new_runtime_error(e))?;
  let arguments = convert::json_string_of(vm, arguments)?;
  let result = tooling
    .call_tool_blocking(&convert::str_of(tool), &arguments)
    .map_err(|e| vm.new_runtime_error(e))?;
  convert::tool_result_to_py(vm, result)
}

fn tooling_is_open(
  name: &str,
  uuid: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let tooling =
    crate::omw::omw::tooling::get(name).map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(tooling.is_open(&convert::str_of(uuid))))
}

fn tooling_cancel(
  name: &str,
  uuid: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<()> {
  let tooling =
    crate::omw::omw::tooling::get(name).map_err(|e| vm.new_runtime_error(e))?;
  tooling.cancel(&convert::str_of(uuid));
  Ok(())
}

fn tooling_kind(name: &str, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  let kind = crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .kind();
  Ok(vm.new_pyobj(kind))
}

fn tooling_list_resources(
  name: &str,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let resources = crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .list_resources()
    .map_err(|e| vm.new_runtime_error(e))?;
  let values: PyResult<Vec<PyObjectRef>> = resources
    .into_iter()
    .map(|resource| convert::resource_to_py(vm, resource))
    .collect();
  Ok(vm.ctx.new_list(values?).into())
}

fn tooling_read_resource(
  name: &str,
  uri: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let content = crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .read_resource(&convert::str_of(uri))
    .map_err(|e| vm.new_runtime_error(e))?;
  convert::resource_content_to_py(vm, content)
}

fn tooling_subscribe_resource_list(
  name: &str,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id = crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .subscribe_resource_list()
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

fn tooling_subscribe_resource(
  name: &str,
  uri: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id = crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .subscribe_resource(&convert::str_of(uri))
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

fn tooling_unsubscribe_resource_list(
  name: &str,
  uuid: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .unsubscribe_resource_list(&convert::str_of(uuid));
  Ok(())
}

fn tooling_unsubscribe_resource(
  name: &str,
  uuid: &PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::tooling::get(name)
    .map_err(|e| vm.new_runtime_error(e))?
    .unsubscribe_resource(&convert::str_of(uuid));
  Ok(())
}
