# Deployment

`omw` runs anywhere its single static binary runs. The release tarballs
(`omw-<arch>.tar.gz`, plus `-rhai` / `-js` variants) contain a statically linked
musl binary — no runtime dependencies besides CA certificates for TLS — so the
same artifact drops onto NixOS (via the [NixOS module](./nixos/module.md)), onto
any systemd host (via the [plain unit](./systemd.md)), or into a minimal
container (see [Docker](./docker.md)).

## Configuration everywhere

Every deployment reads the same TOML file (`--config`, default `omw.toml`).
`assets/omw.example.toml` in the repo is the shared starting point; the
[Docker page](./docker.md#compose) walks through it. Secrets layer over the file
from `OMW__`-prefixed environment variables (`__` separator), e.g.
`OMW__PROVIDERS__OPENAI__API_KEY` overrides `providers.openai.api_key` — keep
keys out of the file and supply them from the environment.

## Workspace convention

Agents that touch the filesystem (filesystem MCP tooling, brain scripts on disk)
expect a persistent workspace directory:

| deployment | workspace             | how it is provided                          |
| ---------- | --------------------- | ------------------------------------------- |
| NixOS      | `/var/lib/<stateDir>` | `services.omw.stateDir` (`StateDirectory=`) |
| systemd    | `/var/lib/omw`        | `StateDirectory=omw` in the unit            |
| Docker     | `/var/lib/omw`        | named volume or bind mount                  |

Point brain `script` paths and filesystem tooling roots at the workspace so all
three deployments share one config shape.

## Static builds

The flake cross-compiles `x86_64-unknown-linux-musl` /
`aarch64-unknown-linux-musl` and checks the result with `file` + `ldd`
(`checks.static*` in `src/nix/dev.nix`). That is what makes the Alpine image
(`assets/Dockerfile`) a plain `COPY` with no toolchain.
