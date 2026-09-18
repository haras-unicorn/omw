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

The unit runs `omw <mode> --config <file>` directly (`ExecStart`, no shell):

```sh
omw loop --config /etc/omw.toml
```

Two things follow from this:

- **`settings` and `settingsFile` are mutually exclusive.** `settings` is an
  attribute set rendered to TOML at build time; `settingsFile` is a path to a
  TOML file on the system. Choose whichever fits.
- **Secrets are layered from the environment.** `OMW__`-prefixed variables (`__`
  separator, e.g. `OMW__PROVIDERS__OPENAI__API_KEY`) override file values at
  runtime, so API keys never have to live in the Nix store. Set them with the
  `environment` option (systemd `Environment=`) or an `environmentFile`.

`mode` selects `run` (every agent once) or `loop` (keep agents running,
restarting on failure — the default, suited to a service).

`extraArgs` passes extra CLI flags after the mode; use `[ "--watch" ]` to
hot-reload agent scripts (a changed brain file restarts its agent while inboxes
and subscriptions survive).

`variant` selects which package variant runs: `default` (the
crates.io-equivalent build, no script runtime), `rhai` (the `omw-rhai` package,
which compiles the bundled rhai interpreter in) or `js` (the `omw-js` package,
which compiles the bundled js interpreter in). Overridable entirely with
`package`.

## Users and state

By default the service runs under a systemd _dynamic user_ (no `user` /
`group`). Set `user` and/or `group` to pin a specific identity. `stateDir`
declares a `StateDirectory` (created under `/var/lib`, also used as
`WorkingDirectory`), which is where a filesystem MCP tooling's workspace would
live and where the service can persist state.

Example with an MCP filesystem tooling rooted at the state directory:

```nix
{{#include ../../../assets/omw.nix}}
```

## Hardening

`services.omw.hardening` (default `true`) applies a systemd sandbox modeled on a
service that spawns nodejs MCP servers and `bwrap` wrappers:

- Identity/capabilities: `NoNewPrivileges=true` (confirmed compatible with
  `bwrap`-wrapped MCP servers), `RestrictSUIDSGID=true`,
  `RestrictRealtime=true`, `LockPersonality=true`,
  `SystemCallArchitectures=native`, empty `CapabilityBoundingSet` /
  `AmbientCapabilities`. `LimitMEMLOCK=infinity` raises the unit's ceiling, but
  inside containers the outer `RLIMIT_MEMLOCK` still wins — set
  `OMW__TUNABLES__ALLOW_UNLOCKED_SECRETS=true` in the environment there instead
  (see [tunables](../../tunables.md)), since secrets cannot `mlock()` under the
  container's limit. Deliberately omitted: `MemoryDenyWriteExecute=true` would
  break the wasmtime JIT and nodejs MCP servers; `SystemCallFilter=` is left
  open for the same reason (wasmtime/node need broad syscalls).
- Filesystem/IPC: `ProtectSystem=strict`, `ProtectHome=true`,
  `ProtectClock=true`, `ProtectKernelModules=true`, `ProtectControlGroups=true`,
  `ProtectProc=invisible`, `PrivateIPC=true`, `RemoveIPC=true`, `UMask=0077`.
  `PrivateDevices=false` keeps `/dev` usable for MCP servers and `bwrap`.
- Network: `RestrictAddressFamilies=AF_UNIX AF_NETLINK AF_INET AF_INET6` —
  provider egress plus local stdio sockets and NSS lookups.

Under `ProtectSystem=strict` the unit only sees API mounts plus its state
directory. Use `services.omw.readOnlyPaths` (`BindReadOnlyPaths=`) when brains
or the config file live elsewhere (e.g. the `/etc/brain.rhai` used in tests),
and `services.omw.readWritePaths` (`ReadWritePaths=`) for filesystem MCP
workspaces outside the state directory. Set `services.omw.hardening = false` to
drop the sandbox entirely, or override individual keys with
`services.omw.serviceConfig`, which merges last.
