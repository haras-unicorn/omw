# Examples

Planned layout for runnable examples. Not yet implemented.

## Layout

```text
src/lib/omw/examples/          # Cargo examples (auto-discovered)
  embed_with_defaults.rs
  custom_provider.rs
  custom_tooling.rs
  custom_runtime.rs
  custom_endpoint.rs

examples/                      # brain examples (not a workspace member)
  01-hello/   brain.rhai brain.js rust/ omw.rhai.toml omw.js.toml omw.wasm.toml
  02-tool-agent/  same shape
  03-endpoint/    same shape
  04-ping-pong/   same shape (agents: alice, bob)
```

Each `examples/<name>/` gets a `README.md` with run lines (`omw run --config`,
`omw-rhai` / `omw-js` flavor notes). `docs/library.md` still says runnable
examples land in a separate PR; remove that stub when these land.

## Constraint: `mock` is test-only

`provider/mock.rs` and `tooling/mock.rs` are `pub(crate)` behind the test-only
`mock` feature, and `src/wasm/omw-wasm-mock` is `publish = false`. Brain
examples therefore cannot use `kind = "mock"`.

Plan: keep the test mocks private. Brain TOMLs use only built-in kinds
(`openai` + `mcp/http`) pointed at localhost doubles, so they run under the
stock `omw` binary. The `tests/examples.rs` harness loads the same on-disk brain
scripts but constructs the `Config` programmatically, overriding `base_url` /
`url` with the ephemeral wiremock / in-process rmcp URLs. The `custom_provider`
/ `custom_tooling` library examples stay library-only teaching material; they
are not referenced from brain TOMLs.

## Brain examples (identical x3: rust / js / rhai)

API map to preserve per language:

| op       | rust (`omw-wasm-rust`)                 | rhai (snake_case maps)              | js (camelCase global)             |
| -------- | -------------------------------------- | ----------------------------------- | --------------------------------- |
| chat     | `Provider::get("openai")?.chat(...)`   | `omw::provider::get("openai").chat` | `omw.provider.get("openai").chat` |
| stream   | `chat_stream` + `StreamGuard` + `recv` | `chat_stream` + uuid + `recv`       | `chatStream` + uuid + `recv`      |
| tool     | `call_tool_blocking(name, json)`       | `call_tool_blocking(name, #{...})`  | `callToolBlocking(name, {...})`   |
| envelope | `EventEnvelope{id, event}`             | `#{id, kind, payload}`              | `{id, kind, payload}`             |

1. **01-hello** — blocking
   `chat("gpt-test", [{ role: user, content: "say hi" }], [])`; terminal message
   is the reply text. Proves provider wiring in ~10 lines. The shipped TOML
   points `base_url` at localhost with a placeholder key; the test overrides it
   with the wiremock URL.

2. **02-tool-agent** — ReAct loop: `chat` returns
   `tool_calls[0] { echo, input: "hi" }` → `call_tool_blocking("echo", ...)` →
   final `chat` with the tool result → terminal `"<chat>|<tool>"`. Mirrors
   `tests/agents.rs:142-160` and `runtime/rhai.rs:306-319` / `js.rs:306-319`.
   Proves the tool round-trip offline.

3. **03-endpoint-server** — `subscribe_endpoint("gpt-4o")` + `recv()` loop on
   `endpoint-message` → `chat` the inbound messages →
   `stream_endpoint(session, { content })` + `{ finish_reason: "stop" }`.
   Mirrors `rhai.rs:662-670,752-756`. TOML adds
   `[endpoint] kind = "openai" listen = "127.0.0.1:8080"`; test with
   `curl /v1/chat/completions`.

4. **04-ping-pong** — no provider / tooling: alice `subscribe_agent("bob")` +
   `send_agent("bob", "ping")`, bob replies `"pong"`; both `memory_set` handles
   for reload survival. Mirrors `rhai.rs:454-505` and
   `docs/hot-reload.md:104-142`. Two `[[agents]]` in one TOML; proves the actor
   model plus `--watch` (`reload` / `shutdown` via `subscribe_lifecycle`).

