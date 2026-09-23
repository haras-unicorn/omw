# 02-tool-agent

A ReAct loop against the mock provider and mock tooling: the first `chat`
returns a tool call, the brain runs it with `call_tool_blocking`, then sends the
result back for a final answer.

```sh
omw-test run examples/02-tool-agent
```

It asserts `call(chat)` → `call(call_tool_blocking)` → `call(chat)` and a
`completed` outcome. See the [examples guide](../../docs/examples.md) for how
the variants and assertions work.
