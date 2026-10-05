# AGENTS.md

OMW = OpenAI + MCP + WASM. `omw` is an agent runtime: it loads an agent "brain"
(a WASM component, or a Rhai script evaluator, or a JS interpreter), drives it
through a host layer that bridges an OpenAI-family chat provider and MCP tool
servers, and runs it for one iteration or loops it.

## Layout

A Cargo workspace with the crates below plus a single WIT contract.

- `src/lib/omw` — the `omw` library crate: the agent runtime logic, a build
  script (`build.rs`) plus the vendored WIT contract under `wit/`.
  - `agent.rs` — bootstrap: turns a parsed config into provider + tooling +
    bus + `AgentContext`, then runs the agent's runtime for one iteration or
    loops it. `run_agents`/`loop_agents` also have `_traced` twins that attach
    an optional `host/trace.rs` channel and, for `run`, return the collected
    trace stream. Every public entry takes a caller-owned `shutdown::Shutdown`
    latch: the library installs no OS signal subscription (the binary owns
    that). A `pub(crate)` controlled path (`run_agents_controlled` +
    `StopRegistry`) lets `omw::testing` seed memory and stop an
    `outcome = "asserted"` agent on settle via a per-agent stop flag on the
    context.

  - `config.rs` — the config model plus the `Format` enum (TOML / YAML / JSON,
    inferred from a path's extension or chosen explicitly with `--format`, with
    `Format::parse` as the one format-aware deserializer the library uses for
    both `Config` and `[assertions]`). The default is the first of `omw.toml`,
    `omw.yaml`, `omw.yml`, `omw.json` in the current directory (erroring when
    none exists), overridable with the `--config` flag or layered from
    `OMW__`-prefixed environment variables: global provider/tooling/runtime maps
    (each an impl-agnostic `kind` + opaque params), an optional singular
    `[endpoint]` entry of the same shape, plus per-agent wiring and per-agent
    seeded `[memory.<agent>]` values (a string seed is stored verbatim and
    anything else is JSON-stringified, then inserted into the agent's memory
    before its brain runs, so they persist across hot reloads like any other
    memory). `Config::schema_json` emits the machine-readable schema; each impl
    category contributes a `pub(crate)` implementation `JsonSchema` that unions
    the built-in kinds enabled in the build (openai/mcp/openai-endpoint/wasm for
    `omw-cli`, plus the `mock` doubles and any script runtimes for `omw-test`)
    with the generic `ImplConfig` escape hatch, so the schema follows the
    crate's features (`schema.rs` holds the shared `kind_variant` helper). The
    global `[tunables]` (`Tunables`) carries the size/timeout/backoff knobs
    (plus the live-view `tui_tick_ms` / `tui_tab_capacity`), each with a default
    fn and accessor.

  - `log.rs` — initializes the structured, leveled JSON tracing subscriber
    (`RUST_LOG`-driven via `EnvFilter`, default `info`).

  - `watch.rs` — the public file-watching module: the generalized `Watcher`
    (`watch` / `add` / `next_change`, `RecursiveMode`) plus `Scripts`, which
    maps each agent's brain script to the agents using it and reports agent
    names to restart on change. Built on `notify-debouncer-full`; access/read
    events are ignored and only create/modify/remove (and rescan) events count,
    so a watcher never reacts to its own directory reads. Enabled per invocation
    with `--watch` on `run` / `loop`; the supervisor in `agent.rs` keeps the
    shared bus alive across reloads. (Path-to-directory inference is a CLI
    concern and lives in the binaries.)

  - `provider/` — the `Provider` abstraction over an OpenAI-family chat stream,
    implemented for OpenAI in `openai.rs` (behind the `provider-openai` feature)
    and as a scripted in-memory double in `mock.rs` (behind
    `any(test, feature = "mock")`; sequenced `turns` — each optionally ending
    with an `error` after its deltas or `pending` forever — plus `models`). The
    `build` factory dispatches on the configured `kind`.

  - `tooling/` — the `Tooling` abstraction over MCP-style tool servers,
    implemented as an MCP client in `mcp.rs` (behind the `tooling-mcp` feature)
    with a `transport`-tagged config enum (`stdio` / `http`), and as a scripted
    in-memory double in `mock.rs` (behind `any(test, feature = "mock")`: static
    `tools`, an ordered name-verified `tool_calls` list (each a text `result`
    convenience plus optional `structured_content`, `error` or `pending`),
    `initial_resource_*` plus ordered `resource_*_updates` (each optionally
    `error` or `pending`), each step gated by a shared `after` and paced by
    `delay_ms`; it learns the trace through `Tooling::attach_trace`, a default
    no-op the supervisor calls after building entries). `call_tool` returns a
    `ToolCallResult` (`content` + optional `structured_content`, both JSON). The
    `build` factory dispatches on the configured `kind`. For deterministic runs,
    the host takes a mock-only `pub(crate) ResourceInjections` hook carried on
    the `ToolingEntry` (set by `Registry::register_mock`, off the public
    `Tooling` trait) and registers the mock's gated steps as call-boundary
    injections instead of pumping them; the host's `call_tool` /
    `subscribe_resource*` use it when present, otherwise the pump.

  - `runtime/` — the `Runtime` abstraction (`Runtime::run(&AgentContext)`), with
    `bindings.rs` (the single `bindgen!` for the `omw` world, mapped onto host
    types), `host.rs` (the generated WIT `Host` trait implementations, bridging
    the synchronous engine to the async provider/tooling/bus), `engine.rs` (the
    generic WASM component loader + a generic `run` that calls the exported
    `runtime.run`, plus the per-runtime `WasiConfig`/`Preopen` sandbox flattened
    into each wasm-based runtime's config), `wasm.rs` (loads the agent's `.wasm`
    or `.wat` brain), `rhai.rs` (the bundled Rhai evaluator that loads `.rhai`
    brains enabled by the `runtime-rhai` feature), `js.rs` (the bundled JS
    evaluator that loads `.js` brains enabled by the `runtime-js` feature) and
    `python.rs` (the bundled Python evaluator that loads `.py` brains enabled by
    the `runtime-python` feature). The `runtime-wasm` feature gates
    `bindings`/`engine`/`host`/`wasm`; `runtime-rhai`, `runtime-js`,
    `runtime-python` and `mock` each imply it. The default features are
    `runtime-wasm`, `provider-openai`, `tooling-mcp` and `endpoint-openai`.

  - `endpoint/` — the `Endpoint` abstraction (`serve(bus, registry)`), with
    `openai.rs` (the optional OpenAI-compatible HTTP server behind the
    `endpoint-openai` feature: `[endpoint]` config with `kind` + `listen`,
    started only when configured: exposing `GET /v1/models` and
    `POST /v1/chat/completions`, routing each request as an `endpoint-message`
    inbox event under the model name the calling agent subscribed to, and
    streaming the agent's `stream-endpoint` deltas back as SSE or a buffered
    JSON completion. The `build` factory dispatches on the configured `kind`;
    transport-agnostic state lives in `host/bus.rs` (`endpoint_subscribe` /
    `endpoint_route`) and `host/endpoint.rs` (`EndpointRegistry`), and a
    scripted `mock.rs` client double (behind `any(test, feature = "mock")`)
    drives sessions for `omw-test`: each of its `requests` (carrying a required
    `session_id` label) is registered synchronously — before any agent runs — as
    a call-boundary injection that fires once the model is subscribed and its
    `after` pattern (if any) has been observed, so concurrent requests are
    routed deterministically; an optional `session_end = { close = N }` /
    `{ abort = N }` ends the session after `N` deltas (normal `close`, since
    `EndpointRegistry::close` delivers the normal end, or erroring `abort`)
    instead of the default silent drain. A request fires once the model is
    subscribed, or — when gated by an `after` pattern — once a matching trace
    event has been observed (order-only, via the bus's append-only `observed`
    log). `serve` only keeps the endpoint task alive. The agent's
    `stream-endpoint` call detail carries the delta (`session`, `content`,
    `tool_call`, `finish_reason`), so the reply is observable as the brain's own
    calls and no separate endpoint trace event exists; each reply records both
    the `session_id` and the opaque session UUID plus its ending (`closed` /
    `aborted` / `error`) in the snapshot.

  - `bindings.rs` — the single `bindgen!` for the `omw` world, mapped onto host
    types.

  - `build.rs` — for the `runtime-rhai`/`runtime-js`/`runtime-python` and `mock`
    features, cross-compiles the bundled guests for `wasm32-wasip2`, wraps a
    core module into a component with `wasm-tools component new` when needed,
    prints it to WAT with the `wasmprinter` build-dep, AOT-serializes it with
    `wasmtime`, and embeds all four via `include_bytes!`. The guest sources live
    outside the `omw` package (`src/wasm/*`), so setting
    `OMW_WASM_BUILD_VENDORED` (only the release prebuild) also copies each
    component into the package's `wasm/`. When the sources are absent (a
    registry checkout), `build.rs` embeds the vendored components instead of
    cross-building, so the published crate is self-contained. A
    `runtime-wasm`-less build runs no wasm tooling. `.release-plz.toml` sets
    `publish_allow_dirty = true` for `omw` because the vendored components are
    untracked (but un-ignored) at release time.

  - `testing/` — the deterministic brain-testing substrate. `assert.rs` holds
    the `[assertions]` model, parser (`parse(source, Format)`, format-aware),
    and ordered-subsequence pattern matcher (`Matcher`) over `call` / `inbound`
    plus the `opened` / `closed` lifecycle kinds (matched by source kind and
    close reason, never by UUID); `harness.rs` drives a run through the
    controlled path, consumes the trace live, stops `outcome = "asserted"`
    agents as their assertions settle (logging a settle line), bounds the run by
    `tunables.test_timeout_secs` (marking unsettled agents timed out), snapshots
    the mock back ends' scripted queues and per-call returns (provider deltas,
    tooling results, endpoint replies), and returns a `Report` whose
    `AgentReport`s carry the observed events, cursor and diff. `assert.rs` also
    holds the `pub(crate)` `TraceLog` (an append-only trace log with independent
    per-gate scanning) that the tooling mock still uses for its legacy
    `call_tool` `after` gate; resource and endpoint `after` gates use
    call-boundary injections instead. `scaffold.rs` holds the best-effort
    `scaffold` function that converts a deployment `Config` into a structured
    `toml::Table` test config whose provider/tooling/endpoint are the in-config
    mocks, introspecting the real back ends through the registries to
    pre-populate models/tools/resources; rendering it (pretty-printing, the
    provenance banner, the output path) is the `omw` binary's job. Exposed as
    `omw::testing` and re-exported from `prelude`; the `omw-test` binary is a
    thin CLI over it.

  - `host/` — the host side of the actor model.
    - `bus.rs` is the per-agent inbox + subscription registry that fans messages
      out tagged with a subscription UUID (and unsubscribes by handle). It also
      owns the deterministic-testing plumbing: an append-only `observed` trace
      log (appended by `trace_event`) and an ordered `Vec` of `Injection`s
      (mock-registered call-boundary deliveries). `flush_injections` runs each
      triggered injection synchronously at a guest call boundary and drops the
      consumed ones; registration order is preserved (never a `HashMap`).

    - `events.rs` is the `Event`/`EventEnvelope` type every I/O source pushes.

    - `time.rs` is the tick/timer helpers + cancellable timer pump.

    - `streams.rs` is the chat-stream pump registry keyed by UUID that delivers
      `chat-delta`/`chat-end` events into inboxes (its `CancelRegistry` alias
      also backs timer/resource pumps). Each entry carries the agent, its
      `SourceKind` and the trace tap, so `cancel` / `cancel_all` emit
      `closed{cancelled}` synchronously at the cancellation; pumps emit `closed`
      only for a natural `ended` / `failed` end.

    - `resources.rs` is the cancellable resource-subscription pump that delivers
      `resource-list-updated`/`resource-updated` events into inboxes. Real
      tooling uses the pump; the mock registers call-boundary injections instead
      (see `tooling/mock.rs`).

    - `tool_calls.rs` is the cancellable tool-call pump that delivers
      `tool-result` events into inboxes. Real tooling uses the pump; the mock
      registers an injection instead.

    - `memory.rs` is the per-agent string store (`DashMap`) that survives hot
      reloads via the reused `AgentContext`; guests expose a raw `memory_get` /
      `memory_set` pair plus a JSON-parsing `memory_get_as` / `memory_set_as`
      pair over it.

    - `endpoint.rs` is the per-process endpoint session registry (`open` /
      `push` / `abort` / the normal `close`) that buffers an agent's streamed
      deltas non-blocking, and fires `endpoint-session-end` events on
      normal/abrupt termination.

    - `ctx.rs` is `AgentContext`; `flush_injections` (called from `trace_call` /
      `trace_call_uuid` and the host's `recv` / `try_recv`) is the single flush
      choke point.

    - `trace.rs` is the optional broadcast trace channel (`TraceEvent` = inbound
      / call / opened / closed / outcome; a `call` carries the uuid of the
      handle it returned, and lifecycle events carry `SourceKind` / a
      `CloseReason`; `TraceSender`, `AgentTrace`, `group`) that `omw-test` and
      embedders use to observe what agents saw and did. Both `MessageBus` and
      `AgentContext` hold an `Option<TraceSender>`, so it is zero-overhead when
      unset. `MessageBus::trace_sender` lets the endpoint and tooling mocks
      build a shared append-only `TraceLog` for their legacy `after` gating;
      deterministic mock deliveries use call-boundary injections instead (see
      `host/bus.rs`).

- `src/lib/omw-output` — the binary-only shared output-policy crate (published
  only so the `omw-cli`/`omw-test` binaries can depend on it from the registry;
  not a supported library, picked up by the `src/lib/*` member glob): the
  `LogFormat` model plus `auto` detection (both stdio terminals → tty, then
  `JOURNAL_STREAM` → journald, else pipe), `command_line` (the launched `argv`
  as one string for the info panel), `producer_format` (a data producer never
  shows the live view), `init_logging` (the journald layer, the JSONL `pipe`
  layer, or the ratatui/crossterm live view), and the once-only `report_error`.
  The live view (`live.rs` state + `Live` handle, `layer.rs` tracing layer,
  `tui.rs` render thread) is driven on stderr so a redirected stdout keeps
  carrying data; it routes MCP child stderr to `mcp:<name>` sources by the
  `source`/`tooling` fields, and both views share a rounded-bordered info panel
  (command + `set_details` facts): the `Agents` view pairs a "Sources" list with
  a log pane, the `Tests` view pairs a "Tests" list (`✅`/`❌` marks + a
  spinner) with a "Logs" pane and a bordered gauge. It honours `NO_COLOR`,
  restores the terminal on drop/panic, and falls back to `pipe` when raw mode
  cannot be entered. `init_logging` returns `Option<Live>`; the binaries own the
  handle and select on its quit receiver so Ctrl-C in raw mode requests the
  library's `Shutdown`. Because the view starts before config load, the binaries
  push the `tunables.tui_tick_ms` / `tui_tab_capacity` knobs into it with
  `Live::configure` (crate-local defaults hold until then; `0` capacity is
  unlimited). This crate depends on `ratatui` + `crossterm`; the `omw` library
  does not depend on it.

- `src/bin/omw-cli` — the OMW CLI binary crate. It contains a basic run
  function, argument/config parsing and initialization for `tracing` and
  `rustls`, plus `shutdown.rs` (the SIGTERM/SIGINT subscription that requests
  the library's `Shutdown` latch — the binary owns the process signal, not the
  library) and `stdio.rs` (the `-` / `/dev/stdin` / `/dev/stdout` convention for
  config inputs and command outputs). It also has the `scaffold` subcommand: a
  thin wrapper over `omw::testing::scaffold` that loads a deployment config,
  renders the returned `toml::Table` (provenance banner + pretty-print) and
  writes an `omw.test.toml` with mock back ends pre-populated from the real
  ones. Config paths default to the first of `omw.{toml,yaml,yml,json}` in the
  current directory and are parsed by extension or a `--format` override (a
  standard stream needs `--format`). `run`/`loop` install the live view when the
  resolved format is `tty`, announce a status line, and select on the live
  view's quit receiver (Ctrl-C in raw mode) to request the library's `Shutdown`;
  `schema`/`scaffold` are data producers and always log on `pipe`. `tests/`
  holds process-level integration tests (spawned with `assert_cmd`): argv/exit
  codes, config paths and format inference, the stdio convention, the `OMW__`
  env overlay, and graceful SIGTERM/SIGINT shutdown (a raw process + `nix`,
  watching stderr for a ready marker).

- `src/bin/omw-test` — the deterministic brain-testing binary crate. Mirrors
  `omw-cli` (`cli`, `log`, `tls`, `stdio`) plus `collect.rs` (recursive
  test-config discovery — a file whose stem is `omw.test` or ends with
  `.omw.test` and whose extension is `toml`/`yaml`/`yml`/`json` — plus
  root-relative-path include/exclude filtering and format-aware config/assertion
  loading, attaching each test's inherited `omw.test.base.<ext>` ancestors and
  layering them under it by deep table merge), `run.rs` (traced run + assertion
  check via `omw::testing`) and `wasm.rs` (the hidden `compile-wasm` subcommand
  that cross-builds a caller-supplied rust brain file or tree for
  `wasm32-wasip2` into components, gated behind the non-default `compile-wasm`
  feature that only the dev shell enables). The `[assertions]`
  model/parser/matcher and the `--watch` debounced watcher live in the `omw`
  library (`omw::testing`); the binary only discovers, filters, prints and sets
  the exit code. `omw-test run [path]` (default `.`) recursively runs every
  discovered config through the `omw::testing` harness against the in-config
  `kind = "mock"` doubles, printing `PASS`/`FAIL` per test and exiting non-zero
  with a diff when an assertion mismatches; it is fail-fast by default (`--all`
  keeps going), each test inherits the `omw.test.base.<ext>` configs in its
  directory and ancestors up to the discovery root, `--dump <path>` writes
  per-test traces and mock snapshots as JSON, YAML or TOML (`--dump-format`,
  default JSON), and a standard-stream path (`-`, `/dev/stdin`) is a single
  config read from stdin (needs `--format`); `--include`/`--exclude` filter the
  root-relative config paths and `--watch` re-runs on change. On `tty` it drives
  the live view's `Tests` view (a tests list marked `✅`/`❌`, a logs/failure
  pane, a bordered gauge) and resets it each `--watch` pass; verdicts go to
  stdout only when stdout is not the terminal the live view owns, and after the
  live view tears down the per-test lines and tally are replayed to stdout.
  Depends on `omw` with `default-features = false, features = ["mock"]` (plus
  the script runtimes). `tests/` holds process-level integration tests (spawned
  with `assert_cmd`): argv/exit codes, the stdio convention, the `OMW_TEST__`
  env overlay, and `run` PASS/FAIL/discovery/filter semantics.
