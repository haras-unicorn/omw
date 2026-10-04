// 11-tool-call-order: queue a gated call, trigger it with a `listTools` call,
// then queue a second call that fires immediately. The results arrive in the
// order the brain caused them.
const t = omw.tooling.get("mcp");
const first = t.callTool("first", {});
const _tools = t.listTools();
const r1 = omw.host.recv();
const second = t.callTool("second", {});
const r2 = omw.host.recv();
omw.host.log("info", r1.kind + "|" + r2.kind);
