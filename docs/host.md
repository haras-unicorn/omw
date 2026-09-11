# The host interface

The `host` interface (`host` in `src/native/omw/wit/omw.wit`) exposes the
static, baked-in capabilities of the runtime to an agent brain: logging, timer
helpers, inter-agent messaging, event receipt, and UUID generation. It is
imported by every brain (wasm components and the bundled rhai / js interpreters
alike) and implemented 1:1 by the host's `host::imports` module.

This page describes the WIT surface from the guest's point of view. The actor
mechanics that back it are covered in [actor](./actor.md).

## Events

The unit of everything the guest can observe is an `event-envelope`, a record
with two fields:

- `id` — the UUID handle of the subscribed source the event came from, and
- `event` — one of the following variant payloads:

| variant                                      | payload                        | meaning                                                           |
| -------------------------------------------- | ------------------------------ | ----------------------------------------------------------------- |
| `message(string)`                            | the text                       | a message from a subscribed agent                                 |
| `error(string)`                              | the error text                 | a failed I/O surfaced to the guest                                |
| `timer`                                      | —                              | a timestamp / duration / cron timer fired                         |
| `reload`                                     | —                              | the brain script changed; exit so the run restarts                |
| `shutdown`                                   | —                              | the process is shutting down; exit terminally                     |
| `chat-delta(chat-delta)`                     | a stream chunk                 | a chat-stream delta                                               |
| `chat-end`                                   | —                              | an open chat stream finished                                      |
| `tool-result(tool-result)`                   | `{ name, arguments, value }`   | a queued tool invocation returned                                 |
| `resource-list-updated`                      | `list<resource-info>`          | a subscribed resource _list_ changed, with the new list           |
| `resource-updated`                           | `resource-content`             | a subscribed resource updated in place, with freshly read content |
| `endpoint-message(endpoint-message)`         | `{ session, messages, tools }` | an inbound endpoint chat request routed to a subscribed agent     |
| `endpoint-session-end(endpoint-session-end)` | `{ session, error? }`          | an endpoint session ended: normal or abrupt                       |

A `chat-delta` carries `content`, a `tool-call`, and a `finish-reason`, all
optional, so a chunk may carry text, a partial tool call, or a terminal reason.

A `tool-result` event's payload carries the tool's `name`, its `arguments`, and
its `value` — the text result queued `call-tool` returned.

A `resource-updated` event's `resource-content` carries the resource's `uri`, an
optional `mime-type`, and the `content` itself — actual text for textual
formats, base64 for anything else (match on `mime-type` to tell which).

An `endpoint-message` event payload carries the endpoint session's `session` id,
the chat history as `messages` (a `chat-message` per entry), and `tools` the
tools the client advertised. An `endpoint-session-end` payload carries the
`session` id and an optional `error` when the session was interrupted.

The guest correlates an envelope with a specific source by matching `id` against
the UUID the opening call returned — for example the UUID from
`provider.chat-stream`, a `host.wait-*` call, `tooling.call-tool`, or
`tooling.subscribe-*`.

## Message flow

- `subscribe-agent(agent)` — subscribe to messages from another agent.
- `unsubscribe-agent(uuid)` — cancel a subscription by its `subscribe-agent`
  UUID.
- `subscribe-lifecycle()` — subscribe to lifecycle events (`reload`, `shutdown`,
  and reload-failure `error`); returns a UUID handle they arrive tagged with.
  Errors on a second subscribe (one per run).
- `unsubscribe-lifecycle(uuid)` — drop the lifecycle subscription; a foreign
  UUID is a no-op.
- `send-agent(agent, payload)` — send text to another agent. The message only
  lands in the recipient's inbox if it subscribed to the sender, tagged with
  that subscription's UUID.
- `recv()` — blocking receive of the next event from this agent's single inbox,
  with a host-side timeout (see `recv_timeout_secs` in
  [tunables](./tunables.md)). Returns an `event-envelope` or an error.
