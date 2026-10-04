# 10-endpoint-stop

Endpoint session lifecycle. An OpenAI-compatible endpoint agent subscribes under
`gpt-4o`, streams one content delta back per request, and then reacts to the
session ending rather than sending a terminal `finish_reason` itself. The mock
client ends two sessions from the client side: the first stops normally
(`session_end = { close = 1 }`), the second is aborted
(`session_end = { abort = 1 }`), so the brain sees an `endpoint-session-end`
inbound event for each — normal (no `error`) then abrupt
(`error = "endpoint session aborted"`).

```sh
omw-test run examples/10-endpoint-stop
```

It asserts `subscribe_endpoint` (and its `opened` lifecycle), two rounds of
inbound `endpoint-message` → `chat` → `stream_endpoint`, an
`endpoint-session-end` after each (the second carrying the abort error), then
`unsubscribe_endpoint` (and its `closed` lifecycle) and a `completed` outcome.
See the [examples guide](../../docs/examples.md) for how the variants and
assertions work.
