# Examples

The `examples/` directory holds runnable agents that exercise the whole `omw`
stack with no keys, no network and no external services. Each case is a shared
`omw.test.template.toml` plus a brain per variant under `rhai/`, `js/` and
`wasm/`, and a committed `<variant>/omw.test.toml` generated from the template.

They double as documentation: read a case's `omw.test.toml` to see what a brain
does, and its `brain.rhai` / `brain.js` / `brain.rs` to see how to write it.

## Running

Run one case (every variant) or all of them:

```sh
omw-test run examples/01-hello
omw-test run examples
```

Each case is checked against the assertions in its config, so a green run means
the brain's provider calls, inbox events and terminal outcome match what the
case claims. See [Testing](./testing/testing.md) for the binary and the
assertion language.

The `wasm` cells need their `brain.wasm`, which the repo's dev shell builds for
you through `dev test`; to build one cell by hand, see
[Rust brains](./runtime/wasm.md#rust-brains). The `dev` wrapper runs every cell
(`dev test brain examples`) and gates the `wasm` ones on the
`OMW_TEST_WASM_RUNTIME_NON_NATIVE` environment variable.

## Variants and templates

A case is one config shared across three languages. The template uses two
placeholders — `{{RUNTIME}}` (the runtime `kind`) and `{{SCRIPT}}` (the sibling
brain file) — and names its runtime `runtime`:

```toml
[runtime.runtime]
kind = "{{RUNTIME}}"

[[agents]]
name = "alice"
runtime = "runtime"
script = "{{SCRIPT}}"
```

`dev format` expands each template into its per-variant `omw.test.toml` (only
for variants whose brain file exists) and commits the result; `dev lint`
regenerates and compares, failing when a committed config is stale. You can
regenerate by hand with `omw generate test config <case> <variant>`.

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
- `05-patterns` — assertion patterns: regex `detail` leaves, `$any`, `$skip`,
  and a subsequence over a multi-turn provider script.
- `06-memory` — `[memory.alice]` seeds a value the brain reads and branches on.
- `07-asserted` — `outcome = "asserted"` stops a brain that would otherwise loop
  forever.
- `08-endpoint-order` — the endpoint mock's `after` gate asserts a request
  arrives at a specific point relative to the brain's calls.
- `09-resources` — the tooling mock's resource list and content: subscribe to
  both, read a resource, and react to scripted, `after`-gated updates.
