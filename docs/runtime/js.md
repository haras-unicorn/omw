# The js runtime

The `js` runtime (`runtime::js`, kind `js`) lets an agent's brain be a
JavaScript script instead of a hand-written wasm component. It evaluates the
script on the bundled js interpreter ([Boa], a wasm component compiled into the
host at build time whose `omw.*` host imports route to the very same global
provider / tooling / bus as every other runtime).

The interpreter ships in the `omw-js` package/binary (`nix run .#omw-js` or the
`omw-js-<arch>.tar.gz` release tarball). The default `omw` binary doesn't
include it. With the `--features runtime-js` build flag it is compiled into the
host at build time instead.

[Boa]: https://boajs.dev

## Configuration

The js runtime takes no required parameters beyond the shared WASI sandbox:

```toml
[runtime.js]
kind = "js"

[[agents]]
name = "alice"
runtime = "js"
script = "brain.js"
```

A custom interpreter component can be substituted via the runtime's
`interpreter` parameter; otherwise the interpreter compiled into the binary is
used. The same WASI sandbox keys as the [wasm runtime](wasm.md) apply, flattened
alongside `interpreter`:

```toml
[runtime.js]
kind = "js"
inherit_env = true
env = { FOO = "bar" }

[[runtime.js.preopens]]
host_path = "./data"
guest_path = "/data"
perms = "read_only"
```

## The interpreter and the WIT bindings

The bundled guest (`omw-wasm-js-interpreter`) exports the `runtime` interface
(`kind` returns `js`, `run(script)` evaluates the script) and imports the `omw`
world. On startup it registers an `omw` global with three namespaces that expose
the WIT interfaces to the script:

- `omw.provider.get(name)` — returns a provider handle object whose blocking
  `chat`, streaming `chatStream`, `isOpen`, `cancel`, `listModels`, and `kind`
  entries are methods.
- `omw.tooling.get(name)` — returns a tooling handle object whose `listTools`,
  `callTool`, `isOpen`, `cancel`, `callToolBlocking`, `listResources`,
  `readResource`, `subscribeResourceList`, `subscribeResource`,
  `unsubscribeResourceList`, `unsubscribeResource`, and `kind` entries are
  methods.
- `omw.host.*` — the host helpers: `log`, `timeNow`, `timeFormat`, `waitUntil`,
  `waitFor`, `waitCron`, `cancelTimer`, `subscribeAgent`, `unsubscribeAgent`,
  `subscribeLifecycle`, `unsubscribeLifecycle`, `subscribeEndpoint`,
  `unsubscribeEndpoint`, `streamEndpoint`, `sendAgent`, `recv`, `tryRecv`,
  `newUuid`, `base64Encode`, `base64Decode`, `memoryGet`, `memorySet`,
  `memoryRemove`, `sleepFor`, `sleepUntil`, and `sleepCron`.

Handles are plain objects holding the configured `name` plus native methods, so
scripts call them method-style (`p.chatStream(...)`, `t.callTool(...)`). Method
names are camelCase only. The time functions take plain numbers: the interpreter
converts JavaScript's `f64` numbers to the WIT `u64` tick type at the boundary
(rejecting negatives, fractions truncate).

## Values in js

Events come back as objects shaped `{ id, kind, payload }`:

- `id` — the envelope's UUID;
- `kind` — one of `message`, `error`, `timer`, `chat-delta`, `chat-end`,
  `tool-result`, `resource-list-updated`, `resource-updated`,
  `endpoint-message`, `endpoint-session-end`;
- `payload` — the text for `message`/`error`, an object for `chat-delta` (with
  `content`, `tool_call` `{ id, name, arguments }`, and `finish_reason`), an
  object for `tool-result` (`{ name, arguments, value }`), a list of resource
  objects (`{ uri, name, description?, mime_type? }`) for
  `resource-list-updated`, a resource-content object
  (`{ uri, mime_type?, content }`) for `resource-updated`, an object for
  `endpoint-message` (`{ session, messages, tools }`, with `messages` a list of
  `{ role, content?, tool_call? }` objects, and `tools` a list of
  `{ name, description?, input_schema }`), an object for `endpoint-session-end`
  (`{ session, error? }`), and `null` otherwise. The `content` field holds
  actual text for textual formats and base64 for anything else — match on
  `mime_type` to tell which. Decode binary payloads with `omw.host.base64Decode`
  (which returns an array of bytes) and encode back with
  `omw.host.base64Encode`.

Tool and resource shapes additionally carry camelCase aliases (`inputSchema`,
`mimeType`) alongside the snake_case keys.

The script's completion value becomes its terminal message, stringified as JSON:
a string result is returned as-is, while arrays and objects surface as `[...]`.
`undefined` and `null` complete with no message.

## Example brain

```js
let p = omw.provider.get("openai");
let id = p.chatStream("gpt-4o", [{ role: "user", content: "say hi" }], []);

let out = "";
while (true) {
  let ev = omw.host.recv();
  if (ev.id === id && ev.kind === "chat-delta") {
    out += ev.payload.content;
  }
  if (ev.id === id && ev.kind === "chat-end") {
    break;
  }
  if (ev.kind === "error") {
    throw ev.payload;
  }
}
out;
```

The script's final value becomes its terminal message when it is not
`undefined`/`null`.

## Memory

`memoryGet` returns the value or `undefined` when absent; `memorySet` stores;
`memoryRemove` returns true when a value was present:

```js
omw.host.memorySet("timer", omw.host.waitFor(1000));
// ... after a reload, the same context still has it:
let timer = omw.host.memoryGet("timer");
```
