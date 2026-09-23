# 04-ping-pong

Two agents (`alice` and `bob`) run one shared brain script and no provider or
tooling, proving the actor model: explicit `subscribe_agent` subscriptions,
tagged inbox messages, and per-agent memory.

Because the script cannot tell which name is its own, each agent subscribes to
both names and pings both. The self-addressed ping is always delivered — the
agent subscribed to its own name first — so exactly one `recv` succeeds. The
subscription handles go to memory so a hot reload could reuse them.

```sh
omw-test run examples/04-ping-pong
```

It asserts the same stream for both agents: two `subscribe_agent` calls, two
`memory_set` calls, two `send_agent` calls, one inbound `message`, two more
`send_agent` calls, two `unsubscribe_agent` calls, and a `completed` outcome.
See the [examples guide](../../docs/examples.md) for how the variants and
assertions work.
