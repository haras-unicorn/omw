# OMW

<!-- ANCHOR: body -->

OMW = OpenAI + MCP + WASM.

`omw` is an agent runtime. You declare agents in a TOML configuration, each
wiring a _provider_ (an OpenAI-family chat service), _tooling_ (MCP tool
servers), and a _brain_ — either a compiled WASM component or a [rhai] script.
`omw` then drives each agent through an actor model: every agent owns a single
inbox, and chat streams, tool results, timers, and messages from other agents
all arrive there as tagged events the brain consumes.

[rhai]: https://rhai.rs

## Who it is for

`omw` is for people who want a small, local agent runtime that is genuine about
its inputs and outputs: the brain is real WASM, tooling speaks MCP, and the
configuration is plain TOML. It is not a framework — there is no DSL to learn
and no orchestration layer. You bring a provider key, a couple of MCP servers,
and a brain, and `omw` runs it for one iteration (`run`) or keeps it going
(`loop`).

## How it works

- **Providers** are OpenAI-family chat services. `provider.chat` opens a
  _streaming_ response whose deltas arrive as events in the agent's inbox.
- **Tooling** is MCP tool servers. They expose callable _tools_ and readable
  _resources_; resource subscriptions deliver change events.
- **Brains** are runtimes. The `wasm` runtime loads an agent as a compiled
  component; the `rhai` runtime evaluates a script on a bundled interpreter.
  Both see the same `omw` host interface.
- **Agents** are actors. They subscribe to each other explicitly, so a message
  only ever reaches an agent that chose to listen.

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
[GitHub release] as tarballs containing the `omw` binary.

```sh
curl -L -o omw.tar.gz \
  https://github.com/haras-unicorn/omw/releases/latest/download/omw-x86_64-linux.tar.gz
tar -xzf omw.tar.gz
./omw-x86_64-linux
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

### home-manager

There is no home-manager module — omw is typically a system-level agent service.
home-manager users simply add the package:

```nix
{ pkgs, ... }:
{
  home.packages = [ pkgs.omw ];
}
```

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
[The NixOS module]: https://haras-unicorn.github.io/omw/nixos.html

<!-- ANCHOR_END: body -->

## Documentation

The documentation is available at <https://haras-unicorn.github.io/omw/>.
