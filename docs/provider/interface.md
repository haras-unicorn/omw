# The provider interface

A _provider_ is an abstraction over an OpenAI-family chat service: something you
hand a model, a conversation, and (optionally) the tool signatures the model may
call, and which streams back deltas. Named providers live in the global
`[providers.<name>]` config map and are looked up at runtime by name.

## The abstraction

A provider exposes, through the WIT `provider` interface:

- `kind()` — which implementation this is (e.g. `openai`), letting a guest break
  the abstraction when it chooses to.
- `name()` — the configured name of the instance.
- `list-models()` — the model names this provider exposes; errors if they cannot
  be enumerated.
- `chat(model, messages, tools, params)` — run a chat conversation to
  completion, and return the full [`chat-result`] in-band: the concatenated
  content, the reassembled tool calls, the terminal finish reason, the
  concatenated reasoning/thinking content, and token `usage`. No events are
  delivered; the call blocks the brain until it finishes or errors, and cannot
  be cancelled.
- `chat-stream(model, messages, tools, params)` — open a _streaming_ chat
  response. Returns a UUID handle; deltas flow into the agent's inbox as
  `chat-delta` events until a terminal `chat-end` (or `error`) event closes the
  stream.
- `is-open(uuid)` — whether a chat stream identified by `uuid` is still open.
- `cancel(uuid)` — cancel an open stream by `uuid`.

`params` is an optional opaque JSON object of generation settings
(`temperature`, `max_tokens`, `reasoning_effort`, `response_format`, …). It is
forwarded to the provider and merged over the provider's configured defaults, so
a brain can set per-call settings without a config change. An implementation
ignores keys it does not understand, and the mandatory request fields (`model`,
`stream`, `messages`, `tools`) always win.

The handle is obtained once with `provider.get(name)`, which returns a
`provider` resource; all further calls go through that handle so the guest never
repeats the name.

## Reasoning and usage

Reasoning models stream a second channel of output alongside the answer. `omw`
carries it explicitly:

- `chat-message` has a `reasoning` field. A brain can echo an assistant
  message's reasoning back to the provider on a later turn (some models require
  this for multi-turn tool use).
- `chat-delta` has a `reasoning` field for each streamed reasoning chunk, plus a
  `usage` field (many providers report token counts only on the final chunk).
- `chat-result` concatenates `reasoning` and carries the response's `usage`.

`usage` reports `prompt-tokens`, `completion-tokens`, and `total-tokens`, each
optional because not every provider reports every count.

## The streaming contract

`chat-stream` is the only long-running call, and it drives the two shapes of the
host bridge at once:

- **Open and return.** The call starts a chat-stream pump on the bridge runtime
  and returns the stream's UUID immediately; it does not block the brain.
- **Consume in the inbox.** The pump delivers each chunk as a `chat-delta`
  event, then a `chat-end` event, all tagged with the returned UUID. The brain
  collects them with `recv`/`try-recv`.

Two contracts matter when writing or using a provider implementation:

- Implementations must return an **error before the first delta** on transport
  or authentication failure, rather than a silent empty stream — the guest sees
  the failure as an `error` event instead of a misleading `chat-end`.
- **Dropping the returned stream aborts the in-flight request**, unsubscribing
  any pump reading it. This is how cancellation is granted for free.

## The blocking call

`chat` is the in-band counterpart of `chat-stream`: it runs the same
conversation, but collects every delta to completion itself, and returns the
accumulated result instead of delivering events. A brain that wants a simple
round-trip without stream bookkeeping can use `chat` and read
`content`/`reasoning`/`tool_calls`/`finish_reason`/`usage` off the returned
[`chat-result`].