- `src/wasm/omw-wasm-rhai-interpreter` — the Rhai guest component
  (`#![no_main]`), compiled to `wasm32-wasip2`. Exports the `runtime` interface
  (`kind` + `run(script)`) and registers the `omw` static module whose
  `provider`/`tooling`/`host` functions route to the host.

- `src/wasm/omw-wasm-js-interpreter` — the JS guest component (`#![no_main]`),
  compiled to `wasm32-wasip2`. Exports the `runtime` interface (`kind` +
  `run(script)`) and registers the `omw` global (Boa) whose
  `provider`/`tooling`/`host` namespaces route to the host (camelCase). Ships
  the JS-side declarations: the hand-written `omw.d.ts` for the `omw` global
  (including the generic `Omw<Config>` type), and the generated `omw.all.d.ts` —
  the concatenation of the deployment/testing config types
  (`assets/schema.d.ts`, `assets/schema.test.d.ts`) and `omw.d.ts` — which is
  the file to vendor next to a `brain.js` (see `docs/runtime/js.md`).

- `src/wasm/omw-wasm-python-interpreter` — the Python guest component
  (`#![no_main]`), compiled to `wasm32-wasip2` on the bundled RustPython
  interpreter. Exports the `runtime` interface (`kind` + `run(script)`) and
  installs the `omw` global whose `provider`/`tooling`/`host` namespaces route
  to the host (snake_case; records surface as attribute objects). A script that
  parses as a single expression returns its value as the terminal message;
  anything else runs as statements. Ships the Python-side declarations: the
  hand-written `omw.pyi` stubs for the `omw` global, the generated
  `assets/schema.py` / `assets/schema.test.py` pydantic models (from
  `datamodel-code-generator`), and the combined `omw.all.py` (both schemas
  namespaced under `OmwConfig` / `OmwTestConfig`, plus the stubs) — the file to
  vendor next to a `brain.py` (see `docs/runtime/python.md`).

