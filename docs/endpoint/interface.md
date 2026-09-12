# The endpoint interface

An _endpoint_ is an abstraction over an inbound server: something agents
subscribe to under model names, and which routes outside requests into their
inboxes. Unlike providers, tooling, and runtimes there is at most one endpoint
per process, and it is optional: the `[endpoint]` config block holds a `kind`
string plus opaque params, exactly like a single `[providers.<name>]` entry.
When `[endpoint]` is absent no server is started, and `subscribe-endpoint`
simply errors.

## The abstraction

An endpoint exposes, through the `endpoint::Endpoint` trait:

- `kind()` — which implementation this is (e.g. `openai`).
- `serve(bus, registry)` — own the serve loop: accept outside requests, open a
  session in the shared `EndpointRegistry`, and route each request as an
  `endpoint-message` inbox event.

`build(kind, params)` dispatches on `kind` to `openai` (behind the
`endpoint-openai` feature) and bails on anything else. The transport state
itself stays outside the trait: `host/bus.rs` (`endpoint_subscribe`,
`endpoint_route`, `endpoint_models`) owns model-to-agent routing, and
`host/endpoint.rs` (`EndpointRegistry` `open` / `push` / `abort`) owns the
per-session buffers — so a new transport only implements `serve`, never the
inbox protocol.

## Configuration

```toml
[endpoint]
kind = "openai"
listen = "127.0.0.1:8080"
```

The endpoint is optional and disabled unless `[endpoint]` is set. The remaining
keys are opaque to the config layer and validated by the implementation at
construction time; see [the OpenAI endpoint](./openai.md) for the `openai` keys.
A build without the matching feature fails at startup when `[endpoint]` names
its `kind`.

## The agent side

An agent becomes a model by subscribing: `host.subscribe-endpoint(model)`
returns a UUID handle, lists the model on the endpoint, and routes inbound
requests as `endpoint-message` events tagged with it; an agent may subscribe
many names, and `host.unsubscribe-endpoint(uuid)` drops a model and abruptly
ends every in-flight session of that subscription. The agent streams deltas back
with `host.stream-endpoint(session, delta)`, non-blocking; a delta carrying a
`finish-reason` ends the session. Sessions end exactly once: normally when the
reply completes, or abruptly on client disconnect or unsubscribe. Each end fires
exactly one `endpoint-session-end` event into the owning agent's inbox. Errors
on unknown, foreign-agent, or ended sessions are reported in-band.

## Session lifecycle

The session buffer (see `session_buffer` in [tunables](../tunables.md)) holds
deltas before further chunks are dropped with a `tracing::warn` — an emergency
lane, not a throttle. A terminal `finish-reason` delta is queued first, then the
session entry is removed, then a `Close` marker, and exactly one
`endpoint-session-end` fires. The host fns and events are documented in
[host](../host.md).
