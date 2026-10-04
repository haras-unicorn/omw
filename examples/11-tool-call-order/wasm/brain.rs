// 11-tool-call-order: queue a gated call, trigger it with a `list_tools` call,
// then queue a second call that fires immediately. The results arrive in the
// order the brain caused them.
#![no_main]

use omw_wasm_rust::{host, prelude::*, serde_json};

omw_wasm_rust::brain!(|| {
  let tooling = Tooling::get("mcp")?;
  let _first = tooling.call_tool("first", serde_json::json!({}))?;
  let _tools = tooling.list_tools()?;
  let r1 = host::recv()?;
  let _second = tooling.call_tool("second", serde_json::json!({}))?;
  let r2 = host::recv()?;
  host::info(&format!("{}|{}", r1.kind(), r2.kind()));
  Ok(())
});