- `src/lib/omw-wasm-rust` — the `omw-wasm-rust` guest SDK for Rust brains
  (published to crates.io): re-exports the generated `omw` world bindings plus
  small builders, typed `Provider`/`Tooling` handles, `host` helpers and
  lifetime guards, and re-exports `serde`/`serde_json`. Provides `_as` helpers
  for opaque JSON: `host::memory_get_as` / `memory_set_as`, and record accessors
  (`ToolCall::arguments_as`, `ToolResult::content_as`,
  `EndpointMessage::params_as`, …). JSON inputs to the typed handles take a
  `serde_json::Value` directly (`Provider::chat`/`chat_stream` `params`,
  `Tooling::call_tool`/`call_tool_blocking` `arguments`). Vendors the WIT
  contract under `wit/` (kept in sync with `src/lib/omw/wit/`).

- `src/wasm/omw-wasm-mock` — the test-only wasm mock brain, cross-compiled by
  the `mock` feature for the engine/wasm runtime tests (not shipped to
  crates.io; its manifest carries `publish = false`).

- `src/lib/omw/wit/omw.wit` — the single WIT contract, used by host (`bindgen!`)
  and guests (`wit-bindgen::generate!`). Changes here ripple into both crates.

- `src/lib/omw/examples/` — runnable _library_ examples (the `omw` package's
  `examples/`, built with `--all-features`), each a self-contained `main` that
  embeds the library: the custom back ends and `embed_with_defaults`, plus
  `testing_harness` (the testing harness), `watching_scripts` (the watcher), and
  `observability` (streaming the trace channel live). `dev test` runs them all.

