# Testing

`omw-test` is the deterministic brain-testing binary. It runs each discovered
`omw.test.toml` through the same traced path the `omw` binary uses
(`run_agents_traced`), against in-process scripted doubles (`kind = "mock"`),
records what every agent saw and did, and checks that recording against an
`[assertions]` section. No keys, no network, no external services.

A test config is a normal `omw.toml`-shaped file named `omw.test.toml`: the same
config `omw` consumes, plus an `[assertions]` table that stock `omw` ignores the
same way it ignores unknown keys. Test configs can be TOML, YAML or JSON
(`omw.test.{toml,yaml,yml,json}`). Values can be layered from the environment
with the `OMW_TEST__` prefix, exactly like `OMW__` for `omw` itself.

## Running

```sh
omw-test run examples/01-hello   # one config or directory
omw-test run examples            # every discovered config
```

- `<path>` is a **file** → run that config once.
- `<path>` is a **directory** → recursively find every test config and run each,
  printing `PASS` / `FAIL <root-relative path>` and a tally. It exits non-zero
  if any failed.
- `<path>` is a **standard stream** (`-`, `/dev/stdin`) → read a single config
  from stdin and run it; pass `--format` since a stream has no extension to
  infer from. Relative brain `script` paths resolve against the current
  directory.
- `--include <glob>` / `--exclude <glob>` (repeatable, OR within each) match the
  test's root-relative path including its file name (`*` does not cross `/`,
  `**` does); include is applied first, then exclude. A missing `script` is
  always a failure, never a skip; narrow the set with the globs instead.
- `--format <fmt>` forces the config format (`toml`, `yaml` or `json`) for every
  loaded config; without it, each file's format is inferred from its extension.
