# Tooling mock

`kind = "mock"` is an in-process scripted MCP-style tooling. Its config maps
one-to-one onto the WIT `tooling` interface, with resources scriptable in order
and every step optionally gated on the trace. It also records every tool call.

```toml
[tooling.mcp]
kind = "mock"
delay_ms = 10

tools = [
  {
    name = "echo",
    title = "Echo",
    description = "echo back",
    input_schema = {},
    annotations = { read_only_hint = true },
  },
]

tool_calls = [
  { name = "echo", result = "hi" },
  {
    name = "add",
    result = "3",
    structured_content = { sum = 3 },
    after = { kind = "call", op = "call_tool" },
  },
]

initial_resource_list = [
  {
    uri = "mem://notes",
    name = "notes",
    title = "Notes",
    mime_type = "text/plain",
    size = 2,
  },
]
initial_resource_contents = { "mem://notes" = ["v1"] }

resource_templates = [
  {
    uri_template = "mem://notes/{id}",
    name = "note",
    mime_type = "text/plain",
  },
]

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
    contents = ["v2"],
    after = { kind = "call", op = "read_resource" },
  },
]
```

## Keys

- `tools` — the tools `list-tools` returns, as `Tool` values. Each may carry an
  optional `title`, an optional `output_schema` (the tool's structured-output
  JSON Schema) and optional `annotations` (`read_only_hint` / `destructive_hint`
  / `idempotent_hint` / `open_world_hint`) alongside `name`, `description?`, and
  `input_schema`.
- `tool_calls` — an **ordered** list of `call-tool` results. Each call consumes
  the next entry and verifies the invoked name matches; a mismatch, running past
  the end, or a call with no script at all is a tool-call error (delivered to
  the brain, which can react to it), so the mock stays honest about call order.
  `result` is a text convenience that becomes a single MCP text content block;
  an optional `structured_content` is passed through as the tool's MCP
  `structuredContent`. An optional `is_error = true` succeeds but marks the
  result a tool-execution error (MCP `isError`), with `result` still supplying
  the content text — distinct from `error`, which fails the call at the protocol
  level. A step may instead set `error = "..."` (fail the call with that error
  after the gate and delay) or `pending = true` (never complete; for
  cancellation tests).
- `initial_resource_list` — the resources `list-resources` starts from.
- `initial_resource_contents` — `read-resource` contents keyed by URI; each
  value is the ordered list of content strings a read yields (for example
  `{ "mem://notes" = ["v1"] }`). Reading a URI with no content (initial or
  applied) errors.
- `resource_templates` — the templates `list-resource-templates` returns, as
  `resource-template` values (`uri_template`, `name`, and optional `title`,
  `description` and `mime_type`).
- `resource_list_updates` — ordered **full replacement** lists replayed by
  `subscribe-resource-list`; each step replaces the current list and fires a
  `resource-list-updated`. A step may instead set `error = "..."` (yield that
  error) or `pending = true` (never yield).
- `resource_content_updates` — ordered one-by-one updates replayed by
  `subscribe-resource` for the matching URI; each step sets the resource's
  `contents` (a list) and fires a `resource-updated` carrying them. A step may
  instead set `error = "..."` or `pending = true`, as above.
- `delay_ms` — how long every scripted step waits before firing (default `10`).

Every `call-tool` is recorded, and the `--dump` snapshot lists each call's
`name`, `arguments` and the `result` it returned (`null` for a call that
errored).

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