Rust brains use `brain!(|| { ... Ok(()) })` (`#![no_main]`, `omw-wasm-rust` dep,
`cargo build --target wasm32-wasip2`), `script = "brain.wasm"`,
`[runtime.wasm] kind = "wasm"`. Rhai / JS brains use `script = "brain.rhai"` /
`"brain.js"`.

## Library examples (`src/lib/omw/examples/*.rs`)

- `embed_with_defaults.rs` — `toml::from_str::<Config>` (add `toml` to
  dev-deps) + the TLS `OnceLock` snippet + `Registries::default()`
  - `run_agents(cfg, false, ...)`. Runnable version of the `library.md:50-61`
    snippet.
- `custom_provider.rs` — `struct EchoProvider` + `impl Provider` +
  `impl provider::Factory` + `register_providers!`, TOML
  `[providers.echo] kind = "echo"`. Lifts `library.md:82-134`.
- `custom_tooling.rs` — in-memory `Tooling`
  (`Factory::build(name, params, tunables)`); lazy-connect notes.
- `custom_runtime.rs` — pure-Rust inline `Runtime` (`run(ctx) -> RunOutcome`,
  `validate(ctx)`) bypassing the WASM engine; the escape hatch.
- `custom_endpoint.rs` — minimal `Endpoint::serve(bus, registry, shutdown)` stub
  showing `shutdown.wait()` + the session registry.

Gate with `[[example]] required-features` so `--no-default-features` customs
build TLS-free while the defaults example requires `provider-openai` etc. Keep
repo style: 2-space, 80-col, `anyhow::Result`, no `unwrap`.

## Testing strategy

Two kinds, two harnesses. No NixOS VMs, no containers, no new env var: there is
no `OMW_TEST_EXAMPLES`. The harness is in-process only (wiremock provider,
in-process rmcp streamable-HTTP echo server, localhost endpoint client), so
everything except the rust `.wasm` builds is `dev test fast`-safe and stays
always-on. No testcontainers, no keys, no network.

### Library examples

Compile-covered for free: `cargo clippy --all-features` and
`cargo test --all-features` both build everything under `src/lib/omw/examples/`.
Each example is also self-running via `dev lib example <example>` and exits 0 on
a programmatic `Config` plus custom registries — no keys, no network, no
services. A `custom_*` example asserts on the recorded calls (model/messages for
providers, name/arguments for tooling) before exiting, so a successful run is
the test.

### Brain examples

Each brain example is also self-running via
`dev brain example <example> <flavor>` (e.g. `dev brain example 01-hello rhai`),
which sets `OMW_TEST_EXAMPLE_FILTER` to that `<example>/<flavor>` pair.

`dev examples` lists and runs every example one by one: each
`src/lib/omw/examples/*.rs` via `cargo run -p omw --example`, then each
`examples/<name>/` flavor (`rhai`, `js`, `wasm`) via the `tests/examples.rs`
harness with `OMW_TEST_EXAMPLE_FILTER` pinned to that pair. `dev lint` runs it
right before `nix flake check`.

One integration test file, `src/lib/omw/tests/examples.rs`, drives the real
example files on disk (`examples/<name>/brain.rhai`, `brain.js`, and the
compiled rust `brain.wasm`) through `run_agents` once, using the same in-process
harness as `tests/agents.rs`: a wiremock OpenAI provider and an in-process rmcp
streamable-HTTP echo server. Each test asserts the terminal outcome and the
recorded calls, proving the three language variants stay functionally identical.
Variants that need more than provider/tooling extend the harness: 03 adds a
localhost endpoint client (`POST /v1/chat/completions`, streamed and buffered),
04 runs two agents with no provider or tooling at all.

Only the rust `.wasm` variant cases are heavy (nested
`cargo build --target wasm32-wasip2` + `wasm-tools`, like `build.rs` does), so
only those cases skip behind the existing `OMW_TEST_WASM_RUNTIME_NON_NATIVE` var
— the same gate the engine and `rhai.rs` / `js.rs` unit tests already use. No
`dev.nix` churn, no new var; `dev test fast` still covers the rhai/js variants
and all five library examples.

