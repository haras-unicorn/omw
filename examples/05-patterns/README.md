# 05-patterns

A brain that chats four times with different model names, against a three-turn
provider script. The point is the assertions, not the brain: they use regex
`detail` leaves, `$any`, and `$skip` to check only the interesting chats out of
a longer stream.

```sh
omw-test run examples/05-patterns
```

It asserts the first `chat` with a model matching `^gpt-a.*`, consumes the next
two calls with `$any` and `$skip`, then matches the final `chat` again — a
subsequence over the trace. See the [examples guide](../../docs/examples.md) for
how the variants and assertions work.
