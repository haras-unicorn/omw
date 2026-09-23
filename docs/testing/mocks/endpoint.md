# Endpoint mock

`kind = "mock"` is an in-process scripted **client** for the endpoint. It plays
requests into a subscribed agent and drains the reply, with no socket and no
`listen` address.

```toml
[endpoint]
kind = "mock"
poll_ms = 10
requests = [
  { model = "gpt-4o", messages = [{ content = "hi" }], stream = true },
  {
    model = "gpt-4o",
    messages = [{ content = "bye" }],
    stream = false,
    after = { kind = "call", op = "chat" },
  },
]
```

## Keys

`poll_ms` — how long to wait between checks for a model subscription (default
`10`); the request fires as soon as the agent subscribes, so this only caps how
often the mock re-checks.

Each `requests` entry is:

- `model` — the model the request targets; the mock waits until an agent
  subscribes to it under this name.
- `messages` — the inbound chat messages, as `{ role, content }` (role defaults
  to `user`).
- `tools` — any tools the caller offered.
- `stream` — whether the caller asked for SSE. Informational only: the mock
  drains the same session either way.
- `after` — the ordering gate (below).

## Ordering with `after`

`after` is the shared step gate: the same `"start"` / pattern vocabulary the
tooling mock uses and the same patterns as
[assertions](../testing.md#assertions). Timing-based endpoint scripting is
flaky; in an actor model what matters is event _order_. `after` gates a request
on the recorded trace:

- absent or `"start"` — fire as soon as the model is subscribed (the default).
- a `call` / `inbound` pattern — wait until an event matching the pattern has
  been observed at any point in the run, then fire. The mock logs the trace
  before any agent runs, so it can gate on an event that precedes its own
  subscription.

So `after = { kind = "call", op = "chat" }` routes the request only once the
agent has made its first `chat`, letting a test assert that the brain handles
the request at a particular point relative to its other work.

`$any` / `$skip` entries cannot match a single event, so a gate that names one
warns and fires immediately rather than hanging; likewise, an embedder that runs
without a trace channel fires immediately with a warning.