- `try-recv()` — non-blocking poll of the next event; returns `none` when the
  inbox is empty.

Correlate a lifecycle event by matching `id` against the UUID
`subscribe-lifecycle` returned, and `kind` for `reload` / `shutdown` / `error`
(a reload-failure `error` means the edit was invalid and the live run kept
going).

## The endpoint

The optional [endpoint server](./endpoint.md) lets each agent address itself as
an OpenAI-compatible model.

The guest side is three calls:

- `subscribe-endpoint(model)` — subscribe this agent to the endpoint under the
  model name `model`; returns a UUID handle. Inbound requests for that model
  arrive as `endpoint-message` events tagged with it, and the model is listed on
  `/v1/models` while subscribed. Errors if the model is already taken.
- `unsubscribe-endpoint(uuid)` — drop the model from `/v1/models`, stop routing,
  and abruptly end every in-flight session of that subscription (each fires an
  `endpoint-session-end` event with an error).
- `stream-endpoint(session, delta)` — stream one `chat-delta` to an endpoint
  session. Non-blocking: it buffers into the session's local queue and returns
  immediately. The reply ends when a delta carries a `finish-reason`; a session
  ends exactly once (delivering an `endpoint-session-end` event).

## Timers

`omw` uses unsigned 64-bit _ticks_ (milliseconds since the Unix epoch) as its
timestamp type. The guest gets a set of pure helpers plus three scheduling
calls:

- `time-now()` — current time in ticks.
- `time-format(ts, format)` — format a tick with a strftime-style format.
- `wait-until(ts)` — wait until a future timestamp fires; errors if `ts` is not
  in the future.
- `wait-for(ms)` — wait for `ms` milliseconds.
- `wait-cron(spec)` — wait until the next fire of a cron spec.
- `cancel-timer(uuid)` — cancel a pending wait by the UUID its `wait-*` call
  returned.
- `sleep-for(ms)` — blocking wait for `ms` milliseconds; returns once the delay
  elapses. Unlike `wait-for`, no `timer` event is scheduled.
- `sleep-until(ts)` — blocking wait until a future timestamp fires; errors if
  `ts` is not in the future. Unlike `wait-until`, no `timer` event is scheduled.
- `sleep-cron(spec)` — blocking wait until the next fire of a cron spec. Unlike
  `wait-cron`, no `timer` event is scheduled.

Each `wait-*` call returns a UUID immediately; when the deadline passes, a
`timer` event tagged with that UUID is delivered to the inbox. The brain reads
it back with `recv`/`try-recv` and matches `id` to know which timer fired. A
pending wait can be cancelled at any time with `cancel-timer(uuid)`. The
`sleep-*` variants are the blocking mirror — they hold the brain until the wait
finishes and return directly (no `timer` event, no cancel handle, and an error
is reported in-band).

## Logging

- `log(level, message)` — write a structured log line. `level` is one of
  `trace`, `debug`, `info`, `warn`, or `error`, and unknown levels default to
  `info`. The calling agent's name is attached as a structured field.

## UUIDs

- `new-uuid()` — a fresh v4 UUID string. Every handle used across the host
  (subscriptions, streams, timers) is one of these. The guest can also use it
  for its own purposes.

## Base64

- `base64-encode(bytes)` — encode raw bytes as standard padded base64 (RFC 4648
  §4), matching the encoding of MCP `blob` resource contents.
- `base64-decode(data)` — decode standard padded base64 back to raw bytes.
  Errors on invalid input.

## Memory

Per-agent string store that survives hot reloads:

- `memory-get(key)` — read a value; none when absent.
- `memory-set(key, value)` — store a value, overwriting.
- `memory-remove(key)` — delete; true when a value was present.

Scoped to the calling agent, so agents cannot race each other. Treat entries
like variables: subscription handles, state-machine state, small checkpoints.
Not a database — keep values small.
