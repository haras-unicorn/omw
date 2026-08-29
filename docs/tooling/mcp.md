# The MCP tooling

The `mcp` tooling (`tooling::mcp`, kind `mcp`) is a client for the Model Context
Protocol, built on the official `rmcp` Rust SDK. It owns the wire protocol and
the `initialize` lifecycle itself, mapping `rmcp`'s typed results onto omw's
`tool` / `resource-info` types and joining text tool results.

## Transports

| transport | default | config keys              | speaks over                    |
| --------- | ------- | ------------------------ | ------------------------------ |
| `stdio`   | yes     | `command`, `args`, `env` | a server subprocess, JSON-RPC  |
| `http`    | no      | `url`, `auth_token`      | a streamable-HTTP MCP endpoint |

## Configuration

| key          | type   | transport | meaning                           |
| ------------ | ------ | --------- | --------------------------------- |
| `transport`  | string | both      | `stdio` (default) or `http`       |
| `command`    | string | stdio     | the server executable             |
| `args`       | list   | stdio     | extra arguments for the command   |
| `env`        | attrs  | stdio     | extra environment for the command |
| `url`        | string | http      | the endpoint URL                  |
| `auth_token` | string | http      | sent as a `Bearer` token          |

The `auth_token` is never logged: it is redacted in the tooling's debug output,
along with the configured environment keys.

```toml
[tooling.mcp]
kind = "mcp"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-everything"]
```

## Connecting

The tooling builds the transport (spawning the stdio subprocess, or opening the
HTTP endpoint) and serves it with `rmcp` in client (`Initialize`) role. Tool
results keep only their text content blocks, joined with newlines; binary
content is dropped.

## Resources

`mcp` surfaces every resource the peer advertises via `list-resources`. The two
subscription modes map onto `rmcp` subscription filters:

- `subscribe-resource-list()` subscribes to list-changed notifications and
  delivers `resource-changed` events;
- `subscribe-resource(uri)` subscribes to a single resource and delivers
  `resource-updated` events.

Both return `BoxStream`s; the host's resource pump drains them and pushes tagged
events into the agent inbox, and dropping the stream cancels the subscription.
