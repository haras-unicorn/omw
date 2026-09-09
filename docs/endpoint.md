# The endpoint

`omw` ships an optional OpenAI-compatible HTTP endpoint: a small axum server
exposing `/v1/models` and `POST /v1/chat/completions`. Each agent subscribes
itself to the endpoint under one or more model names with
`host.endpoint-subscribe`; a chat request arrives in the agent's inbox as an
`endpoint-message` event, and the agent streams its reply back with
`host.endpoint-stream` — SSE chunks for streaming clients, or one buffered JSON
completion otherwise. This makes an `omw` agent drivable by any
OpenAI-compatible client.

## Configuration

```toml
[endpoint]
listen = "127.0.0.1:8080"
```

The endpoint is optional and disabled unless `[endpoint]` is set. The `listen`
address is parsed as a plain socket address (numeric `host:port`), so hostnames
are rejected at startup. Set it to `"0.0.0.0:8080"` to serve all interfaces.
Only one listener is supported.

## The agent side

An agent becomes a model by subscribing: `host.endpoint-subscribe(model)`
returns a UUID handle, lists the model on `/v1/models`, and routes inbound
requests as `endpoint-message` events tagged with it; an agent may subscribe
many names, and `host.endpoint-unsubscribe(uuid)` drops a model and abruptly
ends every in-flight session of that subscription. The agent streams deltas back
with `host.endpoint-stream(session, delta)`, non-blocking; a delta carrying a
`finish-reason` ends the session. Sessions end exactly once: normally when the
reply completes, or abruptly on client disconnect or unsubscribe. Each end fires
exactly one `endpoint-session-end` event into the owning agent's inbox. Errors
on unknown, foreign-agent, or ended sessions are reported in-band.

## The HTTP surface

`GET /v1/models` lists every currently subscribed model name, in sorted order.

`POST /v1/chat/completions` accepts an OpenAI-style request. `stream: true`
delivers SSE deltas ending with `data: [DONE]`; `stream: false` buffers deltas
into one JSON `chat.completion`. `tools` (OpenAI tool schema) is optional.
Errors — unknown roles, malformed tools, unknown models — are reported as OpenAI
`error` responses with matching status codes.

## Session lifecycle

The session buffer (see `session_buffer` in [tunables](./tunables.md)) holds
deltas before further chunks are dropped with a `tracing::warn` — an emergency
lane, not a throttle. A terminal `finish-reason` delta is queued first, then the
session entry is removed, then a `Close` marker, and exactly one
`endpoint-session-end` fires. The host fns and events are documented in
[host](./host.md).