## Implementation plan

Do one step at a time, in order. Each step is one example; brains ship all three
variants (rhai, js, rust) plus TOMLs and a README in the same step. Library
steps add one file under `src/lib/omw/examples/`.

1. [x] `embed_with_defaults.rs` — file-loaded `Config` (`toml`), TLS `OnceLock`
       snippet, `Registries::default()`, `run_agents`. Needs `toml` in dev-deps.
       Test: `dev lib example embed_with_defaults` exits 0 on its inline TOML.
2. [x] `custom_provider.rs` — scripted `EchoProvider` (`responses` streaming
       `ChatDelta`) + `Factory` + `register_providers!` + `[providers.echo]`
       TOML. Library-only teaching material (brains use built-in kinds, not
       this). Test: `dev lib example custom_provider` runs `chat`/`chat_stream`,
       asserts the recorded model/messages, exits 0.
3. [x] `custom_tooling.rs` — in-memory echo tool (canned `value`, records
       `ToolCall`) + `Factory::build(name, params, tunables)` + `[tooling.echo]`
       TOML. Library-only; not referenced from brain TOMLs. Test:
       `dev lib example custom_tooling` runs `call_tool`, asserts the recorded
       name/arguments, exits 0.
4. [ ] `examples/01-hello/` — blocking `chat`, terminal message is the reply.
       Files: `brain.rhai`, `brain.js`, `rust/`, `omw.*.toml`, `README.md`.
       Shipped TOMLs use built-in kinds pointed at localhost; the test builds
       the `Config` programmatically and overrides `base_url` with the wiremock
       URL. Test: `dev brain example 01-hello <flavor>` (the `tests/examples.rs`
       harness discovers `examples/*/` at runtime, so no harness change is
       needed) drives the on-disk brain through `run_agents` and asserts the
       reply text. Only the rust `.wasm` case needs
       `OMW_TEST_WASM_RUNTIME_NON_NATIVE` (nested `wasm32-wasip2` build);
       rhai/js always run.
5. [ ] `examples/02-tool-agent/` — `chat` → `call_tool_blocking` → final `chat`
       → `"<chat>|<tool>"`. Same file shape as 01. Test:
       `dev brain example   02-tool-agent <flavor>` (one case per variant)
       against the wiremock provider plus the in-process echo MCP server
       (`mcp/http` + ephemeral `url`); asserts `"<chat>|<tool>"`. Same `.wasm`
       gate as 01.
6. [ ] `examples/03-endpoint/` — `subscribe_endpoint` + `recv()` loop +
       `stream_endpoint` with terminal `finish_reason`. TOML adds `[endpoint]`;
       README shows the `curl` call. Same file shape. Test:
       `dev brain example   03-endpoint <flavor>` (one case per variant); the
       test is the endpoint client (`POST /v1/chat/completions`, one streamed +
       one buffered call). Same `.wasm` gate as 01.
7. [ ] `examples/04-ping-pong/` — alice/bob `subscribe_agent` / `send_agent` +
       `memory_set` handles, no provider or tooling. Two `[[agents]]`, one TOML
       per runtime. Same file shape. Test:
       `dev brain example 04-ping-pong   <flavor>` (one case per variant pair);
       `run_agents` completes and both agents exchanged ping/pong. Same `.wasm`
       gate as 01.
8. [x] `custom_runtime.rs` — pure-Rust inline `Runtime` (`run` / `validate`)
       bypassing the WASM engine. Test: `dev lib example custom_runtime` runs an
       agent through `run_agents` and asserts `Completed`.
9. [x] `custom_endpoint.rs` — minimal `Endpoint::serve` stub
       (`shutdown.wait()` + session registry). Test:
       `dev lib example   custom_endpoint` boots `run_agents` with the stub
       endpoint and exits 0.
10. [ ] Docs pass — `library.md`, `runtime/{rhai,js,wasm}.md`,
        `examples/README.md`, stub removal. Test: `dev lint` (prettier, cspell,
        markdownlint, link check, `dev examples`, `nix flake check`).
