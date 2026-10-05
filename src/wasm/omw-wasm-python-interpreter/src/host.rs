//! The `omw.host` namespace: static helpers baked into the host module,
//! mirroring the rhai/js interpreters.

use rustpython_vm::builtins::PyStrRef;
use rustpython_vm::{PyObjectRef, PyResult, VirtualMachine};

use crate::convert;

fn opt_str(
  value: Option<String>,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  match value {
    Some(text) => Ok(vm.new_pyobj(text)),
    None => Ok(vm.ctx.none()),
  }
}

pub(crate) fn host_log(
  level: PyStrRef,
  message: PyStrRef,
  _vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::log(
    &convert::str_of(&level),
    &convert::str_of(&message),
  );
  Ok(())
}

pub(crate) fn host_whoami(vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  Ok(vm.new_pyobj(crate::omw::omw::host::whoami()))
}

pub(crate) fn host_time_now(vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  Ok(vm.new_pyobj(crate::omw::omw::host::time_now()))
}

pub(crate) fn host_time_format(
  ts: u64,
  format: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  Ok(vm.new_pyobj(crate::omw::omw::host::time_format(
    ts,
    &convert::str_of(&format),
  )))
}

pub(crate) fn host_wait_until(
  ts: u64,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id = crate::omw::omw::host::wait_until(ts)
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

pub(crate) fn host_wait_for(
  ms: u64,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id =
    crate::omw::omw::host::wait_for(ms).map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

pub(crate) fn host_wait_cron(
  spec: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id = crate::omw::omw::host::wait_cron(&convert::str_of(&spec))
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

pub(crate) fn host_sleep_for(ms: u64, _vm: &VirtualMachine) -> PyResult<()> {
  crate::omw::omw::host::sleep_for(ms);
  Ok(())
}

pub(crate) fn host_sleep_until(ts: u64, vm: &VirtualMachine) -> PyResult<()> {
  crate::omw::omw::host::sleep_until(ts).map_err(|e| vm.new_runtime_error(e))
}

pub(crate) fn host_sleep_cron(
  spec: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::sleep_cron(&convert::str_of(&spec))
    .map_err(|e| vm.new_runtime_error(e))
}

pub(crate) fn host_cancel_timer(
  uuid: PyStrRef,
  _vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::cancel_timer(&convert::str_of(&uuid));
  Ok(())
}

pub(crate) fn host_subscribe_agent(
  agent: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id = crate::omw::omw::host::subscribe_agent(&convert::str_of(&agent))
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

pub(crate) fn host_unsubscribe_agent(
  uuid: PyStrRef,
  _vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::unsubscribe_agent(&convert::str_of(&uuid));
  Ok(())
}

pub(crate) fn host_subscribe_lifecycle(
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id = crate::omw::omw::host::subscribe_lifecycle()
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

pub(crate) fn host_unsubscribe_lifecycle(
  uuid: PyStrRef,
  _vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::unsubscribe_lifecycle(&convert::str_of(&uuid));
  Ok(())
}

pub(crate) fn host_send_agent(
  agent: PyStrRef,
  payload: PyStrRef,
  _vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::send_agent(
    &convert::str_of(&agent),
    &convert::str_of(&payload),
  );
  Ok(())
}

pub(crate) fn host_recv(vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  let envelope =
    crate::omw::omw::host::recv().map_err(|e| vm.new_runtime_error(e))?;
  convert::envelope_to_py(vm, envelope)
}

pub(crate) fn host_try_recv(vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  let envelope =
    crate::omw::omw::host::try_recv().map_err(|e| vm.new_runtime_error(e))?;
  match envelope {
    Some(envelope) => convert::envelope_to_py(vm, envelope),
    None => Ok(vm.ctx.none()),
  }
}

pub(crate) fn host_new_uuid(vm: &VirtualMachine) -> PyResult<PyObjectRef> {
  Ok(vm.new_pyobj(crate::omw::omw::host::new_uuid()))
}

pub(crate) fn host_base64_encode(
  bytes: Vec<u8>,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  Ok(vm.new_pyobj(crate::omw::omw::host::base64_encode(&bytes)))
}

pub(crate) fn host_base64_decode(
  data: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let bytes = crate::omw::omw::host::base64_decode(&convert::str_of(&data))
    .map_err(|e| vm.new_value_error(e))?;
  Ok(vm.ctx.new_bytes(bytes).into())
}

pub(crate) fn host_memory_get(
  key: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  opt_str(
    crate::omw::omw::host::memory_get(&convert::str_of(&key)),
    vm,
  )
}

pub(crate) fn host_memory_get_as(
  key: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  match crate::omw::omw::host::memory_get(&convert::str_of(&key)) {
    Some(text) => convert::json_to_py(vm, &convert::json_or_string(&text)),
    None => Ok(vm.ctx.none()),
  }
}

pub(crate) fn host_memory_set(
  key: PyStrRef,
  value: PyStrRef,
  _vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::memory_set(
    &convert::str_of(&key),
    &convert::str_of(&value),
  );
  Ok(())
}

pub(crate) fn host_memory_set_as(
  key: PyStrRef,
  value: PyObjectRef,
  vm: &VirtualMachine,
) -> PyResult<()> {
  let text = convert::json_string_of(vm, &value)?;
  crate::omw::omw::host::memory_set(&convert::str_of(&key), &text);
  Ok(())
}

pub(crate) fn host_memory_remove(
  key: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  Ok(vm.new_pyobj(crate::omw::omw::host::memory_remove(&convert::str_of(&key))))
}

pub(crate) fn host_subscribe_endpoint(
  model: PyStrRef,
  vm: &VirtualMachine,
) -> PyResult<PyObjectRef> {
  let id = crate::omw::omw::host::subscribe_endpoint(&convert::str_of(&model))
    .map_err(|e| vm.new_runtime_error(e))?;
  Ok(vm.new_pyobj(id))
}

pub(crate) fn host_unsubscribe_endpoint(
  uuid: PyStrRef,
  _vm: &VirtualMachine,
) -> PyResult<()> {
  crate::omw::omw::host::unsubscribe_endpoint(&convert::str_of(&uuid));
  Ok(())
}

pub(crate) fn host_stream_endpoint(
  session: PyStrRef,
  delta: PyObjectRef,
  vm: &VirtualMachine,
) -> PyResult<()> {
  let delta = convert::delta_from_py(vm, &delta)?;
  crate::omw::omw::host::stream_endpoint(&convert::str_of(&session), &delta)
    .map_err(|e| vm.new_runtime_error(e))
}
