# 03-endpoint

An OpenAI-compatible endpoint agent. It subscribes under the model `gpt-4o`,
then answers each inbound request by chatting with the mock provider and
streaming the reply back as two deltas: content, then a terminal
`finish_reason`. The mock client plays two requests, one streamed and one
buffered, so no socket is involved.

```sh
omw-test run examples/03-endpoint
```

It asserts `subscribe_endpoint`, two inbound `endpoint-message` rounds each
followed by `chat` and two `stream_endpoint` calls, then `unsubscribe_endpoint`
and a `completed` outcome. See the [examples guide](../../docs/examples.md) for
how the variants and assertions work.
