// 11-tool-call-order: queue a gated call, trigger it with a `list_tools` call,
// then queue a second call that fires immediately. The results arrive in the
// order the brain caused them.
let t = omw::tooling::get("mcp");
let _first = t.call_tool("first", #{});
let _tools = t.list_tools();
let r1 = omw::host::recv();
let _second = t.call_tool("second", #{});
let r2 = omw::host::recv();
omw::host::log("info", r1.kind + "|" + r2.kind);