- `examples/` — runnable brain examples (not a workspace member): `01-hello`,
  `02-tool-agent`, `03-endpoint`, `04-ping-pong`, `05-patterns`, `06-memory`,
  `07-asserted`, `08-endpoint-order`, `09-resources`, `10-endpoint-stop`,
  `11-tool-call-order` and `12-race-cancels`, each one shared
  `omw.test.base.toml` (the provider/tooling/endpoint wiring, each agent's
  `runtime` and the `[assertions]`), per-variant `rhai/`, `js/`, `python/` and
  `wasm/` dirs (`brain.rhai` / `brain.js` / `brain.py` / `brain.rs`), a
  committed generated `<variant>/omw.test.toml` (only the runtime `kind` and
  each agent's `script`), and a `README.md`. `dev format` regenerates the
  per-variant configs from the base config and `dev lint`
  regenerates-and-compares them. `dev test example brain <case> <variant>`
  builds that cell's `brain.rs` to `brain.wasm` (scaffolding a throwaway crate
  in the system temp dir), then runs the case with only that variant included;
  `dev test brain examples` runs every cell, gating the `wasm` cells on
  `OMW_TEST_WASM_RUNTIME_NON_NATIVE`, and `dev test` runs it alongside the
  library examples and unit tests. The base configs drive the in-config
  `kind = "mock"` doubles and carry the `[assertions.<agent>]` sections.

