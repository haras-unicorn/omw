# AGENTS.md

OMW = OpenAI + MCP + WASM. `omw` is an agent runtime: it loads an agent "brain"
(a WASM component, or a Rhai script evaluator, or a JS interpreter), drives it
through a host layer that bridges an OpenAI-family chat provider and MCP tool
servers, and runs it for one iteration or loops it.

## Layout

A Cargo workspace with four crates plus a single WIT contract.

- `src/native/omw` — the `omw` host binary. A thin `main.rs` entrypoint plus a
  library (`lib.rs`) that contains the runtime logic and a build script
  (`build.rs`) plus the vendored WIT contract under `wit/`.
  - `agent.rs` — bootstrap: turns a parsed config into provider + tooling +
    bus + `AgentContext`, then runs the agent's runtime for one iteration or
    loops it.

  - `config.rs` — the TOML config (default `omw.toml`, overridable with the
    `--config` flag or layered from `OMW__`-prefixed environment variables):
    global provider/tooling/runtime maps (each an impl-agnostic `kind` + opaque
    params) plus per-agent wiring.

  - `log.rs` — initializes the structured, leveled JSON tracing subscriber
    (`RUST_LOG`-driven via `EnvFilter`, default `info`).

  - `watch.rs` — hot-reload file watching (`ScriptWatcher`): maps each agent's
    brain script to the agents using it and reports agent names to restart on
    change. Enabled per invocation with `--watch` on `run` / `loop`; the
    supervisor in `agent.rs` keeps the shared bus alive across reloads.

  - `provider/` — the `Provider` abstraction over an OpenAI-family chat stream,
    implemented for OpenAI in `openai.rs` (behind the `openai` feature). The
    `build` factory dispatches on the configured `kind`.

  - `tooling/` — the `Tooling` abstraction over MCP-style tool servers,
    implemented as an MCP client in `mcp.rs` (behind the `mcp` feature) with a
    `transport`-tagged config enum (`stdio` / `http`). The `build` factory
    dispatches on the configured `kind`.

  - `runtime/` — the `Runtime` abstraction (`Runtime::run(&AgentContext)`), with
    `bindings.rs` (the single `bindgen!` for the `omw` world, mapped onto host
    types), `host.rs` (the generated WIT `Host` trait implementations, bridging
    the synchronous engine to the async provider/tooling/bus), `engine.rs` (the
    generic WASM component loader + a generic `run` that calls the exported
    `runtime.run`, plus the per-runtime `WasiConfig`/`Preopen` sandbox flattened
    into each wasm-based runtime's config), `wasm.rs` (loads the agent's `.wasm`
    or `.wat` brain), `rhai.rs` (the bundled Rhai evaluator that loads `.rhai`
    brains enabled by the `rhai` feature) and `js.rs` (the bundled JS evaluator
    that loads `.js` brains enabled by the `js` feature). The `wasm` feature
    gates `bindings`/`engine`/`host`/`wasm`; `rhai`, `js` and `mock` each imply
    it. The default features are `wasm`, `openai` and `mcp`.

  - `endpoint/` — the optional OpenAI-compatible HTTP server (`[endpoint]`
    config, axum; zero extra features, started only when configured: exposing
    `GET /v1/models` and `POST /v1/chat/completions`, routing each request as an
    `endpoint-message` inbox event under the model name the calling agent
    subscribed to, and streaming the agent's `stream-endpoint` deltas back as
    SSE or a buffered JSON completion.

  - `bindings.rs` — the single `bindgen!` for the `omw` world, mapped onto host
    types.

  - `build.rs` — for the `rhai`/`js` and `mock` features, cross-compiles the
    bundled guests for `wasm32-wasip2`, wraps them into components with
    `wasm-tools`, and embeds them via `include_bytes!`. A `wasm`-less build runs
    no wasm tooling (so crates.io `cargo publish` of `omw` with
    `--no-default-features` verifies standalone).

  - `host/` — the host side of the actor model.
    - `bus.rs` is the per-agent inbox + subscription registry that fans messages
      out tagged with a subscription UUID (and unsubscribes by handle).

    - `events.rs` is the `Event`/`EventEnvelope` type every I/O source pushes.

    - `time.rs` is the tick/timer helpers + cancellable timer pump.

    - `streams.rs` is the chat-stream pump registry keyed by UUID that delivers
      `chat-delta`/`chat-end` events into inboxes (its `CancelRegistry` alias
      also backs timer/resource pumps).

    - `resources.rs` is the cancellable resource-subscription pump that delivers
      `resource-list-updated`/`resource-updated` events into inboxes.

    - `tool_calls.rs` is the cancellable tool-call pump that delivers
      `tool-result` events into inboxes.

    - `memory.rs` is the per-agent string store (`DashMap`) that survives hot
      reloads via the reused `AgentContext`.

    - `endpoint.rs` is the per-process endpoint session registry (`open` /
      `push` / `abort`) that buffers an agent's streamed deltas non-blocking,
      and fires `endpoint-session-end` events on normal/abrupt termination.

    - `ctx.rs` is `AgentContext`.

- `src/wasm/omw-wasm-rhai-interpreter` — the Rhai guest component
  (`#![no_main]`), compiled to `wasm32-wasip2`. Exports the `runtime` interface
  (`kind` + `run(script)`) and registers the `omw` static module whose
  `provider`/`tooling`/`host` functions route to the host.

- `src/wasm/omw-wasm-js-interpreter` — the JS guest component (`#![no_main]`),
  compiled to `wasm32-wasip2`. Exports the `runtime` interface (`kind` +
  `run(script)`) and registers the `omw` global (Boa) whose
  `provider`/`tooling`/`host` namespaces route to the host (camelCase).

- `src/native/omw-wasm-rust` — the `omw-wasm-rust` guest SDK for Rust brains
  (published to crates.io): re-exports the generated `omw` world bindings plus
  small builders, typed `Provider`/`Tooling` handles, `host` helpers and
  lifetime guards. Vendors the WIT contract under `wit/` (kept in sync with
  `src/native/omw/wit/`).

- `src/wasm/omw-wasm-mock` — the test-only wasm mock brain, cross-compiled by
  the `mock` feature for the engine/wasm runtime tests (not shipped to
  crates.io; its manifest carries `publish = false`).

- `src/native/omw/wit/omw.wit` — the single WIT contract, used by host
  (`bindgen!`) and guests (`wit-bindgen::generate!`). Changes here ripple into
  both crates.

- `docs/` — mdbook documentation, published to GitHub Pages.

- `src/nix/nixos.nix` — the NixOS module exposing `services.omw` — a systemd
  unit that `envsubst`s a config (from `settings` or `settingsFile`) into
  `omw --config /dev/stdin`, with `environment`/`environmentFile`, `mode`,
  `extraArgs`, `user`/`group` (or dynamic user), `stateDir` options and a
  `variant` option selecting the `default`, `rhai` or `js` package flavor. Its
  option reference is generated by the `omw-options` flake package.

## Conventions

- Rust edition 2024.

- 2-space indentation enforced by rustfmt and prettier with maximum line length
  of 80. Tabs are never used and line endings are `lf`.

- Host crates deny `unsafe_code`, `unwrap`/`expect`/`panic`/`unreachable`,
  `arithmetic_side_effects`, `todo` and un-reasoned `#[allow]`. The guest crates
  cannot deny `unsafe_code` (the exported component ABI requires it).

- Results are `anyhow::Result`.

- Async abstractions use `async_trait`.

- Structured JSON logging via `tracing` (see `src/native/omw/src/log.rs`).
  - Level discipline: `trace` = wire/delta level, `debug` = flow/transitions,
    `info` = lifecycle milestones, `warn` = recoverable anomalies, `error` =
    failures.

  - Every guest-invoked operation carries the agent name as a structured field.
    The per-agent `run_agent`/`engine.run` spans cover the synchronous run and
    spawned pump tasks (chat streams, timers, resource subscriptions) pass
    `agent = %name` explicitly.

  - Secrets are never logged: `Secret` (`secret.rs`) wraps secret strings in a
    locked (`mlock`), zeroized-on-drop `Box<[u8]>` and redacts on
    `Debug`/`Serialize`, so `?`-logging configs is safe. `mlock` failure fails
    config deserialization (hard fail at startup). `ImplConfig` debug output
    only lists param keys, never values.

- The wasm engine is synchronous and runs on a `spawn_blocking` thread, not a
  tokio worker. Async is bridged through a dedicated tokio runtime held in
  `AgentContext` (`ctx.rt`). `provider.chat-stream` spawns a chat-stream pump on
  `rt` that delivers `chat-delta`/`chat-end` events into the inbox via
  `bus.deliver` (using `futures_util` + `tokio::select!`), and the
  `tooling.*`/`host.try-recv` use `ctx.block_on_reload`. `kanal` is used only
  for the per-agent `MessageBus` inboxes.

- Keep the `omw` WIT world(s) in sync with `runtime/bindings.rs` (host),
  `install_omw` (Rhai guest), the `omw` global (JS guest), and
  `src/native/omw-wasm-rust/wit/` (Rust SDK).

## Development

Assume you are in the default development shell. Commands go through the `dev`
wrapper (written in `flake.nix`):

- `dev run` — `cargo run --bin omw`
- `dev format` — prettier, nixfmt, cargo fmt, then `cargo clippy --fix`
- `dev test` — `cargo clippy --all-features -- -D warnings` and
  `cargo test --all-features`
- `dev test fast` — like `dev test` but with extra environment that tells tests
  to ignore heavier tests (tests that require `testcontainers` or WASM
  compilation)
- `dev lint` — prettier/cspell/nixfmt/markdownlint/taplo checks, then `dev test`
  — CI (`check.yaml`) runs `dev lint` and `nix flake check`

Do not use any shell commands other than the ones provided by `dev`. Please
prefer `dev test fast` over `dev test` if you don't need to test stuff that
touches WASM compilation, MCP servers or OpenAI API servers. Even in those cases
you should try to use `dev test fast` as much as possible for fast iteration
until you need to do a final pass on all tests.

Because `build.rs` cross-compiles the bundled guests (for the `rhai`/`mock`/`js`
features) for `wasm32-wasip2`, building with those features needs that target
and `wasm-tools` on PATH (both provided by the dev shell).
