# The OpenAI provider

The `openai` provider (`provider::openai`, kind `openai`) talks to any
OpenAI-family HTTPS endpoint that exposes the chat completions API, streaming
server-sent events (SSE). It is built on `reqwest` and requires no extra
services.

## Configuration

| key        | type   | default                     | meaning                              |
| ---------- | ------ | --------------------------- | ------------------------------------ |
| `base_url` | string | `https://api.openai.com/v1` | API base, before `/chat/completions` |
| `api_key`  | string | unset (no auth)             | sent as a `Bearer` token             |
| `model`    | string | unset                       | the model reported by `models()`     |

All keys are optional. The `api_key` is never logged: the config's value is
redacted in debug output and in the provider's `Debug` impl.

```toml
[providers.openai]
kind = "openai"
api_key = "sk-…"          # usually sourced from your environment at runtime
model = "gpt-4o"
```

## How a chat stream works

`chat` sends a `POST {base_url}/chat/completions` with `stream: true`, the
model, the conversation, and (when non-empty) the tools. Non-2xx responses are
returned as an error _before any delta_ — satisfying the interface's streaming
contract. On success the response body is decoded line by line:

- lines are split on newlines and stripped of their `data:` prefix;
- a `[DONE]` marker ends the stream with a final `stream-end`,
- each JSON chunk contributes one `delta` event.

### Tool-call reassembly

OpenAI streams tool-call arguments in fragments. The provider accumulates
`arguments` per tool-index and only surfaces a `tool-call` once both its `id`
and `name` are known; when all fragments have arrived it emits the fully
reassembled call. The lowest tool index is surfaced first, keeping order
deterministic.

## OpenAI vs. anything else

Because the interface is just "an OpenAI-family chat stream", `openai` is the
only provider compiled into the binary by default. Alternative endpoints with
the same wire shape work by pointing `base_url` at them; anything genuinely
different would be a new provider `kind`.
