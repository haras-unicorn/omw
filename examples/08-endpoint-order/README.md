# 08-endpoint-order

An endpoint agent whose request is gated on the trace. The mock client's request
carries `after = { kind = "call", op = "chat" }`, so it is routed only once the
brain has made its warm-up `chat`; the brain then answers it.

```sh
omw-test run examples/08-endpoint-order
```

It asserts the order `subscribe_endpoint` → `chat` → inbound `endpoint-message`
→ two `stream_endpoint` calls → `unsubscribe_endpoint`, proving the request
arrived at the intended point relative to the brain's work. See the
[examples guide](../../docs/examples.md) for how the variants and assertions
work.