- `docs/` — mdbook documentation, published to GitHub Pages. `docs/testing/`
  (the binary plus per-mock pages) and `docs/examples.md` cover the test and
  example features.

- `src/nix/dev.nix` / `src/nix/dev.nu` — the flake's `dev` wrapper: `dev.nix`
  builds the dev shells, owns the `binaries`/`variants`/`formats` build-matrix
  axes (also exposed as `flake.lib`, each variant carrying its runtime kind and
  example `source`/`script`), and points the wrapper at `dev.nu`, which defines
  the `dev` subcommands (`format` / `lint` / `test` / `build` / `release*` /
  `update`) and the `omw` helpers they share. `dev.nu` evaluates
  `flake.lib.{binaries,variants,formats}` (via `omw matrix`) as the single
  source of truth for builds, tarballs and examples, so adding a runtime is a
  new entry in `variants`.
- `src/nix/json-schema-to-typescript.nix` — a flake-parts module that packages
  the `json-schema-to-typescript` CLI (`json2ts`, which is not in nixpkgs) as a
  `buildNpmPackage` from the pinned upstream source
  (`npmBuildScript = "build:server"`). The `json2ts` bin comes from the
  package's own `bin` field and is put on the dev/ci shell `PATH`.

- `src/nix/nixos.nix` — the NixOS module exposing `services.omw` — a systemd
  unit that runs `omw <mode> --config <file>` directly (secrets layer over the
  file from `OMW__`-prefixed `environment`/`environmentFile`), with `mode`,
  `extraArgs`, `user`/`group` (or dynamic user), `stateDir`, default-on
  `hardening` (+ `readOnlyPaths`/`readWritePaths` path allow-lists and a
  `serviceConfig` escape hatch) and a `variant` option selecting the `default`,
  `rhai`, `js` or `python` package variant. Its option reference is generated by
  the `omw-options` flake package.

