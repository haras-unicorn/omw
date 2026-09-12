# The OpenAI endpoint

The `openai` endpoint (`endpoint::openai`, kind `openai`) is a small axum server
exposing `/v1/models` and `POST /v1/chat/completions`. It is compiled behind the
`endpoint-openai` cargo feature, which is on by default; a build without it
fails at startup when `[endpoint]` names `kind = "openai"`.

## Configuration

| key      | type   | default | meaning                               |
| -------- | ------ | ------- | ------------------------------------- |
| `listen` | string | —       | socket address to listen on, required |

```toml
[endpoint]
kind = "openai"
listen = "127.0.0.1:8080"
```

The `listen` address is parsed as a plain socket address (numeric `host:port`),
so hostnames are rejected at startup. Set it to `"0.0.0.0:8080"` to serve all
interfaces. Only one listener is supported.

## The HTTP surface

`GET /v1/models` lists every currently subscribed model name, in sorted order.

`POST /v1/chat/completions` accepts an OpenAI-style request. `stream: true`
delivers SSE deltas ending with `data: [DONE]`; `stream: false` buffers deltas
into one JSON `chat.completion`. `tools` (OpenAI tool schema) is optional.
Errors — unknown roles, malformed tools, unknown models — are reported as OpenAI
`error` responses with matching status codes.
