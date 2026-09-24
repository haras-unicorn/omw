# Brain examples

Runnable agents that exercise the whole `omw` stack with no keys, no network,
and no external services. Each case is one shared `omw.test.template.toml` plus
a brain per variant under `rhai/`, `js/`, and `wasm/`, with a committed
`<variant>/omw.test.toml` generated from the template.

Run one case (every variant) or all of them:

```sh
omw-test run examples/01-hello
omw-test run examples
```

Each case is checked against the assertions in its config. See the
[examples guide](../docs/examples.md) for how the variants, templates, and
assertions work, and [Testing](../docs/testing/testing.md) for the binary.

- `01-hello` — provider wiring: one blocking `chat`.
- `02-tool-agent` — the ReAct tool round-trip: `chat` → `call_tool_blocking` →
  `chat`.
- `03-endpoint` — an OpenAI-compatible endpoint session: `subscribe_endpoint`,
  inbound `endpoint-message`, a reply streamed back with `stream_endpoint`.
- `04-ping-pong` — two agents on one shared brain, `subscribe_agent` /
  `send_agent` plus per-agent memory.
- `05-patterns` — assertion patterns: regex `detail` leaves, `$while`, `$until`,
  and a subsequence over a multi-turn provider script.
- `06-memory` — `[memory.alice]` seeds a value the brain reads and branches on.
- `07-asserted` — `outcome = "asserted"` stops a brain that would otherwise loop
  forever.
- `08-endpoint-order` — the endpoint mock's `after` gate asserts a request
  arrives at a specific point relative to the brain's calls.
- `09-resources` — the tooling mock's resource list and content: subscribe to
  both, read a resource, and react to scripted, `after`-gated updates.
