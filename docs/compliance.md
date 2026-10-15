# Compliance

`omw` sits on top of three external contracts: the WebAssembly Component Model
and WASI, the Model Context Protocol (MCP), and the informal OpenAI-family
chat-completions HTTP API. This page is the single place that says what revision
of each we build against, how faithful we are to it, and what we plan to do
about the gaps.

## Priorities

When the standards and our shape conflict, four concerns decide. They are
ordered, highest importance first:

1. **Compliance.** If `omw` is not compliant, it stops being functional and the
   project dies. A non-conforming client or server gets rejected, dropped, or
   silently corrupted by the other side of the wire. This outranks everything
   else.
2. **User-friendliness.** Brains and embedders are the users. A clear, ergonomic
   interface that does the obvious thing is worth more than a perfectly faithful
   model of every wire field. This is second only to compliance.
3. **Typed interfaces.** Types catch mistakes early and document shape. We pay
   for them where the shape is stable and known, but we do not let type purity
   force churn.
4. **Future-proofing.** Keeping up with every upstream change is effectively
   impossible given how fast the MCP revision cycle and the OpenAI-family API
   move. We accept periodic maintenance (at least twice a year, see
   [Roadmap and maintenance](#roadmap-and-maintenance)) instead of distorting
   the design for a future that is not here yet. This is deliberately last.

In one line: **be correct, be pleasant, be typed, be durable — in that order.**

## What omw is

`omw` is a headless agent runtime. You declare agents in a config, give each a
provider, some tool servers, and a brain, and `omw` runs them as actors: one
inbox per agent, with chat streams, tool results, timers and inter-agent
messages all arriving as tagged events the brain reads. There is no UI, no
interactive session, and no daemon that decides things on your behalf.

The design goal is **control**: the operator owns the machine and its agents.
Wherever a standard offers omw's operator more control over its own agents, omw
takes it; wherever a standard moves control to a remote service, requires an
interactive surface omw does not have, asks omw to generate content on a tool's
behalf, or is merely decorative, omw declines. A brain decides; a tool serves;
the operator owns the machine. Read every divergence below through that lens.

## How the priorities shape the code

The priority order maps onto a small number of concrete rules.

- **Opaque at the contract, typed at the edges.** Fast-moving provider fields
  (for example token-usage details and message roles) are carried as opaque JSON
  strings across the WIT boundary and the library. This is the future-proofing
  tax we accept, and it keeps the contract stable while the upstream API keeps
  growing. The interpreters that run brains and the Rust guest SDK then _type
  those fields out_, because they know the shape and their users deserve
  ergonomics. Opaqueness is a contract decision, not a brain decision.
- **Typed where the shape is stable or protocol-defined.** Genuinely structural
  things — a tool call, a tool result, a resource — stay typed in the contract.
  When MCP (a real protocol, not an API) defines a field, we can type it with
  confidence.
- **MCP is shielded by the client library.** The MCP wire — transports,
  negotiation, `_meta`, subscription mechanics, result discriminators — is
  largely absorbed by our MCP client dependency, not re-encoded by us. We only
  expose the surface a brain needs and let the library carry the rest.
- **No per-record opaque escape hatches.** A blanket "extra" blob on every
  record is rejected as cursed: it hides structure and invites it to rot. We add
  opaque fields only where a specific upstream field is fast-moving.
- **Control stays local.** Features that hand control to a remote service,
  require an interactive surface `omw` does not have, ask `omw` to generate
  content on a tool's behalf, or are mere decoration are declined even when a
  standard defines them.
- **Batch breaking changes.** Because the contract is the expensive part to
  change, we prefer a deliberate, batched breaking revision over continuous
  churn. Each contract change should be justifiable against the priority order
  above.

## Spec baseline

| Standard                                     | Target               | Primary source                                                                                                                    | Reviewed |
| -------------------------------------------- | -------------------- | --------------------------------------------------------------------------------------------------------------------------------- | -------- |
| WebAssembly Component Model / WASI Preview 2 | preview 2 (`wasip2`) | <https://component-model.bytecodealliance.org/> and <https://github.com/WebAssembly/WASI>                                         | 2026-10  |
| Model Context Protocol                       | `2026-07-28`         | <https://modelcontextprotocol.io/specification/2026-07-28>                                                                        | 2026-10  |
| OpenAI-family chat completions (informal)    | live API             | <https://platform.openai.com/docs/api-reference/chat/create> and the generated types in <https://github.com/openai/openai-python> | 2026-10  |

WASM is the calm surface: the Component Model and WASI Preview 2 move very
slowly, and no drift or gap is currently known on that front. Almost all of the
following is about MCP and the OpenAI-family API, both of which changed
substantially over the last year.

### Missing pieces

Standard features `omw` does not expose, all of them deliberate. Nothing here is
a backlog item unless noted.

#### Deliberate non-goals

- **MCP multi round-trip requests (MRTR): elicitation and sampling.** In
  `2026-07-28` a server may answer a normal request with
  `resultType: "input_required"`, asking the client to gather more information
  and retry — including asking a human (`elicitation/create`), listing the
  client's workspace directories (`roots/list`), or running an LLM on the
  server's behalf (`sampling/createMessage`). Supporting this reverses the
  contract: a tool would go from a dumb executor the operator drives to
  something that drives the operator, and at worst makes `omw` spend its own
  model context generating content for a tool. It would also add a large amount
  of surface (a new event kind, a host function, a pending-call store, a
  per-request policy, and provider access from the tooling layer) for a feature
  the project does not want. `omw` stays conformant by omission — servers **MUST
  NOT** send an `inputRequests` entry the client has not declared support for,
  so advertising no elicitation, sampling or roots means none can arrive. If a
  server ever sends `input_required` carrying only `requestState` (no
  `inputRequests`), the safe behaviour is to surface it as an error rather than
  mishandle it. Sources:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/mrtr>
  and <https://modelcontextprotocol.io/specification/2026-07-28/server/tools>.

- **MCP completion.** `completion/complete` offers autocomplete suggestions for
  prompt and resource-template arguments as a user types. `omw` is headless and
  exposes no prompts; a brain fills a resource template programmatically rather
  than typing into a form, so there is no consumer for suggestions. Non-goal
  unless an interactive surface ever exists. Source:
  <https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/completion>.

- **MCP icons.** Tool and resource icons are decorative UI metadata with no
  meaning to a headless runtime. Non-goal.

- **OpenAI Responses API.** A different, OpenAI-only surface with no
  cross-vendor contract; see [Deliberate divergences](#deliberate-divergences).

#### PR-driven

- **MCP caching hints.** Servers may return `ttlMs` and `cacheScope` freshness
  hints on list and read results. Ignoring them is compliant — the client rule
  is a SHOULD, and an absent TTL means "immediately stale". Honoring them is a
  small client-side cache (freshness, invalidation by change notifications,
  per-page under pagination, authorization scoping, and never caching retries),
  not a field pass-through. Not planned, but a welcome contribution. Source:
  <https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching>.

## Deliberate divergences

- **Chat completions, not Responses.** The Responses API is increasingly the
  recommended surface for the newest hosted OpenAI models, but it has no
  cross-vendor wire contract, and OpenAI-compatible servers overwhelmingly
  implement `/v1/chat/completions` instead. `omw` targets the interoperable
  surface on purpose. Sources:
  <https://platform.openai.com/docs/api-reference/chat/create> and
  <https://platform.openai.com/docs/guides/function-calling>.
- **Control-driven refusals.** The deliberate non-goals in
  [Missing pieces](#missing-pieces) — MRTR with elicitation and sampling,
  completion, and icons — exist because `omw` is headless and keeps the operator
  in control. A tool serves; it does not drive the operator or demand that `omw`
  generate content for it. Declining these is a design position, not an
  oversight, and it is conformant because the standards gate the relevant
  behaviour behind capabilities we do not advertise.
- **Caching is PR-driven.** Ignoring caching hints is compliant, so support
  lands only if someone contributes it.
- **The redundant WIT copy for the output-policy crate is kept on purpose** to
  keep the vendoring story uniform. Do not "clean it up".
- **Single-value simplifications** in the contract are accepted where the
  upstream "many" is rare and the simplification is worth the ergonomics; when
  that trade stops being defensible, the simplification is one of the drifts the
  findings would record.

## How a change propagates

Every contract change touches the same layers, in the same order. This is the
checklist to follow for the next one.

1. **Contract.** Author the change once in the WIT contract. Every crate that
   vendors a copy is refreshed from that single source by the formatter, and the
   linter fails if any copy diverges.
2. **Shared library.** Mirror the change in the runtime's provider, tooling,
   host and event types, and in the encoders and decoders that read and write
   the wire.
3. **Interpreter bindings.** Re-expose it on the `omw` global in each
   interpreter (rhai, js, python), typing out any opaque JSON the contract
   carries.
4. **Rust guest SDK.** Re-expose it as typed builders and accessors.
5. **Declarations and schemas.** Regenerate the guest declaration companions and
   the machine-readable schemas by running the formatter, which also refreshes
   the generated TypeScript and Python models.
6. **Mocks.** Teach the provider, tooling and endpoint doubles to script the new
   shape, so it is testable and dumpable.
7. **Examples.** Update the committed example configs and brains that exercise
   the shape.
8. **Docs.** Update the provider, endpoint, host, tooling and runtime pages that
   describe the old shape.

## Roadmap and maintenance

- **Cadence.** Review at least twice a year, aligned with the NixOS release
  cycle (the same rhythm already used to re-validate the rest of the stack), and
  additionally whenever a new MCP revision or a breaking OpenAI-family change
  lands.
- **Open work.** None. Every drift and planned feature this page tracked has
  landed; the roadmap is clear until the next maintenance cycle turns up new
  gaps.
- **Not scheduled.** MRTR, completion and icons are non-goals, not backlog.
  Caching is [PR-driven](#pr-driven) and lands only if someone contributes it.
- **Expected cost.** WASM: near zero. MCP: the client library absorbs most wire
  churn, but every new capability we choose to expose is a contract change;
  budget roughly one breaking contract revision a year. OpenAI-family: low as
  long as fast-moving fields stay opaque; only shape changes force work.

## Maintaining this page

Every maintenance cycle, and whenever `wasm`, `mcp` or `openai` related
behaviour changes, do the following:

1. **Re-check the baselines.** Bump the revision and reviewed date in
   [Spec baseline](#spec-baseline) against the sources listed there. Note that
   the OpenAI-family API has no versioned spec, so cite the reference page and
   the generated SDK types together.
2. **Re-derive the findings.** Walk the current MCP spec sections for tools,
   resources, pagination, subscriptions and multi round-trip requests, and the
   current chat-completions reference plus generated types. Add anything new to
   the drifts or missing-pieces lists; remove anything resolved.
3. **Keep the distinction honest.** A drift is a wrong shape of a thing we
   already do; a missing piece is a thing we do not do. Do not relabel one as
   the other to make the list look better.
4. **Re-evaluate the shapes.** If a fast-moving field is now stable enough to
   type, or a typed field is now churning, move it across the opaque/typed line
   described in
   [How the priorities shape the code](#how-the-priorities-shape-the-code).
5. **Update the roadmap.** Adjust the open work and cadence if upstream moved.
6. **Record the cycle.** Add a short dated line under this heading noting what
   changed and what was deferred.
7. **Revisit the non-goals deliberately.** A non-goal is a design position, not
   a forgotten item. Only promote one to the roadmap if the project's control
   stance changes, and say why.
