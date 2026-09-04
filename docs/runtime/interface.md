# The runtime interface

A _runtime_ is how an agent's "brain" is loaded and driven for one iteration.
Where providers and tooling are I/O, a runtime is the _program_ the agent runs:
it is handed the agent's brain (a wasm component or a rhai script) and asked to
execute it against the [`AgentContext`](#the-agent-context).

Named runtimes live in the global `[runtime.<name>]` config map. Each agent
wiring pins itself to one by name.

## The abstraction

A runtime exposes, through the WIT `runtime` interface (exported by the guest
component and called by the host):

- `kind()` — which brain implementation this is (`wasm` or `rhai`).
- `run(script)` — run one iteration. Returns the terminal message if the brain
  chose to exit, otherwise nothing.

The host's `Runtime::run(&AgentContext)` drives this synchronously off the tokio
worker (on a `spawn_blocking` thread because the wasm engine and the rhai
interpreter component built on it are synchronous). The script argument is the
agent's brain. For a wasm brain the program is baked into the component and the
script is unused, while for Rhai it is the Rhai source text.

## How a run happens

`build(kind)` dispatches to `wasm` or `rhai`. Each runtime loads its engine, and
pushes a blocking task that:

1. builds a `Store` whose data is the host `Host` (the agent context, a resource
   table, and a WASI context),
2. wires the imports — the WASI wasip2 imports plus the `provider`, `tooling`,
   and `host` interfaces — into a `Linker`,
3. instantiates the component,
4. calls the exported `runtime.run(script)` and
5. surfaces the terminal message as `Exited(message)` or `Completed` if the
   brain ran to a finish.

## The agent context

Every runtime call receives an `AgentContext`: the agent `name`, the brain
`script` path, the named `providers` and `tooling` registries, the shared
`MessageBus`, the agent's own `StreamRegistry` (chat streams), timer
`CancelRegistry`, resource-subscription `CancelRegistry` and the dedicated tokio
`rt` used to bridge synchronous host calls to the async provider/tooling.

Provider and tooling registries, and the message bus, are shared across all
agents in one process — the `StreamRegistry`, timer/resource registries, and
bridge runtime are per-agent.
