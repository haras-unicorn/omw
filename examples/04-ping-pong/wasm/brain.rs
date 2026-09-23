// Two agents share this script. Without a `whoami` host call it cannot tell
// which name is its own, so it subscribes to both, pings both, and receives
// exactly one message: its self-addressed ping is always delivered because it
// subscribed to its own name first. The handles go to memory so a hot reload
// can reuse them; the RAII guards unsubscribe when the run ends.
#![no_main]

use omw_wasm_rust::host;

omw_wasm_rust::brain!(|| {
  let sub_alice = host::subscribe_agent("alice")?;
  let sub_bob = host::subscribe_agent("bob")?;
  host::memory_set("sub-alice", sub_alice.uuid());
  host::memory_set("sub-bob", sub_bob.uuid());
  host::send_agent("alice", "ping");
  host::send_agent("bob", "ping");
  let event = host::recv()?;
  host::send_agent("alice", "pong");
  host::send_agent("bob", "pong");
  host::info(event.kind());
  Ok(())
});
