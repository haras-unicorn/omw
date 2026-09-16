# Systemd (non-NixOS)

For hosts without the [NixOS module](./nixos/module.md), the unit below is a
drop-in that mirrors what the module generates with
`services.omw.hardening = true` (in the repo: `assets/omw.service`):

```ini
{{#include ../../assets/omw.service}}
```

Install it (the `assets/…` paths are repo-relative):

```sh
sudo useradd -r -d /var/lib/omw omw
sudo install -m 644 assets/omw.service /etc/systemd/system/omw.service
sudo mkdir -p /etc/omw
sudo install -m 600 assets/omw.example.toml /etc/omw/omw.toml
sudo install -m 600 assets/omw.example.env /etc/omw/env
sudo systemctl daemon-reload
sudo systemctl enable --now omw
```

Secrets live in `/etc/omw/env` as `OMW__`-prefixed variables (see the
[deployment overview](./deployment.md)); they layer over `/etc/omw/omw.toml` at
runtime. Put brains and the filesystem tooling workspace under `/var/lib/omw`
(the unit's `StateDirectory` + `WorkingDirectory`).

## Hardening

The unit carries the same sandbox as the NixOS module (see
[the NixOS module](./nixos/module.md#hardening) for the rationale):

- `NoNewPrivileges`, `RestrictSUIDSGID`, `RestrictRealtime`, `LockPersonality`,
  empty capability sets plus `LimitMEMLOCK=infinity` (for allowing secret
  protection against swapping) — safe, and compatible with `bwrap`-wrapped MCP
  servers.
- `ProtectSystem=strict` + `ProtectHome` + `ProtectProc=invisible` +
  `PrivateIPC` — the filesystem is read-only outside API mounts and the state
  directory; allow-list extras with `BindReadOnlyPaths=` (brains or config
  outside `/var/lib/omw`) and `ReadWritePaths=` (filesystem MCP workspaces
  elsewhere).
- `RestrictAddressFamilies=AF_UNIX AF_NETLINK AF_INET AF_INET6` — provider
  egress plus local stdio MCP sockets.
- `MemoryDenyWriteExecute` is deliberately absent: it would break the wasmtime
  JIT and nodejs MCP servers. `PrivateDevices=false` keeps `/dev` usable for MCP
  servers and `bwrap` wrappers.
