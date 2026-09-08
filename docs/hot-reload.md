# Hot reload

`omw run --watch` / `omw loop --watch` restarts an agent when its brain script
changes, without losing bus state. Only the run restarts; everything addressable
survives.

This matches the actor model in [actor](./actor.md): the inbox is the agent, the
brain is just its current reader. Lifecycle notifications (`reload`, `shutdown`,
reload-failure `error`) follow the same model: explicit subscribe, UUID
correlation, no magic inbox traffic for agents that did not opt in.

## Using `--watch`

Pass `--watch` to either mode:

```sh
omw run --config omw.toml --watch
omw loop --config omw.toml --watch
```

The watcher tracks each agent's `script` path. It watches parent directories
non-recursively (editors that save via `write temp + rename` still trigger) and
debounces 200ms, so one save restarts the agent once.

## What survives and what dies

Preserved across reload:

- Inboxes (queued events are never drained or dropped).
- Agent subscriptions and endpoint subscriptions.
- Providers, tooling, and endpoint sessions.

Discarded on reload:

- Wasm memory/stack (a fresh `Store` per run).
- Open pump tasks: chat streams, timers, resource subscriptions, tool calls.
  They are cancelled so stale events cannot leak into the next run.

Rhai brains re-read the script file and wasm brains reload the component on
every iteration, so the next run picks up the edit.

## Validation and startup

A bad edit never kills a good run. The supervisor validates the new script
_before_ aborting the live one, while the old run keeps executing. Only a valid
script ends the current run. An invalid script keeps the old run alive, logs a
host-side warning, and delivers an `error` event on the lifecycle handle (if the
brain subscribed) so it can react however it wants.

Startup is gated the same way: a broken script never produces a first iteration.
Without `--watch` this fails fast. With `--watch` the agent parks until an edit
fixing the script validates, then starts normally.

## How a restart happens

Three tiers, mirroring SIGTERM/SIGKILL:

1. Cooperative: `recv` aborts on the next 200ms poll without draining the inbox,
   blocking calls abort through a helper, and a `reload`/`shutdown` event covers
   `try-recv` pollers. Fast (≤200ms for `recv`).
2. Grace timeout (5s): the supervisor stops waiting and reports the abort.
   Bounds restart latency.
3. Preemptive (epoch trap, 100ms after grace): one epoch increment traps wasm
   loops that never yield to the host.

Shutdown reuses the same three tiers, but is terminal: `run` collects it as an
error, `loop` breaks instead of restarting.

Every host call parks somewhere different, so each needs its own interrupt:

| Brain is stuck in                                                       | Where it parks                       | Interrupt                                                                                                          | Upstream sees                             |
| ----------------------------------------------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------- |
| `host.recv`                                                             | inbox slices (200ms)                 | flag poll, `Err("agent reloaded"/"agent shutting down")`, inbox kept; plus a `reload`/`shutdown` event for pollers | nothing (pure local wait)                 |
| `host.try-recv` / `send` / etc                                          | returns immediately                  | nothing needed; pollers observe the queued `reload`/`shutdown` event                                               | nothing                                   |
| `provider.chat-stream` pump                                             | biased `select!` on cancel           | open streams are cancelled, pump breaks                                                                            | TCP close, OpenAI sees client disconnect  |
| `host.wait-*` / timers                                                  | `select!` on cancel                  | open timers are cancelled                                                                                          | nothing                                   |
| resource subscription pumps                                             | stream + cancel select               | open subscriptions are cancelled                                                                                   | depends on transport                      |
| `tooling.call-tool` pump                                                | `select!` on the future              | abort drops the in-flight call                                                                                     | MCP call dropped client-side              |
| blocking `provider.chat`                                                | 200ms abort slices                   | abort handle, `Err("agent reloaded")`                                                                              | normal completed request, result dropped  |
| blocking `call-tool-blocking`, `list-*`, `read-resource`, `subscribe-*` | same helper                          | same as above                                                                                                      | server runs to completion, result dropped |
| blocking `sleep-duration/timestamp/cron`                                | same helper                          | same as above = true cancel                                                                                        | nothing                                   |
| pure wasm `while true {}`                                               | executing wasm, never yields to host | epoch trap after grace + 100ms                                                                                     | nothing                                   |

