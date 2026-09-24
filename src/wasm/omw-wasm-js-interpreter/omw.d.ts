// TypeScript declarations for the `omw` global that the bundled
// `omw-wasm-js-interpreter` component injects into every js brain script.
// Reference it from your editor (`/// <reference path="./omw.d.ts" />`) to get
// completion and checking inside `brain.js`. Scripts are evaluated as plain
// scripts, so nothing is imported: `omw` is a global.
//
// See docs/runtime/js.md for the prose reference.

/** A sender role in a chat conversation. */
type Role = "system" | "user" | "assistant" | "tool";

/** A request for the model to invoke one named tool. */
interface ToolCall {
  /** Tool call identifier. */
  id: string;

  /** Tool name. */
  name: string;

  /** Tool arguments as an opaque JSON string. */
  arguments: string;
}

/** A single message in a chat conversation. */
interface ChatMessage {
  /** The sender; defaults to `"user"` when omitted. */
  role?: Role;

  /** Optional text content; absent on messages bearing only a tool call. */
  content?: string;

  /** Optional tool call attached to this message. */
  tool_call?: ToolCall;
}

/** One incremental chunk of a streaming chat response. */
interface ChatDelta {
  /** Optional text content produced by this chunk. */
  content?: string;

  /** Optional tool call announced by this chunk. */
  tool_call?: ToolCall;

  /** Optional terminal reason the stream stopped (`"stop"`, `"length"`, …). */
  finish_reason?: string;
}

/** A callable tool offered to the model, as passed to `chat`/`chatStream`. */
interface ChatTool {
  /** The tool's name. */
  name: string;

  /** Optional human-readable description of what the tool does. */
  description?: string;

  /** The tool's JSON-schema document; defaults to `"{}"` when omitted. */
  input_schema?: string;
}

/** A tool as returned by `listTools`, carrying both key spellings. */
interface Tool {
  /** The tool's name. */
  name: string;

  /** Optional human-readable description of what the tool does. */
  description?: string;

  /** The tool's JSON-schema document (camelCase alias). */
  inputSchema: string;

  /** The tool's JSON-schema document. */
  input_schema: string;
}

/** The full result of a blocking `chat` call. */
interface ChatResult {
  /** The concatenated text content; absent if the response was only calls. */
  content?: string;

  /** The reassembled tool calls the model made, in first-seen order. */
  tool_calls: ToolCall[];

  /** Optional terminal reason the response stopped (`"stop"`, …). */
  finish_reason?: string;
}

/** The result of a single tool invocation. */
interface ToolResult {
  /** The tool's name. */
  name: string;

  /** The tool's arguments, as the opaque JSON passed to `callTool`. */
  arguments: string;

  /** The tool's JSON result. */
  value: string;
}

/** A tooling resource as returned by `listResources`. */
interface ResourceInfo {
  /** The URI of this resource (e.g. `file:///path/to/file`). */
  uri: string;

  /** The programmatic name of the resource. */
  name: string;

  /** Optional description of what this resource represents. */
  description?: string;

  /** The MIME type of the resource, if known (camelCase alias). */
  mimeType?: string;

  /** The MIME type of the resource, if known. */
  mime_type?: string;
}

/**
 * The content of a single resource. `content` holds actual text for textual
 * formats and base64-encoded bytes for anything else; match on `mime_type` to
 * tell the two apart.
 */
interface ResourceContent {
  /** The URI of the resource the content belongs to. */
  uri: string;

  /** The MIME type of the resource, if known (camelCase alias). */
  mimeType?: string;

  /** The MIME type of the resource, if known. */
  mime_type?: string;

  /** The resource's content: plain text, or base64 for binary formats. */
  content: string;
}

/** An inbound endpoint chat request routed to a subscribed agent. */
interface EndpointMessage {
  /** UUID of the session; deltas streamed back address this. */
  session: string;

  /** The chat history as submitted by the endpoint client. */
  messages: ChatMessage[];

  /** The tools advertised by the endpoint client. */
  tools: ChatTool[];
}

/** An endpoint session ended, normally or abruptly. */
interface EndpointSessionEnd {
  /** UUID of the session that ended. */
  session: string;

  /** Present when the session ended abruptly rather than via a reason. */
  error?: string;
}

/** An event delivered to the agent's inbox, tagged with its source UUID. */
type OmwEvent =
  | { id: string; kind: "message"; payload: string }
  | { id: string; kind: "error"; payload: string }
  | { id: string; kind: "timer"; payload: null }
  | { id: string; kind: "reload"; payload: null }
  | { id: string; kind: "shutdown"; payload: null }
  | { id: string; kind: "chat-delta"; payload: ChatDelta }
  | { id: string; kind: "chat-end"; payload: null }
  | { id: string; kind: "tool-result"; payload: ToolResult }
  | { id: string; kind: "resource-list-updated"; payload: ResourceInfo[] }
  | { id: string; kind: "resource-updated"; payload: ResourceContent }
  | { id: string; kind: "endpoint-message"; payload: EndpointMessage }
  | { id: string; kind: "endpoint-session-end"; payload: EndpointSessionEnd };

/** A configured provider instance, returned by `omw.provider.get`. */
interface ProviderHandle {
  /** The configured name of this instance. */
  name: string;

  /** Which implementation this is. */
  kind(): string;

  /** Model names this provider exposes. */
  listModels(): string[];

