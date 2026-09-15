# Tunables

All timing, buffering, and backoff knobs live in one global `[tunables]`
section. Every field is optional; omitted fields fall back to the defaults
below.

```toml
[tunables]
inbox_bound = 1024
recv_slice_ms = 200
recv_timeout_secs = 60
reload_poll_ms = 200
interrupt_budget_ms = 100
reload_grace_secs = 5
loop_backoff_start_ms = 100
loop_backoff_cap_secs = 30
tooling_connect_backoff_start_ms = 100
tooling_connect_backoff_cap_secs = 30
watch_debounce_ms = 200
session_buffer = 8192
cancel_pumps_on_reload = true
```

They can also be layered from the environment (`OMW__` prefix, `__` separator),
e.g. `OMW__TUNABLES__RECV_TIMEOUT_SECS=30`.

## Reference

| Knob                               | Units  | Default | What it does                                       |
| ---------------------------------- | ------ | ------- | -------------------------------------------------- |
| `inbox_bound`                      | events | 1024    | Per-agent inbox capacity; sends fail past it.      |
| `recv_slice_ms`                    | ms     | 200     | How often a blocking `recv` checks for reload.     |
| `recv_timeout_secs`                | secs   | 60      | How long a blocking `recv` waits before timeout.   |
| `reload_poll_ms`                   | ms     | 200     | How often blocking calls check for reload.         |
| `interrupt_budget_ms`              | ms     | 100     | Unwind budget after the interrupt traps a loop.    |
| `reload_grace_secs`                | secs   | 5       | How long the supervisor waits for a coop exit.     |
| `loop_backoff_start_ms`            | ms     | 100     | Backoff start for `loop` restarts on failure.      |
| `loop_backoff_cap_secs`            | secs   | 30      | Backoff cap for `loop` restarts (doubling).        |
| `tooling_connect_backoff_start_ms` | ms     | 100     | Backoff start for MCP tooling reconnects.          |
| `tooling_connect_backoff_cap_secs` | secs   | 30      | Backoff cap for MCP tooling reconnects (doubling). |
| `watch_debounce_ms`                | ms     | 200     | How long the watcher coalesces one save's events.  |
| `session_buffer`                   | deltas | 8192    | Per-session endpoint reply buffer before drops.    |
| `cancel_pumps_on_reload`           | bool   | true    | Cancel open pumps on reload; `false` keeps them.   |

See [hot reload](./hot-reload.md) for how the reload knobs interact.
