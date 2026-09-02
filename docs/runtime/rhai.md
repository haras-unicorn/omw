# The rhai runtime

The `rhai` runtime (`runtime::rhai`, kind `rhai`) lets an agent's brain be a
[rhai] script instead of a hand-written wasm component. It evaluates the script
on the bundled rhai interpreter — itself a wasm component compiled into the host
at build time — whose `omw.*` host imports route to the very same global
provider / tooling / bus as every other runtime.

[rhai]: https://rhai.rs

## Configuration

The rhai runtime takes no required parameters:

```toml
[runtime.rhai]
kind = "rhai"

[[agents]]
name = "alice"
runtime = "rhai"
script = "brain.rhai"
```

A custom interpreter component can be substituted either via the runtime's
`interpreter` parameter or the `OMW_RHAI_INTERPRETER` environment variable;
otherwise the interpreter compiled into the binary is used.

## The interpreter and the WIT bindings

The bundled guest (`omw-rhai-wasm-interpreter`) exports the `runtime` interface
(`kind` returns `rhai`, `run(script)` evaluates the script) and imports the
`omw` world. On startup it registers an `omw` static module with three
sub-modules that expose the WIT interfaces to the script:

- `omw::provider::get(name)` — returns a provider handle map whose `chat`,
  `is-open`, `cancel`, `models`, and `kind` entries are methods.
- `omw::tooling::get(name)` — returns a tooling handle map whose `list-tools`,
  `call-tool`, `list-resources`, `subscribe-resource-list`,
  `subscribe-resource`, and `kind` entries are methods.
- `omw::host::*` — the host helpers: `log`, `now`, `timestamp_add`,
  `timestamp_sub`, `timestamp_diff`, `timestamp_format`, `wait_timestamp` /
  `wait_duration` / `wait_cron`, `subscribe`, `send`, `recv`, `try_recv`,
  `new_uuid`.

Handles are rhai maps; methods are `FnPtr`s stored on them, so scripts call them
method-style (`p.chat(...)`, `t.call-tool(...)`). The time functions take plain
integer literals: the interpreter converts rhai's `i64` integers to the WIT
`u64` tick type at the boundary (rejecting negatives).

## Values in rhai

Events come back as maps shaped `#{ id, kind, payload }`:

- `id` — the envelope's UUID;
- `kind` — one of `message`, `error`, `timer`, `delta`, `stream-end`,
  `resource-changed`, `resource-updated`;
- `payload` — the text for `message`/`error`, a map for `delta` (with `content`,
  `tool_call` `{ id, name, arguments }`, and `finish_reason`), and unit
  otherwise.

## Example brain

```rhai
let p = omw::provider::get("openai");
p.chat("gpt-4o", [
  #{ role: "user", content: "say hi" },
], []);

let out = "";
loop {
  let ev = omw::host::recv();
  if ev.kind == "delta" { out += ev.payload.content }
  if ev.kind == "stream-end" { break }
  if ev.kind == "error" { throw ev.payload }
}
out
```

The script's final value becomes its terminal message when it is not unit.