- `assets/` — deployment examples included verbatim in the docs (`omw.service`,
  `Dockerfile`, `compose.yaml`, `omw.example.toml`, `omw.example.env`,
  `omw.nix`) plus the committed machine-readable schemas (`schema.json`,
  `schema.test.json`), the config TypeScript declarations generated from them
  (`schema.d.ts`, `schema.test.d.ts`), each wrapped in a namespace, and the
  pydantic models generated from them (`schema.py`, `schema.test.py`).

- `docs/deployment/` — deployment pages (`deployment.md` overview plus
  `systemd.md`, `docker.md`, `nixos/`); keep the shared config/workspace
  conventions in sync with the assets above.

## Conventions

- Rust edition 2024.

- 2-space indentation enforced by rustfmt and prettier with maximum line length
  of 80. Tabs are never used and line endings are `lf`.

- Host crates deny `unsafe_code`, `unwrap`/`expect`/`panic`/`unreachable`,
  `arithmetic_side_effects`, `todo` and un-reasoned `#[allow]`. The guest crates
  cannot deny `unsafe_code` (the exported component ABI requires it).

- Results are `anyhow::Result`.

- Async abstractions use `async_trait`.

- Structured JSON logging via `tracing` (see `src/bin/omw-cli/src/log.rs`).
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
  `install_omw` (Rhai guest), the `omw` global (JS guest), the `omw` global
  (Python guest), and the vendored copies under `src/lib/omw-wasm-rust/wit/`,
  `src/wasm/omw-wasm-rhai-interpreter/wit/`,
  `src/wasm/omw-wasm-js-interpreter/wit/`,
  `src/wasm/omw-wasm-python-interpreter/wit/` and `src/wasm/omw-wasm-mock/wit/`.

