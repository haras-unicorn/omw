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
- `models()` — the model names this provider exposes.
- `chat(model, messages, tools)` — open a _streaming_ chat response. Returns a
  UUID handle; deltas flow into the agent's inbox as `delta` events until a
  terminal `stream-end` (or `error`) event closes the stream.
- `is-open(uuid)` — whether a chat stream identified by `uuid` is still open.
- `cancel(uuid)` — cancel an open stream by `uuid`.

The handle is obtained once with `provider.get(name)`, which returns a
`provider` resource; all further calls go through that handle so the guest never
repeats the name.

## The streaming contract

`chat` is the only long-running call, and it drives the two shapes of the host
bridge at once:

- **Open and return.** The call starts a chat-stream pump on the bridge runtime
  and returns the stream's UUID immediately; it does not block the brain.
- **Consume in the inbox.** The pump delivers each chunk as a `delta` event,
  then a `stream-end` event, all tagged with the returned UUID. The brain
  collects them with `recv`/`try-recv`.

Two contracts matter when writing or using a provider implementation:

- Implementations must return an **error before the first delta** on transport
  or authentication failure, rather than a silent empty stream — the guest sees
  the failure as an `error` event instead of a misleading `stream-end`.
- **Dropping the returned stream aborts the in-flight request**, unsubscribing
  any pump reading it. This is how cancellation is granted for free.
