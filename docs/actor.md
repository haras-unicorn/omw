# The actor model

`omw` is built around a small actor model. Each configured agent is an actor: it
owns a single inbox on a shared event bus, and runs its "brain" — the agent
runtime — for repeated iterations against that inbox. Everything the agent
touches (chat streams, tooling resources, timers, other agents) arrives at that
one inbox as a tagged event, so the brain is a pure, mostly-synchronous event
consumer.

This page explains why the model is shaped that way and how the pieces fit
together. The concrete interfaces are documented in [host](./host.md).

## The single inbox

Every agent gets exactly one inbox: a bounded channel on a shared message bus.
Nothing is routed to the brain directly — chat deltas, tool results, timers, and
messages from other agents all land in the same inbox as an
[`EventEnvelope`][events] carrying:

- `id` — the UUID of the _subscribed source_ the event came from.
- `event` — the strongly-typed payload (`message`, `error`, `timer`,
  `chat-delta`, `stream-end`, `resource-list-updated`, `resource-updated`).

The brain consumes events with `host.recv` (a blocking receive with a 60 second
host-side timeout) or `host.try-recv` (a non-blocking poll). Because every event
is tagged with a UUID, a single inbox is enough to multiplex many concurrent
sources — the brain correlates a delta or timer to the specific handle that
opened it by matching the envelope `id` against the UUID returned by the call
that created it.

## Nothing is shared by default

Inter-agent messaging is subscription-based. An agent does not receive anything
from another agent unless it explicitly subscribed:

- `host.subscribe(agent)` returns a new UUID handle for that source.
- `host.unsubscribe(uuid)` removes a subscription by handle.
- `host.send(agent, payload)` delivers the message only if the _recipient_
  subscribed to the _sender_. Each recipient's message is tagged with the UUID
  of _its own_ subscription to the sender, not a global topic, so a sender
  fanning out to many subscribers reaches each one through a distinct handle.

This keeps the coupling between actors explicit and auditable: an agent can only
ever be contacted by the actors it chose to listen to.

## I/O sources are pump tasks

Synchronous wasm brains cannot await async I/O directly. Instead, every
long-lived I/O source is driven by a _pump task_ spawned onto the agent's bridge
runtime (the `AgentContext.rt` tokio runtime), which pushes events into the
inbox:

- a `provider.chat` call spawns a [chat-stream pump][streams] that delivers
  `chat-delta` events as chunks arrive and a terminal `stream-end` (or `error`)
  event when the stream closes.
- a `tooling.subscribe-resource-list` / `subscribe-resource` call spawns a
  [resource pump][resources] that delivers `resource-list-updated` /
  `resource-updated` events.
- a `host.wait-timestamp` / `wait-duration` / `wait-cron` call schedules a
  [timer][time] that pushes a `Timer` event at the deadline.

Every pump holds a cancel signal keyed by its UUID handle: `provider.cancel`,
`host.cancel`, and the `tooling.unsubscribe-*` calls can drop it early, stopping
further deliveries before the source naturally ends.

Pull-style calls (`models`, `list-tools`, `call-tool`, `list-resources`) are far
shorter, so the host runs them to completion with `rt.block_on` instead of
spawning a pump. Both approaches run _off_ the wasm thread — pump tasks on the
tokio runtime, blocking calls on the `spawn_blocking` thread the engine runs on
— so the synchronous engine never blocks a tokio worker.

## Why it is shaped this way

Keeping a single inbox per agent means the brain's scheduling does not live in
the host. The agent decides, iteration by iteration, which events to handle and
in what order — the host just guarantees that everything relevant eventually
shows up, tagged, in order on one queue. That is what lets the brain (whether a
hand-written wasm component or a [rhai script](./runtime/rhai.md)) be written as
a plain sequential program over a stream of facts.

[events]: ./host.md#events
[streams]: ./provider/interface.md
[resources]: ./tooling/interface.md#subscriptions
[time]: ./host.md#timers
