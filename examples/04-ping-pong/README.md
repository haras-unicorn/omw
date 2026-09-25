# 04-ping-pong

Two agents (`alice` and `bob`) run one shared brain script and no provider or
tooling, proving the actor model: explicit `subscribe_agent` subscriptions,
tagged inbox messages, per-agent memory, and `whoami`.

`whoami` tells each agent its own name, so it subscribes only to itself and
plays a deterministic ping-pong with its own inbox: send `ping`, receive it,
send `pong`, receive it. The subscription handle goes to memory so a hot reload
could reuse it.

```sh
omw-test run examples/04-ping-pong
```

It asserts the same stream for both agents: a `whoami` call, one
`subscribe_agent` call, one `memory_set` call, then a `send_agent` / inbound
`message` pair, another `send_agent` / inbound `message` pair, and an
`unsubscribe_agent` call, all with a `completed` outcome. See the
[examples guide](../../docs/examples.md) for how the variants and assertions
work.
