# 11-tool-call-order

Queued tool calls, delivered at guest call boundaries.

The tooling mock scripts two `call_tool` results: the first is `after`-gated on
a `list_tools` call, the second fires as soon as it is queued. Because the host
flushes call-boundary injections synchronously, each result lands at the
boundary of the call that triggered it — a function of the brain, never of the
scheduler.