- Shared wiring lives in an `omw.test.base.<ext>` file that, by convention,
  applies to the test configs in its directory and below. See
  [Base configs](#base-configs).
- `--all` keeps going after a failure and reports every failing test. Without it
  `omw-test` is fail-fast: it stops at the first failure and prints that test's
  detail.
- `-j <N>` / `--jobs <N>` runs up to `N` tests concurrently. It defaults to the
  machine's logical core count (`-j 0` also means auto), so a suite tests in
  parallel out of the box; `-j 1` is the sequential behavior. Under fail-fast
  with more than one job, a failure stops launching new tests but the tests
  already in flight finish and report, so the tally can cover more than the
  single first failure.
- `--dump <path>` writes the per-test traces and mock queue snapshots as
  machine-readable data (`-` / `/dev/stdout` streams them), for passing and
  failing tests alike. Each entry carries the test label, its verdict, each
  agent's observed events, assertion cursor and diff, and the mock back ends'
  consumed-vs-remaining scripted queues plus their per-call returns. An empty
  discovery writes an empty `{ "tests": [] }`. `--dump-format json|yaml|toml`
  selects the encoding; it defaults to `json`, which keeps a piped dump
  `jq`-able (`toml` drops nulls, which it cannot represent).
- `--watch` re-runs on change instead of exiting: after each pass it waits for a
  debounced filesystem event, then only re-runs the tests that change affects —
  a test whose config, one of its inherited base configs, or one of its agents'
  brain scripts was touched (plus any newly discovered config). A change that
  affects no test is logged and skipped instead of re-running everything. The
  tty view keeps every discovered test listed with its last verdict, so
  unaffected tests still show their previous pass/fail. The library hot-reload
  watch is always off.

### Failure diagnostics

A passing test needs no explanation; a failing or hanging one needs the state
that produced it.

- **Timeout.** `tunables.test_timeout_secs` (default 30, `0` disables) bounds
  each test's harness run. On expiry the run is force-stopped and every
  unsettled agent is marked timed out, so a hang is unusable rather than an
  indefinite wait.
- **Trace state.** A failure shows each observed event with an index next to the
  assertion cursor — the next assertion the matcher was waiting on.
- **Host/runtime state.** Every source the brain opens emits `opened` / `closed`
  (with a reason: ended, cancelled or failed) trace events, and each
  handle-returning call carries the UUID it returned as its `uuid` field, so
  "opened but delivered nothing" is visible.
- **Mock queue state.** On failure the mock provider/tooling/endpoint report
  their consumed vs remaining turns/`tool_calls`/`requests` (real back ends
  report nothing). `--dump` writes the same snapshots plus each mock's per-call
  returns (provider deltas, tooling results, endpoint replies).
- **Settle signal.** The instant an `outcome = "asserted"` agent's assertions
  settle, a per-agent line is logged, so a hang is unmistakably a hang rather
  than a pending pass.
- **Durable tally.** An interactive run replays the per-test `PASS`/`FAIL` lines
  and the final tally to stdout once the live view tears down, so the result
  stays in scrollback instead of only flashing on the alternate screen.

## Output

Verdicts are the test runner's product, so they go to **stdout**; diagnostics go
to **stderr** (or the journal), so `omw-test run | ...` stays parseable while
logs stream separately.

- `--log-format auto|tty|pipe|journald` selects how logs render. `auto` picks
  `tty` when both stdout and stderr are terminals, else `journald` under systemd
  (`JOURNAL_STREAM` is set), else `pipe` (JSONL on stderr).
- On a terminal the **live view** owns the screen: an info panel showing the
  launched command and the discovery path/test count, a list of every discovered
  test marked `✅`/`❌` (with a spinner on the running one), a logs pane for the
  running test — or the failed test's assertion diff (expected vs observed)
  beside its buffered logs — and a bordered `N/M tests` gauge.
- In `pipe` the same verdict lines are plain: one `PASS` / `FAIL <label>` per
  test plus the final tally on stdout.
- `--watch` keeps every discovered test listed with its last verdict across
  passes and only advances the affected tests' state; the gauge covers the tests
  running in the current pass.

See [Output](../output.md) for the full policy, the `RUST_LOG` level recipes and
the per-command table.

## Schema

```sh
omw-test schema --output schema.test.json
omw-test schema --output -             # or /dev/stdout
```

Writes the JSON schema for the test config, describing exactly the back ends the
`omw-test` build enables — the `mock` provider/tooling/endpoint doubles and the
`wasm`/`rhai`/`js` runtimes — so an editor can complete and validate an
`omw.test.toml`. `-` (or `/dev/stdout`) streams it to stdout. The deployment
counterpart is `omw schema` (see
[Configuration schema](../introduction.md#configuration-schema)).

Discovery skips hidden directories and collects any file whose stem is
`omw.test` or ends with `.omw.test` and whose extension is `toml`, `yaml`, `yml`
or `json`, so several test configs can live side by side in one directory. A
shared base config (`omw.test.base.<ext>`) is never matched, since its stem ends
in `.base`.

## Scaffolding

`omw scaffold` (in the `omw` binary) turns a deployment config into a starter
test config: it introspects the real back ends and writes an `omw.test.toml`
whose provider, tooling and endpoint are the in-config mocks, pre-populated
where possible.

```sh
omw scaffold omw.toml                 # writes ./omw.test.toml
omw scaffold omw.yaml --format yaml   # explicit input format
omw scaffold omw.toml --output t.toml # explicit output
omw scaffold omw.toml --output -      # or /dev/stdout, stream to stdout
omw scaffold - --format toml          # read the config from stdin
omw scaffold omw.toml --no-resources  # skip listing/reading tooling resources
omw scaffold omw.toml --force         # overwrite an existing output
```

A standard stream is never treated as an existing file, so `--output -` (or
`/dev/stdout`) needs no `--force`, and the informational path line is omitted so
only the config reaches stdout. A config read from stdin (`-`, `/dev/stdin`)
needs `--format`.

- the **provider** mock gets the models the endpoint reported (`GET /models`,
  empty if the request fails), with an empty `turns` script;
- the **tooling** mock gets the server's `tools` (with their input schemas),
  `initial_resource_list`, and `initial_resource_contents` (unless
  `--no-resources`), with an empty `tool_calls` script;
- the **endpoint** mock gets an empty `requests` list;
- `runtime`, `agents`, `[memory]` and `[tunables]` are copied through verbatim.

Everything is best-effort: a back end that cannot be built or enumerated yields
an empty mock and a warning instead of failing the conversion. The original
params are never carried over, so secrets do not end up in the output. Fill in
the `turns`, `tool_calls` and `requests` to script the run.

## Base configs

A _base config_ is an ordinary `omw.toml`-shaped file (TOML, YAML or JSON) named
`omw.test.base.<ext>` that contributes both the runnable config and its
`[assertions]`. When a directory is discovered, each test config inherits the
base configs in its own directory and in every directory between it and the
discovery root:

```text
base config(s) (outermost first)  →  test config  →  OMW_TEST__ overlay
```

Each later source overrides the earlier ones, so the test config overrides the
base configs it inherits, and the environment overrides them all. Merging is a
deep table merge: a test config's `[agents.<name>]` or `[assertions.<agent>]`
overrides the base config's keys for that entry while inheriting the rest. A
base config's relative `script` resolves against its own directory, so it always
points where it was written. The format of every file is inferred from its
extension, or forced with `--format` when a file has none.

A base only reaches **down** the tree, never above the path passed to
`omw-test run`. Running `omw-test run cases` picks up `cases/omw.test.base.toml`
for every test below it (and `cases/<case>/omw.test.base.toml` for that case),
while a directory that has no base of its own simply inherits its ancestors'.
`omw.test.base.*` is never collected as a test config, and `--include` /
`--exclude` filter the tests independently.

This lets a suite share its wiring and keep each test minimal — usually only the
`[runtime.*]` kind and each agent's `script` differ per variant:

```toml
# cases/01-hello/omw.test.base.toml — shared wiring
[providers.openai]
kind = "mock"
turns = [{ content = "hello, world" }]

[agents.alice]
runtime = "runtime"

[assertions.alice]
outcome = "completed"
events = [{ kind = "call", op = "chat" }]
```

```toml
# cases/01-hello/rhai/omw.test.toml — only what differs
[runtime.runtime]
kind = "rhai"

[agents.alice]
script = "brain.rhai"
```

```sh
omw-test run cases/01-hello
```

The [examples](../examples.md) use exactly this layout: one `omw.test.base.toml`
per case and a shrunk `omw.test.toml` per variant.

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
    `op` is a partial pattern over the op name, so its string is a regex (like
    every other string leaf); `detail` is a partial pattern over the call's JSON
    detail.
  - `{ kind = "inbound", event = "...", payload = { ... } }` — an inbox event.
    `event` is the kebab-case kind (`chat-delta`, `chat-end`, `tool-result`,
    `endpoint-message`, `message`, `timer`, `reload`, `shutdown`, `error`, …);
    `payload` is a partial pattern over the serialized event.
  - `{ kind = "opened", source = "..." }` — a source opened: the pump-backed
    `chat-stream`, `timer`, `resource-list`, `resource` and `tool-call`, or the
    bus-backed `endpoint`, `agent` and `lifecycle` subscriptions.
  - `{ kind = "closed", source = "...", reason = "..." }` — a source closed,
    with the reason it did (`ended`, `cancelled`, `failed`). Both fields are
    optional patterns. There is deliberately no UUID or agent match: a random
    UUID is meaningless to assert on, so the source kind and reason are the
    meaningful fields.
  - `{ "$while" = { kind = "call", ... } }` — greedily consume a run of matching
    trace events, stopping at the first non-match.
  - `{ "$until" = { kind = "call", ... } }` — skip trace events until one
    matches, consuming it.

The list is matched as an **ordered subsequence** over **partial patterns**:

- Only the events you write are checked, in order; anything between them is
  ignored, and trailing events are fine. So
  `events = [{ kind = "call", op = "chat" }]` passes as soon as the first `chat`
  is seen, even if the brain does a hundred things afterwards.
- A pattern object matches when every key it names is present and matches in the
  candidate; extra candidate keys are ignored. **String leaves are regular
  expressions** matched against the candidate string, so `"^gpt-a.*"` is a
  regex. Numbers, booleans and null are compared for equality. `op` follows the
  same rule, so `op = "chat"` also matches `chat_stream`; anchor it with
  `"^chat$"` when you want an exact op.
- **Arrays match as ordered subsequences too**, with the same rules as `events`.
  Unlisted elements between matches are skipped and leading/trailing elements
  are ignored, so `[ "a", "b" ]` matches `[ "x", "a", "b", "y" ]`. An empty
  pattern array matches any array.
- Use `detail`/`payload` to pin only the fields you care about, and the
  `$while`/`$until` sentinels to step over look-alikes or force the cursor
  forward.

### `$while` / `$until` sentinels

The `events` list and every array inside a pattern share one vocabulary.
`{ "$while" = P }` greedily consumes a run of consecutive elements matching the
inner `P`, stopping at the first non-match (zero-or-more). `{ "$until" = P }`
skips ahead to the first element matching `P` and consumes it. Under `events`,
`P` is a `call`/`inbound`/`opened`/`closed` assertion; inside a pattern array it
is an ordinary pattern. `$`-prefixed keys are reserved and never mean a
partial-match field.

```toml
[assertions.alice]
events = [
  { kind = "call", op = "chat" },
  { "$while" = { kind = "inbound", event = "chat-delta" } },
  { "$until" = { kind = "call", op = "chat" } },
]
```

An empty object under either sentinel is an ordinary pattern that matches any
object, so `{ "$until" = {} }` consumes the next object and `{ "$while" = {} }`
consumes a run of objects (objects only: `{}` does not match a primitive). An
invalid regex fails at parse time with the offending pattern.

### Chat detail

`chat` and `chat_stream` calls trace `{ provider, model, messages, tools }`,
serializing the conversation the brain sent. Together with array subsequence
matching this expresses "many messages, only the tail matters":

```toml
detail = { messages = [
  { role = "system" },
  { "$while" = { role = "assistant" } },
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
state = { step = 3, waiting = true }
```

A string seed is stored verbatim; a table, array, number or bool is
JSON-stringified. The brain reads a string with `memory_get` or a structured
value with `memory_get_as` (which parses it back), like any other value. Seeded
entries persist exactly like memory written at runtime (including across hot
reloads). This is a first-class `omw` feature, not a test-only one.

## `outcome = "asserted"`

A test usually cares about a prefix of a run, not its terminal outcome. If the
brain loops or waits forever, making it exit on its own is awkward and easy to
hang. `outcome = "asserted"` means: stop this agent as soon as its `events`
settle, and fail it on the first mismatch. The assertion verdict _is_ the
result.

Stopping is per-agent, so one agent can be checked in isolation while others run
normally. When every asserted agent has a verdict the harness forces the whole
run down, so a brain that loops or blocks can never hang a test.

Because a `$while` is zero-or-more, a **trailing** `$while` settles as soon as
its prefix does (immediately for an assertion that is only a `$while`, exactly
like an empty `events` list), so it asserts nothing on its own. Bound a run you
care about with a following anchor — `$until` or a plain assertion — which is
also what makes the run meaningful under `outcome = "asserted"`:

```toml
[assertions.alice]
outcome = "asserted"
events = [
  { "$while" = { kind = "inbound", event = "chat-delta" } },
  { kind = "inbound", event = "chat-end" },
]
```

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
  Call { agent: String, op: String, uuid: Option<String>, detail: serde_json::Value },
  Opened { agent: String, source: SourceKind, uuid: String },
  Closed { agent: String, source: SourceKind, uuid: String, reason: CloseReason },
  Outcome { agent: String, outcome: RunOutcome },
}
pub type TraceSender = tokio::sync::broadcast::Sender<TraceEvent>;
```

`source` is the source kind — the pump-backed `chat-stream`, `timer`,
`resource-list`, `resource` and `tool-call`, plus the bus-backed `endpoint`,
`agent` and `lifecycle` subscriptions; `reason` is `ended`, `cancelled` or
`failed`. The lifecycle events also double as assurance that a subscription was
actually opened: a rejected duplicate opens nothing and emits nothing.

`run_agents_traced(cfg, watch, registries, tx)` (and the `loop_` twin) spawns a
receiver-drain task, emits one `Outcome` per agent, and returns the flattened
`Vec<TraceEvent>`; the `omw::testing` harness consumes the stream live and
groups it per agent with `host::trace::group`. The channel is `None` for
`omw-cli` and embedders, so it is zero-overhead when unset.

Embedders can drive the same machinery in-process through `omw::testing`
(`Harness`, `Assertions`, `parse(source, Format)`, `check`, `watch`) instead of
shelling out to the binary.
