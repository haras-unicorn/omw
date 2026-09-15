# Library

`omw` can be embedded in your own crate. Parse a `Config`, build a `Registries`
value, register any custom back ends, then call `run_agents` or `loop_agents`.

Add the dependency with the back ends you want:

```toml
[dependencies]
omw = "*"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The default features are `runtime-wasm`, `provider-openai`, `tooling-mcp`, and
`endpoint-openai`. The `runtime-rhai` and `runtime-js` script runtimes are
opt-in features; a `--no-default-features` build yields empty registries.
(Runnable `examples/` programs land in a separate PR.) The CLI-only stack
(`clap`, `config`, `tracing-subscriber`) lives in the separate `omw-cli` crate,
so library consumers never pull it in.

## TLS setup

`omw` follows the standard Rust contract: features select where TLS comes from,
the final binary installs it. The `provider-openai` and `tooling-mcp` features
imply `rustls`, which links the `ring` crypto backend; `reqwest` is built on
`rustls-no-provider`, so the embedding binary must install exactly one
process-global crypto provider once before running agents:

```rust
if rustls::crypto::CryptoProvider::get_default().is_none() {
  rustls::crypto::ring::default_provider()
    .install_default()
    .expect("another crate installed a crypto provider");
}
```

Install your own provider instead (for example `aws-lc-rs`) when you prefer a
different backend; `omw` never installs or overwrites one itself — the `ring`
setup lives in the `omw-cli` binary only. A `--no-default-features` build
without the provider/tooling features is TLS-free.

Certificate trust is orthogonal to the crypto backend: verification uses
`rustls-platform-verifier`, which on Linux loads the system CA bundle once at
startup (honoring `SSL_CERT_FILE`). Static binaries therefore still trust
whatever the host distribution trusts, with no Mozilla bundle baked in. The
bundled `zstd` (via `wasmtime`) needs no action either: downstream builds can
set `ZSTD_SYS_USE_PKG_CONFIG=1` or unify `zstd-sys` features when they want the
system library instead.

## Embed with defaults

```rust
use omw::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let raw: String = std::fs::read_to_string("omw.toml")?;
  let cfg: Config = toml::from_str(&raw)?;
  let registries = Registries::default();
  run_agents(&cfg, false, &registries).await
}
```

`Registries::default()` carries the feature-gated built-ins, one per family.
`Registries::new()` is the same shape with no built-ins. Use `loop_agents` to
restart agents on failure instead of running once. `Config` is impl-agnostic
(`kind` plus opaque params per entry), so unknown `kind` values fail at `build`
time with the list of registered kinds.

## Custom back ends

Each family (`provider`, `tooling`, `runtime`, `endpoint`) has a back-end trait,
a `Factory` trait with one `build` method, an opaque `*Entry` handle, a
`Registry`, and a `register_*` macro. Implement the back-end trait plus
`Factory`, add one macro line, reference the `kind` from config. Built-in impl
`Config` structs stay private: `build` deserializes the opaque
`serde_json::Value` itself.

Custom provider registered by type:

```rust
use omw::prelude::*;

struct MyProvider { model: String }

impl Provider for MyProvider {
  fn kind() -> &'static str { "my-llm" }

  async fn list_models(&self) -> Vec<String> {
    vec![self.model.clone()]
  }

  async fn chat_stream(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> anyhow::Result<
    futures_util::stream::BoxStream<
      'static,
      Result<ChatDelta, String>,
    >,
  > {
    // Call any API, map chunks to `ChatDelta`, return the stream.
    // Return `Err` before the first delta on auth or transport
    // failure, per the provider contract.
    todo_stream()
  }
}

impl omw::provider::Factory for MyProvider {
  fn build(
    name: &str,
    params: &serde_json::Value,
  ) -> anyhow::Result<std::sync::Arc<Self>> {
    let model: String = params
      .get("model")
      .and_then(|v| v.as_str())
      .unwrap_or("my-model")
      .to_owned();
    Ok(std::sync::Arc::new(Self { model }))
  }
}

// In `main`, before running agents:
let mut registries = Registries::default();
omw::register_providers!(registries.providers, MyProvider);
```

```toml
[providers mine]
kind = "my-llm"
model = "my-model"
```

The other families follow the same pattern with their own `build` shape:
`tooling::Factory::build` takes `(name, params, tunables)`,
`runtime::Factory::build` takes `(name, params)`, and `endpoint::Factory::build`
takes just `(params)`. A test double without config plumbing uses the closure
path:

```rust
let echo: Arc<MyFakeProvider> = Arc::new(MyFakeProvider::new());
registries.providers.register_factory("fake", move |name, _params| {
  Ok(echo.clone() as Arc<dyn Provider>)
});
```

Both `register::<T>()` and `register_factory` reject a duplicate `kind` with an
error, never overwrite.

## Lazy tooling

Tooling connects lazily on first use instead of at build time, so every family
shares one sync factory shape. Connection errors surface on first tool or
resource use: blocking callers get `Err`, event-driven callers get the `error`
event variant in the agent inbox. Retries use the
`tooling_connect_backoff_start_ms` (default `100`) and
`tooling_connect_backoff_cap_secs` (default `30`) tunables, doubling up to the
cap. See [tunables](./tunables.md).

## Prelude

`use omw::prelude::*;` re-exports the embedding subset: `Config`, `AgentConfig`,
`ImplConfig`, `Tunables`, `Registries`, the four back-end traits plus their
`Factory` traits aliased as `ProviderFactory` / `ToolingFactory` /
`RuntimeFactory` / `EndpointFactory`, the DTOs (`Role`, `ChatMessage`,
`ChatDelta`, `ChatResult`, `ToolCall`, `Tool`, `ResourceInfo`,
`ResourceContent`, `ResourceNotification`), the `*Entry` handles, `RunOutcome`,
`Event` / `EventEnvelope`, `AgentContext` (`name()` only), `Secret`, `Shutdown`,
`run_agents` / `loop_agents`, plus the `register_*` macros. The macros are also
`#[macro_export]` at the crate root (`omw::register_providers!`, …).
