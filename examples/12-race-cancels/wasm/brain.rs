// 12-race-cancels: open two subscriptions, then cancel both in sequence. The
// `closed` observations are synchronous with each unsubscribe, so their order
// is fixed by the brain. The RAII guards unsubscribe on drop.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let tooling = Tooling::get("docs")?;
  let list = tooling.subscribe_resource_list()?;
  let note = tooling.subscribe_resource("mem://notes")?;
  drop(list);
  drop(note);
  host::info("cancelled both");
  Ok(())
});
