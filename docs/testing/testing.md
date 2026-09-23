# Testing

`omw-test` is the deterministic brain-testing binary. It runs each discovered
`omw.test.toml` through the same traced path the `omw` binary uses
(`run_agents_traced`), against in-process scripted doubles (`kind = "mock"`),
records what every agent saw and did, and checks that recording against an
`[assertions]` section. No keys, no network, no external services.

A test config is a normal `omw.toml`-shaped file named `omw.test.toml`: the same
config `omw` consumes, plus an `[assertions]` table that stock `omw` ignores the
same way it ignores unknown keys. Values can be layered from the environment
with the `OMW_TEST__` prefix, exactly like `OMW__` for `omw` itself.

## Running

```sh
omw-test run examples/01-hello   # one config or directory
omw-test run examples            # every discovered config
```

- `<path>` is a **file** → run that config once.
- `<path>` is a **directory** → recursively find every `omw.test.toml` and run
  each, printing `PASS` / `FAIL <root-relative dir>` and a tally. It exits
  non-zero if any failed.
- `--include <glob>` / `--exclude <glob>` (repeatable, OR within each) match the
  test's root-relative directory path (`*` does not cross `/`, `**` does);
  include is applied first, then exclude. A missing `script` is always a
  failure, never a skip; narrow the set with the globs instead.
- `--watch` re-runs on change instead of exiting: after each pass it waits for a
  debounced filesystem event and runs again (file mode watches the config's
  parent directory; directory mode watches the root recursively). The library
  hot-reload watch is always off.

Discovery skips hidden directories and never collects `omw.test.template.toml`
(the shared templates the examples generate their configs from).

## Assertions

`[assertions.<agent>]` is compared against the agent's recorded trace: an
optional terminal `outcome` plus an ordered `events` list.

```toml
[assertions.alice]
outcome = "completed"
events = [
  { kind = "call", op = "chat", detail = { model = "^gpt-" } },
  { kind = "inbound", event = "chat-delta" },
]
```

- `outcome` is `"completed"`, `{ exited = "<msg>" }`, or `"asserted"` (below).
- each `events` entry is one of:
  - `{ kind = "call", op = "...", detail = { ... } }` — an outbound host call.
    `op` is exact; `detail` is a partial pattern over the call's JSON detail.
  - `{ kind = "inbound", event = "...", payload = { ... } }` — an inbox event.
    `event` is the kebab-case kind (`chat-delta`, `chat-end`, `tool-result`,
    `endpoint-message`, `message`, `timer`, `reload`, `shutdown`, `error`, …);
    `payload` is a partial pattern over the serialized event.
  - `{ "$any" = true }` — consume exactly one trace event, whatever it is.
  - `{ "$skip" = N }` — consume `N` trace events.

The list is matched as an **ordered subsequence** over **partial patterns**:

- Only the events you write are checked, in order; anything between them is
  ignored, and trailing events are fine. So
  `events = [{ kind = "call", op = "chat" }]` passes as soon as the first `chat`
  is seen, even if the brain does a hundred things afterwards.
- A pattern object matches when every key it names is present and matches in the
  candidate; extra candidate keys are ignored. **String leaves are regular
  expressions** matched against the candidate string, so `"^gpt-a.*"` is a
  regex. Numbers, booleans and null are compared for equality.
- **Arrays match as ordered subsequences too**, with the same rules as `events`.
  Unlisted elements between matches are skipped and leading/trailing elements
  are ignored, so `[ "a", "b" ]` matches `[ "x", "a", "b", "y" ]`. An empty
  pattern array matches any array.
- Use `detail`/`payload` to pin only the fields you care about, and the
  `$any`/`$skip` sentinels to step over look-alikes or force the cursor forward.

### `$any` / `$skip` sentinels

The `events` list and every array inside a pattern share one vocabulary:
`{ "$any" = true }` consumes exactly one event or element, and `{ "$skip" = N }`
consumes exactly `N`, so the two behave identically. `$`-prefixed keys are
reserved and never mean a partial-match field.

```toml
[assertions.alice]
events = [
  { kind = "call", op = "chat" },
  { "$any" = true },
  { "$skip" = 1 },
  { kind = "call", op = "chat" },
]
```

An invalid regex fails at parse time with the offending pattern.

### Chat detail

`chat` and `chat_stream` calls trace `{ provider, model, messages, tools }`,
serializing the conversation the brain sent. Together with array subsequence
matching this expresses "many messages, only the tail matters":

```toml
detail = { messages = [
  { role = "system" },
  { "$skip" = 3 },
  { role = "user", content = "final" },
] }
```

`detail` stays partial, so naming only `model` (or nothing at all) still
matches.

## Seeded memory

Top-level `[memory.<agent>]` seeds the named agent's memory before its brain
first runs, so a test can fast-forward an agent to an interesting state instead
of walking it there:

```toml
[memory.alice]
handle = "seed-42"
```

The brain reads it with `memory_get` like any other value, and seeded entries
persist exactly like memory written at runtime (including across hot reloads).
This is a first-class `omw` feature, not a test-only one.

## `outcome = "asserted"`

A test usually cares about a prefix of a run, not its terminal outcome. If the
brain loops or waits forever, making it exit on its own is awkward and easy to
hang. `outcome = "asserted"` means: stop this agent as soon as its `events`
settle, and fail it on the first mismatch. The assertion verdict _is_ the
result.

Stopping is per-agent, so one agent can be checked in isolation while others run
normally. When every asserted agent has a verdict the harness forces the whole
run down, so a brain that loops or blocks can never hang a test.

## Mock back ends

The `mock` cargo feature backs the deterministic doubles. `omw-test` is built
with just the mocks, so a test config wires `kind = "mock"` for its provider,
tooling and endpoint:

- [Provider mock](./mocks/provider.md) — sequenced chat turns.
- [Tooling mock](./mocks/tooling.md) — canned tool results and resources.
- [Endpoint mock](./mocks/endpoint.md) — scripted client requests, optionally
  gated on the trace.

## Tracing

`omw-test` asserts on what agents actually saw and did, through the library's
trace channel (`host/trace.rs`, exported via the `prelude`):

```rust
pub enum TraceEvent {
  Inbound { agent: String, id: String, event: Event },
  Call { agent: String, op: String, detail: serde_json::Value },
  Outcome { agent: String, outcome: RunOutcome },
}
pub type TraceSender = tokio::sync::broadcast::Sender<TraceEvent>;
```

`run_agents_traced(cfg, watch, registries, tx)` (and the `loop_` twin) spawns a
receiver-drain task, emits one `Outcome` per agent, and returns the flattened
`Vec<TraceEvent>`; the `omw::testing` harness consumes the stream live and
groups it per agent with `host::trace::group`. The channel is `None` for
`omw-cli` and embedders, so it is zero-overhead when unset.

Embedders can drive the same machinery in-process through `omw::testing`
(`Harness`, `Assertions`, `parse`, `check`, `watch`) instead of shelling out to
the binary.
