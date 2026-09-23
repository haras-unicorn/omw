# 07-asserted

A brain that never exits on its own: it subscribes to itself, pings itself, then
loops on `recv` forever. The case's `outcome = "asserted"` stops it as soon as
its events settle, so the test checks a prefix of an endless run.

```sh
omw-test run examples/07-asserted
```

It asserts `subscribe_agent`, `send_agent`, and one inbound `message`, then
stops the agent. See the [examples guide](../../docs/examples.md) for how the
variants and assertions work.
