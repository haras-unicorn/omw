# The python runtime

The `python` runtime (`runtime::python`, kind `python`) lets an agent's brain be
a Python script instead of a hand-written wasm component. It evaluates the
script on the bundled python interpreter ([RustPython], a wasm component
compiled into the host at build time whose `omw.*` host imports route to the
very same global provider / tooling / bus as every other runtime).

The interpreter ships in the `omw-python` package/binary (`nix run .#omw-python`
or the `omw-python-<arch>.tar.gz` release tarball). The default `omw` binary
doesn't include it. With the `--features runtime-python` build flag it is
compiled into the host at build time instead.

[RustPython]: https://rustpython.github.io

## Configuration

The python runtime takes no required parameters beyond the shared WASI sandbox:

```toml
[runtime.python]
kind = "python"

[agents.alice]
runtime = "python"
script = "brain.py"
```

A custom interpreter component can be substituted via the runtime's
`interpreter` parameter; otherwise the interpreter compiled into the binary is
used. The same WASI sandbox keys as the [wasm runtime](wasm.md) apply, flattened
alongside `interpreter`:

```toml
[runtime.python]
kind = "python"
inherit_env = true
env = { FOO = "bar" }

[[runtime.python.preopens]]
host_path = "./data"
guest_path = "/data"
perms = "read_only"
```

## The interpreter and the WIT bindings

The bundled guest (`omw-wasm-python-interpreter`) exports the `runtime`
interface (`kind` returns `python`, `run(script)` evaluates the script) and
imports the `omw` world. On startup it installs an `omw` global with three
namespaces that expose the WIT interfaces to the script (no import needed):

- `omw.provider.get(name)` — returns a provider handle object whose blocking
  `chat`, streaming `chat_stream`, `is_open`, `cancel`, `list_models`, and
  `kind` entries are methods. `chat`/`chat_stream` take an optional trailing
  `params` argument: either a dict of generation settings
  (`{"temperature": 0.2, "reasoning_effort": "high"}`) or an already-JSON string
  (e.g. an `endpoint-message`'s `params`), merged over the provider's configured
  defaults.
- `omw.tooling.get(name)` — returns a tooling handle object whose `list_tools`,
  `call_tool`, `is_open`, `cancel`, `call_tool_blocking`, `list_resources`,
  `read_resource`, `subscribe_resource_list`, `subscribe_resource`,
  `unsubscribe_resource_list`, `unsubscribe_resource`, and `kind` entries are
  methods.
- `omw.host.*` — the host helpers: `log`, `whoami`, `time_now`, `time_format`,
  `wait_until`, `wait_for`, `wait_cron`, `cancel_timer`, `subscribe_agent`,
  `unsubscribe_agent`, `subscribe_lifecycle`, `unsubscribe_lifecycle`,
  `subscribe_endpoint`, `unsubscribe_endpoint`, `stream_endpoint`, `send_agent`,
  `recv`, `try_recv`, `new_uuid`, `base64_encode`, `base64_decode`,
  `memory_get`, `memory_get_as`, `memory_set`, `memory_set_as`, `memory_remove`,
  `sleep_for`, `sleep_until`, and `sleep_cron`.

Handles are plain objects holding the configured `name` plus native methods, so
scripts call them method-style (`p.chat_stream(...)`, `t.call_tool(...)`).
Method names are snake_case only. The time functions take plain ints.

## Values in python

Events and records come back as attribute objects (`types.SimpleNamespace`
instances), so a script reads `ev.kind`, `ev.payload.content`, or
`result.tool_calls[0]`. Envelopes are shaped `{id, kind, payload}`:

- `id` — the envelope's UUID;
- `kind` — one of `message`, `error`, `timer`, `chat-delta`, `chat-end`,
  `tool-result`, `resource-list-updated`, `resource-updated`,
  `endpoint-message`, `endpoint-session-end`;
- `payload` — the text for `message`/`error`, an object for `chat-delta` (with
  `content`, `reasoning`, `tool_call` `{id, name, arguments}`, `finish_reason`,
  and `usage` `{prompt_tokens?, completion_tokens?, total_tokens?}`), an object
  for `tool-result` (`{name, arguments, content, structured_content?}`), a list
  of resource objects (`{uri, name, description?, mime_type?}`) for
  `resource-list-updated`, a resource-content object
  (`{uri, mime_type?, content}`) for `resource-updated`, an object for
  `endpoint-message` (`{session, messages, tools, params?}`, with `messages` a
  list of `{role, content?, reasoning?, tool_call?}` objects, `tools` a list of
  `{name, description?, input_schema, output_schema?}`, and `params` the
  generation params the client submitted), an object for `endpoint-session-end`
  (`{session, error?}`), and `None` otherwise. The `content` field holds actual
  text for textual formats and base64 for anything else — match on `mime_type`
  to tell which. Decode binary payloads with `omw.host.base64_decode` (which
  returns `bytes`) and encode back with `omw.host.base64_encode`.

Opaque JSON fields are parsed in place: `arguments`, `input_schema`,
`output_schema`, `params`, and a tool result's `content`/`structured_content`
surface as objects/lists when the stored text is valid JSON and as the raw
string otherwise. `content` is the MCP content-block array, so a text result's
text is `result.content[0].text`. Passing one of these values back into
`chat`/`chat_stream`/`call_tool` re-serializes it (records serialize through
their attributes), so events round-trip.

A script that parses as a single expression is evaluated as one, and its value
becomes the brain's terminal message (stringified with `str`); anything else
runs as statements and completes with no message. A `None` result completes with
no message either.

## Example brain

```python
p = omw.provider.get("openai")
id = p.chat_stream("gpt-4o", [{"role": "user", "content": "say hi"}], [])

out = ""
while True:
  ev = omw.host.recv()
  if ev.id == id and ev.kind == "chat-delta":
    out += ev.payload.content
  if ev.id == id and ev.kind == "chat-end":
    break
  if ev.kind == "error":
    raise RuntimeError(ev.payload)
out
```

## Type declarations

The interpreter ships three declaration files:

- `assets/schema.py` and `assets/schema.test.py` — pydantic v2 models generated
  from the deployment and testing JSON schemas with
  [`datamodel-code-generator`](https://github.com/koxudaxi/datamodel-code-generator).
- `src/wasm/omw-wasm-python-interpreter/omw.pyi` — the hand-written stubs for
  the `omw` global.
- `src/wasm/omw-wasm-python-interpreter/omw.all.py` — the three above combined:
  the deployment models under `OmwConfig`, the testing models under
  `OmwTestConfig`, and the `omw` stubs at module level. Vendor it next to your
  `brain.py`.

The generated config models are namespaced: `OmwConfig.Config` describes the
`omw` deployment config, and `OmwTestConfig.Config` the `omw-test` config.

Python only resolves local files, so vendor a copy next to your `brain.py` and
reference it from your type checker (`mypy`, `pyright`):

```sh
curl -L -o omw.all.py \
  https://raw.githubusercontent.com/haras-unicorn/omw/main/src/wasm/omw-wasm-python-interpreter/omw.all.py
```

## Memory

`memory_get` returns the raw string or `None` when absent; `memory_set` stores a
string verbatim; `memory_remove` returns `True` when a value was present. The
`_as` pair handles structured values: `memory_set_as` stores any
JSON-serializable value (a string is JSON-encoded), and `memory_get_as` parses
the stored value back into an object/array/scalar, falling back to the raw
string:

```python
omw.host.memory_set_as("state", {"step": 3, "waiting": True})
# ... after a reload, the same context still has it:
state = omw.host.memory_get_as("state")
step = state.step
```
