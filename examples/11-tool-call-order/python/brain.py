# 11-tool-call-order: queue a gated call, trigger it with a `list_tools` call,
# then queue a second call that fires immediately. The results arrive in the
# order the brain caused them.
t = omw.tooling.get("mcp")
first = t.call_tool("first", {})
_tools = t.list_tools()
r1 = omw.host.recv()
second = t.call_tool("second", {})
r2 = omw.host.recv()
omw.host.log("info", r1.kind + "|" + r2.kind)