## Library surface

The `omw` library exposes a small embedding contract; everything else is host
plumbing (`pub(crate)`) or per-module private. The `omw-cli` binary crate
(`cli`, `log`, `tls`, `shutdown`, `stdio`) is a separate crate and is not part
of the library at all.

| Module               | `pub` (embedding contract)                                                                                                                                                                                           | `pub(crate)` / private                                                                                                                                                                                                                              |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `agent`              | `Registries`, `run_agents`, `loop_agents`, `run_agents_traced`, `loop_agents_traced` (each takes a caller-owned `Shutdown`)                                                                                          | supervisor internals (`Shared`, `run_agent`) private                                                                                                                                                                                                |
| `config`             | `Config`, `AgentConfig`, `ImplConfig`, `Tunables`, `Format`                                                                                                                                                          | default fns private                                                                                                                                                                                                                                 |
| `provider`           | `Provider`, `Factory`, `Registry`, `ProviderEntry`, DTOs (`Role`, `ChatMessage`, `ChatDelta`, `ChatResult`, `ToolCall`, `Usage`), `register_providers!`                                                              | `openai` private mod, `mock` `pub(crate)` (test only)                                                                                                                                                                                               |
| `tooling`            | `Tooling`, `Factory`, `Registry`, `ToolingEntry`, DTOs (`Tool`, `ResourceInfo`, `ResourceContent`, `ResourceNotification`, `ToolCallResult`), `register_toolings!`                                                   | `mcp` still `pub mod` (impl detail), `mock` `pub(crate)`                                                                                                                                                                                            |
| `runtime`            | `Runtime`, `Factory`, `Registry`, `RuntimeEntry`, `RunOutcome`, `register_runtimes!`                                                                                                                                 | `wasm` / `rhai` / `js` / `python` plus `engine` / `bindings` / `host` private                                                                                                                                                                       |
| `endpoint`           | `Endpoint`, `Factory`, `Registry`, `EndpointEntry`, `register_endpoints!`                                                                                                                                            | `openai` still `pub mod` (impl detail)                                                                                                                                                                                                              |
| `host`               | `AgentContext` (`name()` only), `Event`, `EventEnvelope` (plus `ToolResult`, `EndpointMessage`, `EndpointSessionEnd`, `trace` types (`TraceEvent`, `TraceSender`, `AgentTrace`, `group`))                            | `bus` / `ctx` / `endpoint` / `events` are `pub` mods, `trace` is a `pub` mod (`memory` / `resources` / `streams` / `time` / `tool_calls` are `pub(crate)`)                                                                                          |
| `secret`, `shutdown` | `Secret` (`new`, `expose`), `Shutdown`                                                                                                                                                                               | `shutdown_signal` owned by `omw-cli` (process policy)                                                                                                                                                                                               |
| `testing`            | `Assertions` / `AgentAssertion` / `EventAssertion` / `OutcomeAssertion` / `Pattern` / `ArrayStep` / `After` / `Matcher`, `Harness` / `Report` / `AgentReport`, `parse`, `collect`, `check`, `event_kind`, `scaffold` | —                                                                                                                                                                                                                                                   |
| `watch`              | `Watcher` (`new` / `watch` / `add` / `next_change`), `Scripts` (`with_tunables` / `next_reload`), `RecursiveMode`                                                                                                    | —                                                                                                                                                                                                                                                   |
| `prelude`            | re-exports the embedding subset plus the `register_*` macros (also `#[macro_export]` at the crate root)                                                                                                              | —                                                                                                                                                                                                                                                   |
| binary-only          | —                                                                                                                                                                                                                    | `cli` (`Cli`, `Command`, `RunArgs`, `ScaffoldArgs`, `generate_schema`), `log::init`, `tls::init`, `shutdown::install`, `stdio` owned by the `omw-cli` crate; `omw-test` owns its own `cli`/`collect`/`run`/`wasm` plus mirrored `log`/`tls`/`stdio` |

## Development

Assume you are in the default development shell. Commands go through the `dev`
wrapper (`src/nix/dev.nu`, invoked by `dev.nix`):

