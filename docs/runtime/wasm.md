# The wasm runtime

The `wasm` runtime (`runtime::wasm`, kind `wasm`) loads the agent's brain as a
compiled WebAssembly _component_ implementing the exported `runtime` interface.
This is the general-purpose path: because the agent's program is baked into the
component, the brain is fully portable and the host never sees its logic.

## Configuration

The wasm runtime's brain is a file named by the agent's `script`, plus an
optional WASI sandbox (deny-by-default, like `WasiCtxBuilder`):

```toml
[runtime.wasm]
kind = "wasm"
inherit_env = true
env = { FOO = "bar" }
args = ["--flag"]
initial_cwd = "/work"

[[runtime.wasm.preopens]]
host_path = "./data"
guest_path = "/data"
perms = "read_write" # or "read_only" (default)
```

Available keys: `inherit_stdio` (shorthand for all three below), `inherit_stdin`
/ `inherit_stdout` / `inherit_stderr`, `inherit_env`, `env`, `inherit_args`,
`args`, `initial_cwd`, `preopens`, `allow_blocking_current_thread`,
`insecure_random_seed`, `max_random_size`, `allow_tcp` / `allow_udp` /
`allow_ip_name_lookup`, and `inherit_network` (enables all three network flags
with a permissive address check). The whole block is per named `[runtime.*]`
entry: to give another agent different sandboxing, declare another runtime and
point the agent at it.

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

## Rust brains

A pure Rust brain is a library crate depending on the `omw-wasm-rust` guest SDK,
which re-exports the WIT bindings plus typed `Provider`/`Tooling` handles,
`host` helpers, and RAII guards. There is no single-file support: keep the brain
a real crate so rust-analyzer keeps working.

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
omw-wasm-rust = "0.1"
```

```rust
// src/lib.rs
#![no_main]

omw_wasm_rust::brain!(|| {
  omw_wasm_rust::host::info("hello from a rust brain");
  Ok(())
});
```

Build it for `wasm32-wasip2` and point this runtime at the component:

```sh
cargo build --target wasm32-wasip2
```

```toml
[runtime.wasm]
kind = "wasm"

[[agents]]
name = "server"
runtime = "wasm"
script = "target/wasm32-wasip2/debug/brain.wasm"
```

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
