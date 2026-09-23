# Provider mock

`kind = "mock"` is an in-process scripted chat provider. It records every `chat`
/ `chat-stream` invocation (model, messages, tools) and returns scripted turns,
so a brain's provider traffic is fully deterministic and inspectable.

```toml
[providers.openai]
kind = "mock"
turns = [
  { content = "first reply" },
  { tool_call = { id = "call-1", name = "echo", arguments = '{"input":"hi"}' } },
]
models = ["gpt-test"]
```

## Keys

- `turns` — the scripted turns, popped one per `chat`. A turn is either:
  - `{ content = "..." }` — a plain content turn; the mock emits it and a
    terminal `stop` finish reason.
  - `{ tool_call = { id, name, arguments } }` — a tool-call turn; the mock emits
    the call and a terminal `tool_calls` finish reason. `arguments` is the raw
    JSON string the model would have produced. Write it as a TOML string
    (`arguments = '{"input":"hi"}'`) or, more readably, as the inline JSON value
    (`arguments = { input = "hi" }`); an inline value is stringified when the
    mock is built, so the guest always sees the wire string.
  - Once the script is exhausted the **last turn repeats** for every further
    chat, so a looping brain keeps working without re-listing the script.
- `models` — the model names `list-models` returns. Defaults to
  `["mock-model"]`.

A bare `kind = "mock"` with no `turns` emits an empty stream, which is useful
for tests that only assert the call itself.
