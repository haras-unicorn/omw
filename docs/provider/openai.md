# The OpenAI provider

The `openai` provider (`provider::openai`, kind `openai`) talks to any
OpenAI-family HTTPS endpoint that exposes the chat completions API, streaming
server-sent events (SSE). It is built on `reqwest` and requires no extra
services.

## Configuration

| key        | type   | default                     | meaning                                                     |
| ---------- | ------ | --------------------------- | ----------------------------------------------------------- |
| `base_url` | string | `https://api.openai.com/v1` | API base, before `/chat/completions` and `/models`          |
| `api_key`  | string | unset (no auth)             | sent as a `Bearer` token                                    |
| `model`    | string | unset                       | model returned when the endpoint's `list-models()` is empty |
| `params`   | table  | unset                       | default generation params merged into every request body    |

All keys are optional. The `api_key` is never logged: it is a `Secret` (locked
with `mlock`, zeroized on drop) that redacts on `Debug` and serialize, and `omw`
fails at startup if the lock cannot be taken.

`params` is an opaque table of generation settings — `temperature`,
`max_tokens`, `reasoning_effort`, `top_p`, `response_format`, and anything else
the endpoint accepts — merged into every request body. Per-call `params` from
the brain override these defaults, and the mandatory fields (`model`, `stream`,
`messages`, `tools`) always win over both.

```toml
[providers.openai]
kind = "openai"
api_key = "sk-…"          # usually sourced from your environment at runtime
model = "gpt-4o"
params = { temperature = 0.2, max_tokens = 1024, reasoning_effort = "high" }
```

`list-models()` asks `GET {base_url}/models` and **errors** if the request fails
(unreachable endpoint or non-2xx), so a brain calling it sees the failure. When
the endpoint answers with an empty list it falls back to the configured `model`.
The `omw scaffold` command treats a failure as an empty list.

## Reasoning and usage

The provider reads reasoning output from both spellings endpoints use —
`reasoning` and `reasoning_content` — surfacing it on each `chat-delta`'s
`reasoning` field and concatenated on `chat-result.reasoning`. A `reasoning`
field on an outgoing assistant message is sent back as `reasoning`. Token counts
are read from any chunk's `usage` (many endpoints only send it on the final
chunk, or after a `stream_options.include_usage` request); each delta and the
result carry a `usage` block with `prompt_tokens`, `completion_tokens`, and
`total_tokens` when reported.

## How a chat stream works

`chat` sends a `POST {base_url}/chat/completions` with `stream: true`, the
model, the conversation, the tools (when non-empty), and any configured/call
`params`. Non-2xx responses are returned as an error _before any delta_ —
satisfying the interface's streaming contract. On success the response body is
decoded line by line:

- lines are split on newlines and stripped of their `data:` prefix;
- a `[DONE]` marker ends the stream with a final `chat-end`,
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
