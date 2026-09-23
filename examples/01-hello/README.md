# 01-hello

The smallest agent: one blocking `chat` against the scripted mock provider. The
reply comes back in-band, so the only thing the trace records is the `chat`
call.

```sh
omw-test run examples/01-hello
```

It asserts a single `call(chat)` and a `completed` outcome. See the
[examples guide](../../docs/examples.md) for how the variants and assertions
work.
