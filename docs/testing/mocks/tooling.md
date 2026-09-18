# Tooling mock

`kind = "mock"` is an in-process scripted MCP-style tooling. Its config maps
one-to-one onto the WIT `tooling` interface, with resources scriptable in order
and every step optionally gated on the trace. It also records every tool call.

```toml
[tooling.mcp]
kind = "mock"
delay_ms = 10

tools = [{ name = "echo", description = "echo back", input_schema = {} }]

tool_calls = [
  { name = "echo", result = "hi" },
  { name = "add", result = "3", after = { kind = "call", op = "call_tool" } },
]

initial_resource_list = [
  { uri = "mem://notes", name = "notes", mime_type = "text/plain" },
]
initial_resource_contents = { "mem://notes" = "v1" }

resource_list_updates = [
  {
    resources = [
      { uri = "mem://notes", name = "notes", mime_type = "text/plain" },
    ],
    after = { kind = "call", op = "subscribe_resource_list" },
  },
]

resource_content_updates = [
  {
    uri = "mem://notes",
    content = "v2",
    after = { kind = "call", op = "read_resource" },
  },
]
```

## Keys

- `tools` — the tools `list-tools` returns, as `Tool` values.
- `tool_calls` — an **ordered** list of `call-tool` results. Each call consumes
  the next entry and verifies the invoked name matches; a mismatch, running past
  the end, or a call with no script at all is a tool-call error (delivered to
  the brain, which can react to it), so the mock stays honest about call order.
- `initial_resource_list` — the resources `list-resources` starts from.
- `initial_resource_contents` — `read-resource` content keyed by URI. Reading a
  URI with no content (initial or applied) errors.
- `resource_list_updates` — ordered **full replacement** lists replayed by
  `subscribe-resource-list`; each step replaces the current list and fires a
  `resource-list-updated`.
- `resource_content_updates` — ordered one-by-one updates replayed by
  `subscribe-resource` for the matching URI; each step sets the content and
  fires a `resource-updated`.
- `delay_ms` — how long every scripted step waits before firing (default `10`).

There is **no fallback**: a subscription with no configured updates emits
nothing.

## Ordering with `after`

Each `tool_calls` / `resource_*_updates` step takes an optional `after`, the
shared gate the endpoint mock uses and the same patterns as
[assertions](../testing.md#assertions):

- absent or `"start"` — fire as soon as the step is reached.
- a `call` / `inbound` pattern — wait until a matching trace event has been
  observed at any point in the run, then fire.

The mock logs the trace when the run is built, so a gate can observe events that
precede the step that waits on it; because the log is append-only, gates never
consume each other's events (so this is safe across subscriptions and across
agents sharing one tooling). Without a trace channel a gate warns and fires
immediately rather than hanging.
