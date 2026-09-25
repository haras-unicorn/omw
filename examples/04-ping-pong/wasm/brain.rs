// Two agents share this script. `whoami` tells each agent its own name, so it
// subscribes only to itself and plays a deterministic ping-pong with its own
// inbox. The handle goes to memory so a hot reload can reuse it; the RAII guard
// unsubscribes when the run ends.
#![no_main]

use omw_wasm_rust::host;

omw_wasm_rust::brain!(|| {
  let me = host::whoami();
  let sub = host::subscribe_agent(&me)?;
  host::memory_set("sub", sub.uuid());
  host::send_agent(&me, "ping");
  let ping = host::recv()?;
  host::send_agent(&me, "pong");
  let pong = host::recv()?;
  host::info(&format!("{}|{}", ping.kind(), pong.kind()));
  Ok(())
});
