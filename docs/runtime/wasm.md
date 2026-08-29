# The wasm runtime

The `wasm` runtime (`runtime::wasm`, kind `wasm`) loads the agent's brain as a
compiled WebAssembly _component_ implementing the exported `runtime` interface.
This is the general-purpose path: because the agent's program is baked into the
component, the brain is fully portable and the host never sees its logic.

## Configuration

The wasm runtime takes no parameters — the brain is a file named by the agent's
`script`:

```toml
[runtime.wasm]
kind = "wasm"

[[agents]]
name = "server"
runtime = "wasm"
script = "brain.wasm"
```

The file may be `.wat` (text), `.wasm` (binary), or `.cwasm` (AOT-compiled, also
the fastest to load). The component model is enabled, and the component must
export the `omw.runtime` interface.

## The engine

The shared `WasmEngine` (in `runtime::engine`) is deliberately generic: it has
no knowledge of any particular brain. It:

1. loads the component from the file (WAT, WASM, or AOT-cached),
2. builds a `Store` over the host `Host` (agent context + resource table + WASI
   context),
3. wires the host imports into a `Linker` — the WASI wasip2 imports and the
   `provider` / `tooling` / `host` interfaces,
4. instantiates the component,
5. calls `runtime.run(script)` and returns the terminal message.

Because it is synchronous, the engine runs on a `spawn_blocking` thread rather
than a tokio worker, so the host imports' use of `rt.block_on` stays legal.
`WasmEngine` is `Clone`, so one loaded component is reused across iterations.
