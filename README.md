# OMW

<!-- ANCHOR: body -->

OMW = OpenAI + MCP + WASM.

`omw` is an agent runtime. You declare agents in a TOML configuration, each
wiring a _provider_ (an OpenAI-family chat service), _tooling_ (MCP tool
servers), and a _brain_ — either a compiled WASM component, a [rhai] script, or
a JavaScript script. `omw` then drives each agent through an actor model: every
agent owns a single inbox, and chat streams, tool results, timers, and messages
from other agents all arrive there as tagged events the brain consumes.

[rhai]: https://rhai.rs

## Who it is for

`omw` is for people who want a small, local agent runtime that is genuine about
its inputs and outputs: the brain is real WASM, tooling speaks MCP, and the
configuration is plain TOML. It is not a framework — there is no DSL to learn
and no orchestration layer. You bring a provider key, a couple of MCP servers,
and a brain, and `omw` runs it for one iteration (`run`) or keeps it going
(`loop`).

## How it works

- **Providers** are OpenAI-family chat services. `provider.chat-stream` opens a
  _streaming_ response whose deltas arrive as events in the agent's inbox; the
  in-band `provider.chat` blocks for the full result.
- **Tooling** is MCP tool servers. They expose callable _tools_ and readable
  _resources_; resource subscriptions deliver change events.
- **Brains** are runtimes. The `wasm` runtime loads an agent as a compiled
  component; the `rhai` runtime evaluates a script on an interpreter that ships
  as an opt-in flavor, as does the `js` runtime. The default `omw`
  package/binary ships with the `wasm`, `openai`, and `mcp` back ends but
  without either script runtime, whereas the `omw-rhai` package /
  `omw-rhai-<arch>.tar.gz` binary (`--features rhai`) includes the rhai
  interpreter and the `omw-js` package / `omw-js-<arch>.tar.gz` binary
  (`--features js`) includes the js interpreter. All three see the same `omw`
  host interface (rhai in snake_case, js in camelCase). To write a pure Rust
  brain, depend on the `omw-wasm-rust` guest SDK crate instead of running
  `wit-bindgen` yourself; see [Rust brains].
- **Agents** are actors. They subscribe to each other explicitly, so a message
  only ever reaches an agent that chose to listen.
- **Endpoint** is an optional OpenAI-compatible HTTP server. Set `[endpoint]`
  with a `listen` address and agents can subscribe themselves under model names:
  inbound chat requests arrive in the agent's inbox as events, and the agent
  streams its reply back (SSE or buffered JSON). Any OpenAI-compatible client
  can then drive an agent.
- **Hot reload** is `--watch` on `run` / `loop`. When a brain script changes,
  the agent's run restarts on the new script while inboxes, subscriptions, and
  sessions survive. The new script is validated before the live run ends, so a
  bad edit never kills a good run — and a broken script never starts.

## Installation

`omw` is packaged as a Nix flake. Run it directly without installing:

```sh
nix run github:haras-unicorn/omw
```

or build the `omw` binary with:

```sh
nix build github:haras-unicorn/omw
```

### Releases

Prebuilt binaries for `x86_64-linux` and `aarch64-linux` are attached to each
[GitHub release] as tarballs containing the `omw` binary. The default
`omw-<arch>.tar.gz` ships no rhai runtime; grab the `omw-rhai-<arch>.tar.gz`
tarball (or the rhai Nix package) when your brains are rhai scripts, or the
`omw-js-<arch>.tar.gz` tarball (or the js Nix package) when your brains are
JavaScript scripts:

```sh
curl -L -o omw.tar.gz \
  https://github.com/haras-unicorn/omw/releases/latest/download/omw-x86_64-linux.tar.gz
tar -xzf omw.tar.gz
./omw-x86_64-linux
```

The rhai flavor is the same shape, with the `-rhai` name:

```sh
curl -L -o omw-rhai.tar.gz \
  https://github.com/haras-unicorn/omw/releases/latest/download/omw-rhai-x86_64-linux.tar.gz
tar -xzf omw-rhai.tar.gz
./omw-rhai-x86_64-linux
```

The js flavor is the same shape, with the `-js` name:

```sh
curl -L -o omw-js.tar.gz \
  https://github.com/haras-unicorn/omw/releases/latest/download/omw-js-x86_64-linux.tar.gz
tar -xzf omw-js.tar.gz
./omw-js-x86_64-linux
```

[GitHub release]: https://github.com/haras-unicorn/omw/releases

## Usage

Configuration lives in a TOML file (default `omw.toml` in the current directory,
overridable with `--config`). It declares named providers, tooling, and runtimes
plus a list of agents:

```toml
[providers.openai]
kind = "openai"
api_key = "sk-…"
model = "gpt-4o"

[tooling.mcp]
kind = "mcp"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-everything"]

[runtime.rhai]
kind = "rhai"

[[agents]]
name = "alice"
runtime = "rhai"
script = "brain.rhai"
```

Then drive it:

```sh
omw run    # run every agent once
omw loop   # keep every agent running, restarting on failure
```

Serve agents over HTTP with the optional [endpoint]: add a `listen` address,
have a brain subscribe itself under a model name, then any OpenAI-compatible
client can call it:

```toml
[endpoint]
listen = "127.0.0.1:8080"
```

```rhai
let sub = omw::host::subscribe_endpoint("gpt-4o");
```

Inbound requests arrive in the agent's inbox as `endpoint-message` events; the
brain streams its reply back with `stream_endpoint` (SSE for `stream: true`, one
buffered JSON completion otherwise). `GET /v1/models` lists subscribed models.

Edit brains live with `--watch` on either mode: when a brain file changes, the
agent's current run ends and restarts on the new script, while inboxes,
subscriptions, and sessions survive on the shared bus. The new script is
validated _before_ the live run ends, so a bad edit keeps the good run alive
(plus an `error` event if the brain subscribed to lifecycle events) — and a
broken script never starts (parks under `--watch`, fails fast without it).
Brains opt in to `reload` / `shutdown` notices with `subscribe_lifecycle`.

See the [endpoint] and [hot reload] pages for the full reference.

To write a pure Rust brain, depend on the `omw-wasm-rust` guest SDK crate
instead of running `wit-bindgen` yourself; see [Rust brains].

Configuration can also be layered from the environment (`OMW__` prefix) or
generated as a JSON schema:

```sh
omw schema --output config.schema.json
```

See the [docs] for the full reference.

### NixOS

The flake ships a NixOS module exposing `services.omw` — a systemd unit that
runs omw from a config file, `envsubst`-ing environment variables into it so
secrets never live in the Nix store:

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    omw.url = "github:haras-unicorn/omw";
  };

  nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
    modules = [
      omw.nixosModules.default
      {
        services.omw = {
          enable = true;
          settingsFile = "/etc/omw.toml";
          environmentFile = "/var/lib/omw/env";
        };
      }
    ];
  };
}
```

See [The NixOS module] in the documentation for the full option set, including
`settings` vs `settingsFile`, `mode`, `user`/`group`, and `stateDir`.

## Binary cache

Builds are cached on the [haras cachix cache]. When the flake is used directly
(for example with `nix run github:haras-unicorn/omw`), the cache is configured
automatically through the flake's `nixConfig`. To use it when the package comes
from an overlay, add the following to your nix configuration:

```nix
{
  nix.settings = {
    substituters = [ "https://haras.cachix.org" ];
    trusted-public-keys = [
      "haras.cachix.org-1:/HIo1JYqOIH1Nwk1EGXhuPPvDW0WekxIbY5CiXUZbYw="
    ];
  };
}
```

[haras cachix cache]: https://app.cachix.org/cache/haras
[docs]: https://haras-unicorn.github.io/omw/
[endpoint]: https://haras-unicorn.github.io/omw/endpoint.html
[hot reload]: https://haras-unicorn.github.io/omw/hot-reload.html
[Rust brains]: https://haras-unicorn.github.io/omw/runtime/wasm.html#rust-brains
[The NixOS module]: https://haras-unicorn.github.io/omw/nixos.html

<!-- ANCHOR_END: body -->

## Documentation

The documentation is available at <https://haras-unicorn.github.io/omw/>.
