# Provider mock

`kind = "mock"` is an in-process scripted chat provider. It records every `chat`
/ `chat-stream` invocation (model, messages, tools, and the opaque `params` the
brain passed) and returns scripted turns, so a brain's provider traffic is fully
deterministic and inspectable.

```toml
[providers.openai]
kind = "mock"
turns = [
  { content = "first reply" },
  { tool_call = { id = "call-1", name = "echo", arguments = '{"input":"hi"}' } },
  { reasoning = "let me think", content = "the answer", usage = { prompt_tokens = 12, completion_tokens = 5 } },
]
models = ["gpt-test"]
```

## Keys

- `turns` — the scripted turns, popped one per `chat`. A turn is a table with
  any of:
  - `content = "..."` — text; the mock emits it and a terminal `stop` finish
    reason.
  - `reasoning = "..."` — reasoning/thinking text; emitted as its own delta
    before the content/tool-call delta.
  - `tool_call = { id, name, arguments }` — a tool call; the mock emits the call
    and a terminal `tool_calls` finish reason. `arguments` is the raw JSON
    string the model would have produced. Write it as a TOML string
    (`arguments = '{"input":"hi"}'`) or, more readably, as the inline JSON value
    (`arguments = { input = "hi" }`); an inline value is stringified when the
    mock is built, so the guest always sees the wire string.
  - `usage = { prompt_tokens = 12, completion_tokens = 5, total_tokens = 17 }` —
    token counts attached to the turn's terminal delta (every key optional).
  - `error = "..."` — emit the turn's deltas, then fail the stream with this
    error (the brain sees a provider error mid-stream).
  - `pending = true` — the stream never yields; it stays pending forever, for
    exercising cancellation paths. The turn returns no deltas, so it records
    none.
  - Once the script is exhausted the **last turn repeats** for every further
    chat, so a looping brain keeps working without re-listing the script.
- `models` — the model names `list-models` returns. Defaults to
  `["mock-model"]`.

A bare `kind = "mock"` with no `turns` emits an empty stream, which is useful
for tests that only assert the call itself. Because the recorded call includes
`params`, an assertion can pin a brain's per-call generation settings:

```toml
[assertions.alice]
events = [
  { kind = "call", op = "chat", detail = { params = { temperature = 0.2 } } },
]
```

The `--dump` snapshot also lists every call's scripted `deltas` alongside the
request, so a dump shows exactly what the brain received back.
