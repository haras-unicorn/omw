# The tooling interface

A _tooling_ is an abstraction over an MCP-style tool server: it exposes callable
_tools_ and readable _resources_. Named toolings live in the global
`[tooling.<name>]` config map and are looked up at runtime by name.

## The abstraction

A tooling exposes, through the WIT `tooling` interface:

- `kind()` — which implementation this is (e.g. `mcp`).
- `name()` — the configured name of the instance.
- `list-tools()` — every tool visible on the instance.
- `call-tool(name, arguments)` — invoke a single tool, returning text.
- `list-resources()` — every URI-addressed resource the tooling exposes.
- `subscribe-resource-list()` — subscribe to the resource _list_ changing;
  returns a UUID handle tagged on each `resource-list-updated` event, which
  carries the freshly fetched resource list.
- `unsubscribe-resource-list(uuid)` — cancel a resource-list subscription by
  that UUID.
- `subscribe-resource(uri)` — subscribe to one resource's updates; returns a
  UUID handle tagged on each `resource-updated` event, which carries the freshly
  read resource content (`{ uri, mime_type?, content }`).
- `unsubscribe-resource(uuid)` — cancel a single-resource subscription by that
  UUID.

The handle is obtained once with `tooling.get(name)`, which returns a `tooling`
resource; all further calls go through that handle.

## Tools

A `tool` has a `name`, an optional `description`, and an `input-schema` — a JSON
Schema describing the arguments the model must supply. The guest hands the
signature to a provider so the model can emit a `tool-call` for it, then invokes
it with `call-tool`.

## Resources

A `resource-info` is a single URI-addressed, readable value: it carries a `uri`,
a programmatic `name`, an optional `description`, and an optional `mime-type`.
Resources are how a tooling surfaces read-only data (a file, a database row, a
metric) without it being an instruction-following tool.

A `resource-content` is the content of one such resource read at the moment its
update fired. It carries the `uri`, an optional `mime-type`, and the `content`
itself: the `content` holds actual text for textual formats and base64-encoded
bytes for anything else. Match on `mime-type` to tell the two apart — a `text/*`
(or absent) type means plain text, anything else means base64.

### Subscriptions

Resource _list_ subscriptions and single-resource subscriptions are both
long-lived streams. As with chat streams, the host spawns a resource pump on the
bridge runtime that pushes `resource-list-updated` / `resource-updated` events
into the agent inbox, tagged with the UUID the subscribe call returned. A
`resource-list-updated` event carries the resource list fetched at change time
(an entire `list<resource-info>`), so the guest never has to call back to
`list-resources`; a `resource-updated` event carries the resource content read
at update time (a `resource-content`), so the guest never has to read the
resource back itself. Each subscription can be cancelled early with
`unsubscribe-resource-list` / `unsubscribe-resource` by that UUID. Delivery
contract: **dropping the returned stream cancels the subscription**.

## The streaming contract

`subscribe-resource-list` and `subscribe-resource` mirror the provider's chat
contract: they return a UUID immediately, and events arrive later through the
inbox. The guest matches the envelope `id` against the UUID of the subscription
it wants to hear about.
