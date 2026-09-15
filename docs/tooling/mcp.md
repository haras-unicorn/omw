# The MCP tooling

The `mcp` tooling (`tooling::mcp`, kind `mcp`) is a client for the Model Context
Protocol, built on the official `rmcp` Rust SDK. It owns the wire protocol and
the `initialize` lifecycle itself, mapping `rmcp`'s typed results onto omw's
`tool` / `resource-info` types and joining text tool results.

## Transports

| transport | config keys              | speaks over                    |
| --------- | ------------------------ | ------------------------------ |
| `stdio`   | `command`, `args`, `env` | a server subprocess, JSON-RPC  |
| `http`    | `url`, `auth_token`      | a streamable-HTTP MCP endpoint |

## Configuration

| key          | type   | transport | meaning                  |
| ------------ | ------ | --------- | ------------------------ |
| `transport`  | string | both      | `stdio` or `http`        |
| `command`    | string | stdio     | the server executable    |
| `args`       | list   | stdio     | extra arguments          |
| `env`        | attrs  | stdio     | extra server environment |
| `url`        | string | http      | the endpoint URL         |
| `auth_token` | string | http      | sent as a `Bearer` token |

The `auth_token` and `env` values are never logged: they are `Secret`s (locked
with `mlock`, zeroized on drop) that redact on `Debug` and serialize, and `omw`
fails at startup if the lock cannot be taken.

```toml
[tooling.mcp]
kind = "mcp"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-everything"]
```

## Connecting

The build only parses and validates config; the server is dialed lazily on the
first tool or resource use. The first use retries with exponential backoff
(doubling from `tooling_connect_backoff_start_ms` up to
`tooling_connect_backoff_cap_secs`, see [tunables](../tunables.md)). A failed
first use surfaces as `Err` for blocking callers and as the `error` event
variant for event-driven callers; dropping the call (pump cancel, reload, or
shutdown) cancels the wait. Tool results keep only their text content blocks,
joined with newlines; binary content is dropped.

## Resources

`mcp` surfaces every resource the peer advertises via `list-resources`. The two
subscription modes map onto `rmcp` subscription filters:

- `subscribe-resource-list()` subscribes to list-changed notifications and
  delivers `resource-list-updated` events carrying the freshly fetched list;
- `subscribe-resource(uri)` subscribes to a single resource and, at each update,
  the host reads the resource back (`resources/read`) and delivers a
  `resource-updated` event carrying the content. Textual resource content is
  passed through as-is; binary content arrives base64-encoded, so match on the
  content's `mime-type` to decide whether to decode it.

Both return `BoxStream`s; the host's resource pump drains them and pushes tagged
events into the agent inbox, and dropping the stream cancels the subscription.
