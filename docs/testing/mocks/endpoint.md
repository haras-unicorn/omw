# Endpoint mock

`kind = "mock"` is an in-process scripted **client** for the endpoint. It plays
requests into a subscribed agent and drains the reply, with no socket and no
`listen` address.

```toml
[endpoint]
kind = "mock"
poll_ms = 10
requests = [
  {
    model = "gpt-4o",
    session_id = "first",
    messages = [{ content = "hi" }],
    stream = true,
  },
  {
    model = "gpt-4o",
    session_id = "second",
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
- `session_id` — **required**: a user-written label for the session, so
  concurrent sessions can be told apart. The mock correlates it with the opaque
  session UUID the host and brain see and records both in the snapshot; it is
  never asserted or matched (match on content instead). Reusing a label after
  its session closed is a new session with a new UUID.
- `messages` — the inbound chat messages, as `{ role, content }` (role defaults
  to `user`).
- `tools` — any tools the caller offered.
- `params` — opaque generation params (temperature, …) to forward with the
  request, surfaced as the `endpoint-message`'s `params` JSON string.
- `stream` — whether the caller asked for SSE. Informational only: the mock
  drains the same session either way.
- `after` — the ordering gate (below).
- `session_end` — an opt-in scripted ending (below).

## Concurrent sessions

`serve` runs each request's flow (subscribe gate → `after` gate → route → drain)
concurrently, so more than one session can be live at once. `session_id` is how
you tell them apart; because routing order is no longer intrinsically
sequential, use `after` to impose an order when a test needs one.

## Scripted ending with `session_end`

By default the mock drains a reply and removes the session **silently**, exactly
as before: no `endpoint-session-end` is delivered. Set `session_end` to end the
session from the client side and observe the lifecycle:

- `{ close = N }` — after `N` received deltas, remove the session and deliver a
  normal `endpoint-session-end { error = None }`.
- `{ abort = N }` — after `N` received deltas, abort the session, delivering
  `endpoint-session-end { error = "endpoint session aborted" }`.
- `N = 0` fires immediately after routing, before any delta.
- If the reply ends first (a terminal finish-reason or `Close`) or shutdown
  arrives before `N` deltas, the script is a no-op — the session is removed
  silently.

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

A `$while` / `$until` gate resolves against its inner `call` / `inbound`
condition; an embedder that runs without a trace channel fires immediately with
a warning.

Each routed request's drained reply is recorded, and the `--dump` snapshot lists
every reply's `model`, `session_id`, `session`, streamed `deltas`, and how the
session ended (`closed`, `aborted` and the `error`).
