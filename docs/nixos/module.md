# The NixOS module

The flake ships a NixOS module exposing a single `services.omw` option set that
runs omw as a systemd service. It is the recommended way to run an omw agent (or
several — a single service can run `omw run` / `omw loop`, which already
supports multiple agents from one config) on NixOS.

The full option reference is generated from the module by the flake's
`omw-options` package; this page explains the design and how to use it.

## Enabling the module

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    omw.url = "github:haras-unicorn/omw";
  };

  outputs =
    { nixpkgs, omw, ... }:
    {
      nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
        modules = [
          omw.nixosModules.default
          {
            services.omw = {
              enable = true;
              mode = "loop";
              settingsFile = "/etc/omw.toml";
              environmentFile = "/var/lib/omw/env";
            };
          }
        ];
      };
    };
}
```

## How the service runs

The generated unit reads the configuration, substitutes environment variables
into it, and pipes the result into omw through `--config /dev/stdin`:

```sh
envsubst < /path/to/config | omw --config /dev/stdin loop
```

Two things follow from this:

- **`settings` and `settingsFile` are mutually exclusive.** `settings` is an
  attribute set rendered to TOML at build time; `settingsFile` is a path to a
  TOML file on the system. Choose whichever fits.
- **Secrets are injected at runtime.** Values in the config that look like
  `$VAR` are replaced by `envsubst` from the service environment, so API keys
  never have to live in the Nix store. Set them with the `environment` option
  (exported at the top of the script) or an `environmentFile`.

`mode` selects `run` (every agent once) or `loop` (keep agents running,
restarting on failure — the default, suited to a service).

## Users and state

By default the service runs under a systemd _dynamic user_ (no `user` /
`group`). Set `user` and/or `group` to pin a specific identity. `stateDir`
declares a `StateDirectory` (created under `/var/lib`), which is where a
filesystem MCP tooling's workspace would live and where the service can persist
state.

Example with an MCP filesystem tooling rooted at the state directory:

```nix
{{#include ../../assets/omw.nix}}
```