- `dev format` — prettier, nixfmt, cargo fmt, then `cargo clippy --fix`; also
  regenerates the per-variant test configs, `options.md`, the two schemas
  (`schema.json` for `omw-cli`, `schema.test.json` for `omw-test`) and the
  declarations generated from them (`assets/schema.d.ts`,
  `assets/schema.test.d.ts` and the concatenated
  `src/wasm/omw-wasm-js-interpreter/omw.all.d.ts`), plus the pydantic models
  (`assets/schema.py`, `assets/schema.test.py` via `datamodel-codegen`) and the
  concatenated `src/wasm/omw-wasm-python-interpreter/omw.all.py`)
- `dev test` — `omw test lib examples` (each library example), `omw test units`
  (`cargo clippy --all-features -- -D warnings` plus
  `cargo test --all-features`) and `omw test brain examples` (cross-builds the
  example brains when the gate is on, then `omw-test run examples`)
- `dev test fast` — like `dev test` but with extra environment that tells tests
  to ignore heavier tests (tests that require `testcontainers`, WASM
  compilation, MCP servers or the OpenAI API)
- `dev test example lib <example>` — run one library example
- `dev test example brain <case> <variant>` — build that cell's `brain.wasm`
  when needed and run one example dir with its case's common config
- `dev update` — `nix flake update` plus `cargo update`
- `dev release-pr` — `release-plz release-pr` (opens the release PR)
- `dev release` — vendors the wasm guests first
  (`OMW_WASM_BUILD_VENDORED=1 cargo build --release -p omw --features runtime-rhai,runtime-js,runtime-python,mock`),
  then `release-plz release` (tags + publishes on release PR merge)
- `dev build` — builds the
  `omw-tarball`/`omw-rhai-tarball`/`omw-js-tarball`/`omw-python-tarball`/`omw-test-tarball`
  packages (per-arch `omw[-rhai,-js,-python,-test]-<arch>.tar.gz` via
  `runCommand`) and uploads them to the tag release (`GITHUB_REF_NAME`) with
  `gh`, one artifact at a time so a killed job still leaves the earlier tarballs
  uploaded; then builds the wrapped/unwrapped flake packages (already warm from
  the tarball build) and pushes their runtime closure to the public
  `haras-releases` Cachix cache, pinning each (`<attr>-<system>`,
  `--keep-days 365`)
- `dev lint` — the local full CI pass: `dev lint check`, `dev lint test`,
  `dev lint nix --all-systems`, then `dev lint build`
- `dev lint check` — the static checks:
  wit/options/schema/`.d.ts`/`.py`/per-variant config freshness, prettier,
  cspell, nixfmt, markdownlint, markdown-link-check, taplo and
  `cargo fmt --check`
- `dev lint test` — the same tasks as `dev test`
- `dev lint nix [--all-systems]` — `nix flake check --show-trace` (optionally
  across systems)
- `dev lint build` — `nix build .#omw-tarball`, building the release-profile
  (fat-LTO) null-variant tarball so a broken release build is caught before the
  tag `build` workflow

CI (`check.yaml`) runs `dev lint check`, `dev lint test`, `nix flake check` and
`nix build .#omw-tarball` in parallel jobs; the `nix` and `build` jobs are
runner matrices (`ubuntu-latest`, `ubuntu-24.04-arm`) so each arch checks
itself. The PR check builds the `ci` cargo profile (no LTO,
`codegen-units = 16`, `opt-level = 1`) via the `ci-unwrapped` / `ci-wrapped` Nix
formats, while shipped Nix packages, `apps` and tarballs keep the `release`
profile. `ci-wrapped` shares the `wrapped` wrapper function so both stay in
sync.

There are two Cachix caches: `haras` is the general CI cache (every workflow
pushes to it, `name: haras`) and `haras-releases` is the curated public cache
for users. CI jobs pull from both (`extraPullNames: haras-releases`); only
`dev build` writes to `haras-releases`, and `flake.nix` / `README.md` point
users there.

Do not use any shell commands other than the ones provided by `dev`. Please
prefer `dev test fast` over `dev test` if you don't need to test stuff that
touches WASM compilation, MCP servers or OpenAI API servers. Even in those cases
you should try to use `dev test fast` as much as possible for fast iteration
until you need to do a final pass on all tests. Do not use anything other than
`dev test fast` unless the user specifically demands for it.

Because `build.rs` cross-compiles the bundled guests (for the
`runtime-rhai`/`runtime-js`/`runtime-python`/`mock` features) for
`wasm32-wasip2`, building those features **from source** needs that target and
`wasm-tools` on PATH (both provided by the dev shell). The published crate
embeds the vendored components instead, so consumers need neither (the WAT comes
from the `wasmprinter` build-dep and the AOT from the `wasmtime` build-dep).