Tuning constants (not configurable):

| Knob                  | Value                 |
| --------------------- | --------------------- |
| Watch debounce        | 200ms                 |
| Recv poll slice       | 200ms                 |
| Recv timeout          | 60s                   |
| Inbox bound           | 1024 events           |
| Reload/shutdown grace | 5s                    |
| Blocking-call poll    | 200ms                 |
| Epoch budget          | 100ms after grace     |
| Loop backoff          | 100ms doubling to 30s |

## Writing a reload-safe brain

Subscribe to lifecycle events explicitly and correlate by UUID, like every other
subscription in the actor model (see [host](./host.md)):

```rhai
let lc = omw::host::lifecycle_subscribe();
let sub = omw::host::subscribe("other");
loop {
  let e = omw::host::recv();
  if e.id == lc {
    if e.kind == "reload" { break; }
    if e.kind == "shutdown" { break; }
    if e.kind == "error" {
      omw::host::log("warn", "reload failed: " + e.payload);
      continue;
    }
  }
  // ... handle e ...
}
omw::host::lifecycle_unsubscribe(lc);
```

Rules:

- Lifecycle is opt-in: `lifecycle_subscribe` returns a UUID; `reload`,
  `shutdown`, and reload-failure `error` events arrive tagged with it. A second
  subscribe errors (one lifecycle subscription per run). Unsubscribed brains
  still get aborted on a _valid_ reload (`recv` errors), but get no events and
  no invalid-edit notice.
- `reload-failed` is the plain `error` variant, not a new event: distinguish by
  `e.id == lifecycle_uuid && e.kind == "error"` and read the validation message
  from the payload. The host always logs regardless, so the event is the brain's
  chance to notify itself, not the only signal.
- The live run never exits on an invalid edit. If you see `error` on the
  lifecycle handle, keep running.
- Exit on `reload` (`break`, do cleanup); exit terminally on `shutdown`. `recv`
  may also abort with `"agent reloaded"` / `"agent shutting down"` when blocked
  between polls.
- Prefer evented calls (`chat-stream`, `call-tool`, `wait-*`) over blocking ones
  (`chat`, `call-tool-blocking`, `sleep-*`): both cancel promptly, but evented
  handles keep delivering while blocking ones abort the handle.
- Re-subscribe at the top of the script. Handles are one-shot UUIDs that die
  with the run (lifecycle included). There is no cross-reload memory yet:
  rebuild any cached handles from scratch on each run.
- Never `while true {}` without a host yield; an unyielding loop can only die by
  epoch trap.
- Pollers should use `try-recv` + small `wait-duration`, so the `reload` /
  `error` events are observed promptly.
- A broken script never starts, so the first thing a fresh brain can assume is
  that it compiled.

## Troubleshooting

- "Reload takes 5s": the brain is stuck in a blocking call or unyielding loop
  and the grace expired. Prefer evented calls, or yield to the host regularly.
- "Agent restarts twice": two saves in quick succession, or a stale `reload`
  event surviving into the next run. The drain at iteration start drops queued
  system events; check the watcher debounce vs your editor's save burst.
- "Old stream still delivers": a pump from the previous run outlived the reload.
  Reload cancels all open pumps; if you held the UUID, re-check `is-open` after
  `reload` instead of assuming it is alive.
- "CPU spins after save": a `while true {}` without a host yield. Only the epoch
  trap can kill it, after the grace. Add a `recv`, `wait-duration`, or
  `sleep-duration` to the loop.
- "Edit did nothing": the script was invalid. Check host logs for the warning
  and, if subscribed, the lifecycle `error` payload. The live run kept going;
  fix the script and save again.
- "Agent won't start": the startup gate rejected the script. Same signals as
  above. With `--watch` the agent parks until a fixing edit validates; without
  it the process fails fast.
