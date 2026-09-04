# The host interface

The `host` interface (`host` in `src/native/omw/wit/omw.wit`) exposes the
static, baked-in capabilities of the runtime to an agent brain: logging, timer
helpers, inter-agent messaging, event receipt, and UUID generation. It is
imported by every brain (wasm components and the bundled rhai interpreter alike)
and implemented 1:1 by the host's `host::imports` module.

This page describes the WIT surface from the guest's point of view. The actor
mechanics that back it are covered in [actor](./actor.md).

## Events

The unit of everything the guest can observe is an `event-envelope`, a record
with two fields:

- `id` — the UUID handle of the subscribed source the event came from, and
- `event` — one of the following variant payloads:

| variant                  | payload               | meaning                                                                   |
| ------------------------ | --------------------- | ------------------------------------------------------------------------- |
| `message(string)`        | the text              | a message from a subscribed agent                                         |
| `error(string)`          | the error text        | a failed I/O surfaced to the guest                                        |
| `timer`                  | —                     | a timestamp / duration / cron timer fired                                 |
| `chat-delta(chat-delta)` | a stream chunk        | a chat-stream delta                                                       |
| `stream-end`             | —                     | an open chat stream finished                                              |
| `resource-list-updated`  | `list<resource-info>` | a subscribed resource _list_ changed, with the new list                   |
| `resource-updated`       | `resource-content`    | a subscribed resource was updated in place, with its freshly read content |

A `chat-delta` carries `content`, a `tool-call`, and a `finish-reason`, all
optional, so a chunk may carry text, a partial tool call, or a terminal reason.
A `resource-updated` event's `resource-content` carries the resource's `uri`, an
optional `mime-type`, and the `content` itself — actual text for textual
formats, base64 for anything else (match on `mime-type` to tell which).

The guest correlates an envelope with a specific source by matching `id` against
the UUID the opening call returned — for example the UUID from `provider.chat`,
a `host.wait-*` call, or `tooling.subscribe-*`.

## Message flow

- `subscribe(agent)` — subscribe to messages from another agent.
- `unsubscribe(uuid)` — cancel a subscription by its `subscribe` UUID.
- `send(agent, payload)` — send text to another agent. The message only lands in
  the recipient's inbox if it subscribed to the sender, tagged with that
  subscription's UUID.
- `recv()` — blocking receive of the next event from this agent's single inbox,
  with a 60 second host-side timeout. Returns an `event-envelope` or an error.
- `try-recv()` — non-blocking poll of the next event; returns `none` when the
  inbox is empty.

## Timers

`omw` uses unsigned 64-bit _ticks_ (milliseconds since the Unix epoch) as its
timestamp type. The guest gets a set of pure helpers plus three scheduling
calls:

- `now()` — current time in ticks.
- `timestamp-add(ts, ms)` / `timestamp-sub(ts, ms)` — move a timestamp by an
  offset, saturating.
- `timestamp-diff(a, b)` — signed milliseconds `a - b`.
- `timestamp-format(ts, format)` — format a tick with a strftime-style format.
- `wait-timestamp(ts)` — wait until a future timestamp fires; errors if `ts` is
  not in the future.
- `wait-duration(ms)` — wait for `ms` milliseconds.
- `wait-cron(spec)` — wait until the next fire of a cron spec.
- `cancel(uuid)` — cancel a pending wait by the UUID its `wait-*` call returned.

Each `wait-*` call returns a UUID immediately; when the deadline passes, a
`timer` event tagged with that UUID is delivered to the inbox. The brain reads
it back with `recv`/`try-recv` and matches `id` to know which timer fired. A
pending wait can be cancelled at any time with `cancel(uuid)`.

## Logging

- `log(level, message)` — write a structured log line. `level` is one of
  `trace`, `debug`, `info`, `warn`, or `error` and unknown levels default to
  `info`. The calling agent's name is attached as a structured field.

## UUIDs

- `new-uuid()` — a fresh v4 UUID string. Every handle used across the host
  (subscriptions, streams, timers) is one of these. The guest can also use it
  for its own purposes.