  /**
   * Run a chat conversation to completion and return the full result in-band.
   * Unlike `chatStream`, no events are delivered and the call cannot be
   * cancelled.
   */
  chat(model: string, messages: ChatMessage[], tools: ChatTool[]): ChatResult;

  /**
   * Open a streaming chat response. Returns a UUID handle; deltas flow into the
   * inbox as `"chat-delta"` events until a terminal `"chat-end"` (or `"error"`)
   * closes the stream.
   */
  chatStream(model: string, messages: ChatMessage[], tools: ChatTool[]): string;

  /** Whether a chat stream is still open. */
  isOpen(uuid: string): boolean;

  /** Cancel an open stream by UUID. */
  cancel(uuid: string): void;
}

/** A configured tooling instance, returned by `omw.tooling.get`. */
interface ToolingHandle {
  /** The configured name of this instance. */
  name: string;

  /** Which implementation this is. */
  kind(): string;

  /** Every tool this tooling exposes, callable via `callTool`. */
  listTools(): Tool[];

  /**
   * Invoke a tool by name with JSON-serializable arguments; queues the call and
   * returns a UUID handle. The result arrives as a `"tool-result"` event tagged
   * with that UUID (or an `"error"` event on failure).
   */
  callTool(tool: string, args: unknown): string;

  /** Whether a queued tool call is still open. */
  isOpen(uuid: string): boolean;

  /** Cancel a queued tool call by UUID. */
  cancel(uuid: string): void;

  /** Invoke a tool, blocking until the result is ready. */
  callToolBlocking(tool: string, args: unknown): ToolResult;

  /** Enumerate every resource this tooling exposes. */
  listResources(): ResourceInfo[];

  /** Read one resource's current content. */
  readResource(uri: string): ResourceContent;

  /** Subscribe to the resource list changing; returns a UUID handle. */
  subscribeResourceList(): string;

  /** Cancel a resource-list subscription by UUID. */
  unsubscribeResourceList(uuid: string): void;

  /** Subscribe to one resource's updates; returns a UUID handle. */
  subscribeResource(uri: string): string;

  /** Cancel a single-resource subscription by UUID. */
  unsubscribeResource(uuid: string): void;
}

/** The static host helpers, baked into the runtime. */
interface Host {
  /** Write a structured log line; unknown levels default to `"info"`. */
  log(
    level: "trace" | "debug" | "info" | "warn" | "error",
    message: string,
  ): void;

  /** Current wall clock as ticks (milliseconds since the Unix epoch). */
  timeNow(): number;

  /** Format a tick with a strftime-style format string. */
  timeFormat(ts: number, format: string): string;

  /** Wait until a future timestamp; errors if `ts` is not in the future. */
  waitUntil(ts: number): string;

  /** Wait for `ms` milliseconds; returns a UUID handle. */
  waitFor(ms: number): string;

  /** Wait until the next fire of a cron spec; returns a UUID handle. */
  waitCron(spec: string): string;

  /** Blocking wait for `ms` milliseconds (no `"timer"` event). */
  sleepFor(ms: number): void;

  /** Blocking wait until a future timestamp (no `"timer"` event). */
  sleepUntil(ts: number): void;

  /** Blocking wait until the next fire of a cron spec (no `"timer"` event). */
  sleepCron(spec: string): void;

  /** Cancel a pending wait by the UUID its `wait*` call returned. */
  cancelTimer(uuid: string): void;

  /** Subscribe to messages from another agent; returns a UUID handle. */
  subscribeAgent(agent: string): string;

  /** Unsubscribe from another agent's messages by UUID. */
  unsubscribeAgent(uuid: string): void;

  /** Subscribe to lifecycle events; returns a UUID handle (one per run). */
  subscribeLifecycle(): string;

  /** Drop the lifecycle subscription; a foreign UUID is a no-op. */
  unsubscribeLifecycle(uuid: string): void;

  /** Send text to another agent. */
  sendAgent(agent: string, payload: string): void;

  /** Blocking receive of the next event from this agent's inbox. */
  recv(): OmwEvent;

  /** Non-blocking poll of the next event; `undefined` when the inbox is empty. */
  tryRecv(): OmwEvent | undefined;

  /** A fresh v4 UUID string. */
  newUuid(): string;

  /** Encode raw bytes as standard padded base64. */
  base64Encode(bytes: number[]): string;

  /** Decode standard padded base64 back to raw bytes; errors on bad input. */
  base64Decode(data: string): number[];

  /** Read a memory value by key; `undefined` when absent. */
  memoryGet(key: string): string | undefined;

  /** Store a memory value under a key, overwriting. */
  memorySet(key: string, value: string): void;

  /** Delete a memory value by key; `true` when a value was present. */
  memoryRemove(key: string): boolean;

  /** Subscribe this agent to the endpoint under a model name. */
  subscribeEndpoint(model: string): string;

  /** Drop an endpoint model subscription by UUID. */
  unsubscribeEndpoint(uuid: string): void;

  /** Stream one `chat-delta` to an endpoint session. */
  streamEndpoint(session: string, delta: ChatDelta): void;
}

/** The `omw` global. */
interface Omw {
  /** Look up configured provider instances by name. */
  provider: {
    get(name: string): ProviderHandle;
  };

  /** Look up configured tooling instances by name. */
  tooling: {
    get(name: string): ToolingHandle;
  };

  /** The static host helpers. */
  host: Host;
}

declare const omw: Omw;
