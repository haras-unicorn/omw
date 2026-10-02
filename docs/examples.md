# Examples

The `examples/` directory holds runnable agents that exercise the whole `omw`
stack with no keys, no network and no external services. Each case is one shared
`omw.test.base.toml` plus a brain per variant under `rhai/`, `js` and `wasm/`,
and a committed `<variant>/omw.test.toml` generated from the base config.

They double as documentation: read a case's `omw.test.base.toml` to see what a
brain does, its shrunk `<variant>/omw.test.toml` to see the variant wiring, and
its `brain.rhai` / `brain.js` / `brain.rs` to see how to write it.

## Running

Run one case (every variant) or all of them:

```sh
omw-test run examples/01-hello
omw-test run examples      # every case, excluding the wasm cells
dev test brain examples    # every cell
```

A test config automatically inherits the `omw.test.base.toml` in its case
directory, so `omw-test run examples/01-hello` picks up the shared wiring and
assertions with no flags. Each case is checked against the assertions in its
base config, so a green run means the brain's provider calls, inbox events and
terminal outcome match what the case claims. See [Testing](./testing/testing.md)
for the binary, the assertion language and
[base configs](./testing/testing.md#base-configs).

The `wasm` cells need their `brain.wasm`, which the repo's dev shell builds for
you through `dev test`; to build one cell by hand, see
[Rust brains](./runtime/wasm.md#rust-brains). The `dev` wrapper runs every cell
(`dev test brain examples`) and gates the `wasm` ones on the
`OMW_TEST_WASM_RUNTIME_NON_NATIVE` environment variable.

## Variants and base configs

A case is one config shared across three languages. The base config carries the
shared wiring — provider/tooling/endpoint, each agent's `runtime`, the
`[memory]` seeds and the `[assertions]` — while each variant's `omw.test.toml`
carries only the runtime `kind` and the sibling brain file:

```toml
# omw.test.base.toml
[agents.alice]
runtime = "runtime"

[assertions.alice]
outcome = "completed"
events = [{ kind = "call", op = "chat" }]
```

```toml
# <variant>/omw.test.toml
[runtime.runtime]
kind = "rhai"

[agents.alice]
script = "brain.rhai"
```

`dev format` regenerates each per-variant `omw.test.toml` from the case's base
config (only for variants whose brain file exists) and commits the result;
`dev lint` regenerates and compares, failing when a committed config is stale.

## Assertions

Each case carries an `[assertions.<agent>]` block describing the calls and inbox
events that agent should see, in order, plus its outcome. Assertions are an
ordered subsequence over partial patterns, so a case pins only the interesting
slice of the trace. See [Testing](./testing/testing.md#assertions) for the full
language.

## The cases

- `01-hello` — provider wiring: one blocking `chat`.
- `02-tool-agent` — the ReAct tool round-trip: `chat` → `call_tool_blocking` →
  `chat`.
- `03-endpoint` — an OpenAI-compatible endpoint session: `subscribe_endpoint`,
  an inbound `endpoint-message`, a reply streamed back with `stream_endpoint`.
- `04-ping-pong` — two agents on one shared brain, `subscribe_agent` /
  `send_agent` plus per-agent memory.
- `05-patterns` — assertion patterns: regex `detail` leaves, `$while`, `$until`,
  and a subsequence over a multi-turn provider script.
- `06-memory` — `[memory.alice]` seeds a table the brain reads back parsed with
  `memory_get_as` and branches on.
- `07-asserted` — `outcome = "asserted"` stops a brain that would otherwise loop
  forever.
- `08-endpoint-order` — the endpoint mock's `after` gate asserts a request
  arrives at a specific point relative to the brain's calls.
- `09-resources` — the tooling mock's resource list and content: subscribe to
  both, read a resource, and react to scripted, `after`-gated updates.
