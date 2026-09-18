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

Each `examples/<name>/` gets a `README.md` with run lines
(`omw run --config`, `omw-rhai` / `omw-js` flavor notes).
`docs/library.md` still says runnable examples land in a separate PR;
remove that stub when these land.

## Constraint: `mock` is test-only

`provider/mock.rs` and `tooling/mock.rs` are `pub(crate)` behind the
test-only `mock` feature, and `src/wasm/omw-wasm-mock` is
`publish = false`. Brain examples therefore cannot use `kind = "mock"`.

Plan: keep the test mocks private. The `custom_provider` /
`custom_tooling` library examples (scripted `responses: Vec<String>`
streaming `ChatDelta`; in-memory tool returning a canned `value` while
recording each `ToolCall`) double as the offline back ends the brain
TOMLs reference (for example `[providers.echo] kind = "echo"`).
Brain TOMLs stay offline and `dev test fast`-safe: no `sk-...`,
no `npx`, no `docker`.

## Brain examples (identical x3: rust / js / rhai)

API map to preserve per language:

| op       | rust (`omw-wasm-rust`)                | rhai (snake_case maps)              | js (camelCase global)           |
| -------- | ------------------------------------- | ----------------------------------- | ------------------------------- |
| chat     | `Provider::get("echo")?.chat(...)`    | `omw::provider::get("echo").chat`   | `omw.provider.get("echo").chat` |
| stream   | `chat_stream` + `StreamGuard` + `recv` | `chat_stream` + uuid + `recv`      | `chatStream` + uuid + `recv`    |
| tool     | `call_tool_blocking(name, json)`      | `call_tool_blocking(name, #{...})`  | `callToolBlocking(name, {...})` |
| envelope | `EventEnvelope{id, event}`            | `#{id, kind, payload}`              | `{id, kind, payload}`           |

1. **01-hello** — blocking `chat("echo-model", [{ role: user,
   content: "say hi" }], [])`; terminal message is the reply text.
   Proves provider wiring in ~10 lines.

2. **02-tool-agent** — ReAct loop: `chat` returns
   `tool_calls[0] { echo, input: "hi" }` →
   `call_tool_blocking("echo", ...)` → final `chat` with the tool
   result → terminal `"<chat>|<tool>"`. Mirrors
   `tests/agents.rs:142-160` and `runtime/rhai.rs:306-319` /
   `js.rs:306-319`. Proves the tool round-trip offline.

3. **03-endpoint-server** — `subscribe_endpoint("gpt-4o")` + `recv()`
   loop on `endpoint-message` → `chat` the inbound messages →
   `stream_endpoint(session, { content })` +
   `{ finish_reason: "stop" }`. Mirrors `rhai.rs:662-670,752-756`.
   TOML adds `[endpoint] kind = "openai" listen = "127.0.0.1:8080"`;
   test with `curl /v1/chat/completions`.

4. **04-ping-pong** — no provider / tooling: alice
   `subscribe_agent("bob")` + `send_agent("bob", "ping")`, bob replies
   `"pong"`; both `memory_set` handles for reload survival. Mirrors
   `rhai.rs:454-505` and `docs/hot-reload.md:104-142`. Two
   `[[agents]]` in one TOML; proves the actor model plus `--watch`
   (`reload` / `shutdown` via `subscribe_lifecycle`).

Rust brains use `brain!(|| { ... Ok(()) })` (`#![no_main]`,
`omw-wasm-rust` dep, `cargo build --target wasm32-wasip2`),
`script = "brain.wasm"`, `[runtime.wasm] kind = "wasm"`.
Rhai / JS brains use `script = "brain.rhai"` / `"brain.js"`.

## Library examples (`src/lib/omw/examples/*.rs`)

- `embed_with_defaults.rs` — `toml::from_str::<Config>` (add `toml`
  to dev-deps) + the TLS `OnceLock` snippet + `Registries::default()`
  + `run_agents(cfg, false, ...)`. Runnable version of the
  `library.md:50-61` snippet.
- `custom_provider.rs` — `struct EchoProvider` + `impl Provider` +
  `impl provider::Factory` + `register_providers!`,
  TOML `[providers.echo] kind = "echo"`. Lifts `library.md:82-134`.
- `custom_tooling.rs` — in-memory `Tooling`
  (`Factory::build(name, params, tunables)`); lazy-connect notes.
- `custom_runtime.rs` — pure-Rust inline `Runtime`
  (`run(ctx) -> RunOutcome`, `validate(ctx)`) bypassing the WASM
  engine; the escape hatch.
- `custom_endpoint.rs` — minimal `Endpoint::serve(bus, registry,
  shutdown)` stub showing `shutdown.wait()` + the session registry.

Gate with `[[example]] required-features` so `--no-default-features`
customs build TLS-free while the defaults example requires
`provider-openai` etc. Keep repo style: 2-space, 80-col,
`anyhow::Result`, no `unwrap`.

## Docs and tests

- Update `docs/library.md` (snippets → `cargo run -p omw --example ...`),
  `docs/runtime/{rhai,js,wasm}.md` ("Example brain" →
  `examples/0X-*`), `SUMMARY.md`, `examples/README.md`.
- Verify: `dev test fast` for iteration (offline echo provider +
  in-memory tooling); final full `dev test` + `dev lint` before merge.
