# The tooling interface

A _tooling_ is an abstraction over an MCP-style tool server: it exposes callable
_tools_ and readable _resources_. Named toolings live in the global
`[tooling.<name>]` config map and are looked up at runtime by name.

## The abstraction

A tooling exposes, through the WIT `tooling` interface:

- `kind()` — which implementation this is (e.g. `mcp`).
- `name()` — the configured name of the instance.
- `list-tools()` — every tool visible on the instance.
- `call-tool(name, arguments)` — queue a tool invocation, returning a UUID
  handle. The result arrives as a `tool-result` event (or an `error` event on
  failure) tagged with that UUID.
- `is-open(uuid)` — whether a queued tool call is still open.
- `cancel(uuid)` — cancel a queued tool call by UUID, dropping its pending
  result delivery.
- `call-tool-blocking(name, arguments)` — invoke a tool by name with opaque JSON
  arguments, blocking until the result is ready. Returns the tool's result as a
  `tool-result` (`name`, `arguments`, `content`, optional `structured-content`,
  `is-error`) in-band (errors are surfaced as the `err`).
- `list-resources()` — every URI-addressed resource the tooling exposes.
- `read-resource(uri)` — block and read one resource's _current_ contents — it
  returns a list of `resource-content` (`uri`, optional `mime-type`, and
  `content` which is text for textual formats and base64 for anything else),
  because a single read may yield several (for example, every file behind a
  directory resource).
- `list-resource-templates()` — every parameterized resource the tooling
  exposes; a brain fills a `resource-template`'s URI template to a concrete URI,
  then reads or subscribes it.
- `subscribe-resource-list()` — subscribe to the resource _list_ changing;
  returns a UUID handle tagged on each `resource-list-updated` event, which
  carries the freshly fetched resource list.
- `unsubscribe-resource-list(uuid)` — cancel a resource-list subscription by
  that UUID.
- `subscribe-resource(uri)` — subscribe to one resource's updates; returns a
  UUID handle tagged on each `resource-updated` event, which carries the freshly
  read resource contents (a `list<resource-content>`).
- `unsubscribe-resource(uuid)` — cancel a single-resource subscription by that
  UUID.

The handle is obtained once with `tooling.get(name)`, which returns a `tooling`
resource; all further calls go through that handle.

## Tools

A `tool` has a `name`, an optional human-readable `title`, an optional
`description`, an `input-schema` — a JSON Schema describing the arguments the
model must supply — and an optional `output-schema` describing the tool's
structured output (MCP's `outputSchema`); it is absent when the tool declares no
output shape. It may also carry `annotations` (MCP `ToolAnnotations`): the
optional behavioral hints `read-only-hint`, `destructive-hint`,
`idempotent-hint` and `open-world-hint`. The guest hands the signature to a
provider so the model can emit a `tool-call` for it, then invokes it with
`call-tool`, or synchronously with `call-tool-blocking`.

## Tool results

A `tool-result` carries the tool's `name`, the opaque JSON `arguments` it was
invoked with, its `content`, an optional `structured-content`, and an `is-error`
flag:

- `content` mirrors MCP `content`: a JSON array of content blocks, so a text
  result's text is `content[0].text` (in a typed client, parse `content` into a
  list of blocks).
- `structured-content` mirrors MCP `structuredContent` when the tool declares
  and produces one; absent otherwise.
- `is-error` mirrors MCP `isError`: when set, the call is a tool-execution
  failure, not a success, and the error text is in `content`. A protocol failure
  is delivered as an `error` event instead.

Both `content` and `structured-content` are opaque JSON strings on the wire. The
rhai and js guests parse them in place (objects/arrays when the text is valid
JSON, else the raw string), and the Rust SDK exposes `content_as<T>()` /
`structured_content_as<T>()` accessors. `arguments` is parsed the same way.

## Resources

A `resource-info` is a single URI-addressed, readable value: it carries a `uri`,
a programmatic `name`, an optional human-readable `title`, an optional
`description`, an optional `mime-type`, an optional `size` in bytes, and
optional `annotations` (MCP resource `annotations`: `audience`, `priority` and
`last-modified`). Resources are how a tooling surfaces read-only data (a file, a
database row, a metric) without it being an instruction-following tool.

A `resource-template` is a parameterized resource (MCP `ResourceTemplate`): an
RFC 6570 `uri-template`, a programmatic `name`, and optional `title`,
`description` and `mime-type`. A brain fills the template to a concrete URI,
then reads or subscribes that URI like any other resource.

A `resource-content` is the content of one resource read at the moment its
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
`list-resources`; a `resource-updated` event carries the resource contents read
at update time (a `list<resource-content>`), so the guest never has to read the
resource back itself. Each subscription can be cancelled early with
`unsubscribe-resource-list` / `unsubscribe-resource` by that UUID. Delivery
contract: **dropping the returned stream cancels the subscription**.

## The streaming contract

`subscribe-resource-list` and `subscribe-resource` mirror the provider's
chat-stream contract: they return a UUID immediately, and events arrive later
through the inbox. The guest matches the envelope `id` against the UUID of the
subscription it wants to hear about. `call-tool` follows the same shape: it
returns a UUID held for the queued invocation, and the `tool-result` event
arrives later under that UUID (`call-tool-blocking` is instead the synchronous
mirror, returning the result in-band).
