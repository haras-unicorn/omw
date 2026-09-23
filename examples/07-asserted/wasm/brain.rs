// This brain never exits on its own: it subscribes to itself, sends one
// message, then loops on `recv` forever. The test's `outcome = "asserted"`
// stops it once the first three events have been observed.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let _subscription = host::subscribe_agent("alice")?;
  host::send_agent("alice", "ping");
  loop {
    let event = host::recv()?;
    host::info(event.kind());
  }
});
