// 09-resources: subscribe to the resource list and to one resource, read the
// resource, and react to the mock's scripted, `after`-gated updates. The RAII
// guards unsubscribe on drop.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let tooling = Tooling::get("docs")?;

  // Subscribe to the list first; the mock replays two scripted updates, each
  // gated on a trace event, and the host pumps them into our inbox in order.
  let sub_list = tooling.subscribe_resource_list()?;
  let _list0 = tooling.list_resources()?;
  let e1 = host::recv()?;
  let _list1 = tooling.list_resources()?;
  let e2 = host::recv()?;

  // Now subscribe to one resource; its content update is gated on our read.
  let sub_note = tooling.subscribe_resource("mem://notes")?;
  let _content1 = tooling.read_resource("mem://notes")?;
  let e3 = host::recv()?;
  let content2 = tooling.read_resource("mem://notes")?;

  drop(sub_list);
  drop(sub_note);
  host::info(&format!(
    "{}|{}|{}|{}",
    e1.kind(),
    e2.kind(),
    e3.kind(),
    content2.content,
  ));
  Ok(())
});