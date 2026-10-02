# Docker

All paths below are repo-relative (`assets/Dockerfile`, `assets/compose.yaml`,
`assets/omw.example.toml`, `assets/omw.example.env`). The Dockerfile builds a
minimal Alpine image straight from the GitHub release tarballs (the binaries are
static musl, so there is nothing to compile):

```dockerfile
{{#include ../../assets/Dockerfile}}
```

```sh
docker build \
  --build-arg OMW_VERSION=0.1.0 \
  --build-arg OMW_VARIANT=rhai \
  -t omw .
docker run -d --name omw --restart unless-stopped \
  --env-file omw.env \
  -e OMW__TUNABLES__ALLOW_UNLOCKED_SECRETS=true \
  -v ./omw.toml:/etc/omw/omw.toml:ro \
  -v omw-workspace:/var/lib/omw \
  -p 37532:37532 \
  omw
```

Secrets cannot `mlock()` inside containers (the container's own `RLIMIT_MEMLOCK`
is enforced regardless of the unit's `LimitMEMLOCK=`), so the shipped example
docker-compose.yaml sets `OMW__TUNABLES__ALLOW_UNLOCKED_SECRETS=true` — see
[tunables](../tunables.md). Start from the [example config and env](#compose).
Secrets travel as `OMW__`-prefixed variables (`--env-file` or `-e`), never baked
into the image. `/var/lib/omw` is the `stateDir` equivalent: mount a named
volume or bind mount there for brains and the filesystem MCP workspace. Publish
`37532` only when `[endpoint]` is configured (`listen = "0.0.0.0:37532"` inside
containers).

## Logging

A container has no journald socket and Docker captures output to a pipe, so
omw's `auto` log format selects `pipe`: one JSON object per line on stderr.
`docker logs omw` (or `docker compose logs -f omw`) renders them, and piping
through `jq` makes them readable. `RUST_LOG` selects the level — the default is
`info` — set it with `-e RUST_LOG=debug` or in the env file. The interactive
`tty` view never engages here, since neither stream is a terminal. See
[Output](../output.md) for the full policy and the `--log-format` flag.

## Compose

The compose example wires it together, including an optional HTTP MCP server on
the same network. The example config it mounts is:

```yaml
{{#include ../../assets/compose.yaml}}
```

Compose accepts a git URL as `build.context`, so you can build without cloning
the repo; point `dockerfile` at the in-repo path and keep your `omw.toml` /
`omw.env` beside your own compose file:

```yaml
services:
  omw:
    build:
      context: https://github.com/haras-unicorn/omw.git
      dockerfile: assets/Dockerfile
```

## MCP servers

- **HTTP transport (easy):** run the server as another compose service and point
  the tooling at it (`transport = "http"`, `url = "http://mcp:8000/…"`). No
  image changes needed.
- **stdio transport (node, uvx, bwrap wrappers, …):** the server command must
  exist _inside_ the omw image — a sidecar cannot help, since stdio means a
  subprocess. Extend the image:

```dockerfile
FROM omw AS with-mcp
RUN apk add --no-cache nodejs
RUN npm install -g @modelcontextprotocol/server-filesystem
```

Then reference `command = "server-filesystem"` (or `npx …`) in the tooling
config. The same applies to `bwrap`-wrapped commands: install `bubblewrap` in
the image; `NoNewPrivileges`-style restrictions do not apply inside containers,
and user namespaces work under the default Docker seccomp profile.
