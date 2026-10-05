# The bundled python type declarations: the deployment and testing config
# models (each namespaced under `OmwConfig` / `OmwTestConfig`) and the
# hand-written stubs for the `omw` global.

from __future__ import annotations

from enum import Enum
from typing import Any, Literal, Optional

from pydantic import BaseModel, ConfigDict, Field, RootModel, conint


class OmwConfig:
  class AgentConfig(BaseModel):
      runtime: str = Field(
          ..., description="Which named runtime implementation this agent's brain uses."
      )
      script: str = Field(..., description="The agent's brain script.")


  class Config4(BaseModel):
      listen: str = Field(
          ...,
          description='Socket address to listen on, e.g. `"127.0.0.1:37532"` or\n`"0.0.0.0:37532"`. Hostnames (e.g. `"localhost:37532"`) are rejected at\nstartup.',
      )


  class EndpointImpls1(Config4):
      kind: Literal['openai']


  class FsPerms(Enum):
      read_only = 'read_only'
      read_write = 'read_write'


  class ImplConfig(BaseModel):
      model_config = ConfigDict(
          extra='allow',
      )
      kind: str = Field(..., description='Which implementation this is.')


  class Preopen(BaseModel):
      guest_path: str
      host_path: str
      perms: FsPerms | None = None


  class Secret(RootModel[str]):
      root: str


  class ToolingImpls11(BaseModel):
      args: list[str] | None = []
      command: str
      env: dict[str, Secret] | None = Field({}, validate_default=True)
      transport: Literal['stdio']


  class ToolingImpls12(BaseModel):
      auth_token: Secret | None = None
      transport: Literal['http']
      url: str


  class ToolingImpls13(BaseModel):
      kind: Literal['mcp']


  class ToolingImpls14(ToolingImpls11, ToolingImpls13):
      pass


  class ToolingImpls15(ToolingImpls12, ToolingImpls13):
      pass


  class ToolingImpls1(RootModel[ToolingImpls14 | ToolingImpls15]):
      root: ToolingImpls14 | ToolingImpls15


  class ToolingImpls(RootModel[dict[str, ToolingImpls1 | ImplConfig]]):
      root: dict[str, ToolingImpls1 | ImplConfig]


  class Tunables(BaseModel):
      allow_unlocked_secrets: bool | None = Field(
          False,
          description='Permit secrets to stay unlocked (pageable) when `mlock` fails, e.g.\ninside containers where the outer `RLIMIT_MEMLOCK` cannot be raised.\nDefault `false` (fail-closed). Only enable where the weaker guarantee\nis acceptable.',
      )
      cancel_pumps_on_reload: bool | None = Field(
          True,
          description='Cancel open pumps (streams, timers, resources, tool calls) on reload.\n`false` keeps them across reload.',
      )
      inbox_bound: conint(ge=0) | None = Field(
          1024,
          description='How many events a single agent inbox buffers before sends fail.',
      )
      interrupt_budget_ms: conint(ge=0) | None = Field(
          100,
          description='Uninterrupted runtime execution allowed after grace expires, in ms.',
      )
      loop_backoff_cap_secs: conint(ge=0) | None = Field(
          30, description='Backoff cap for `loop` restarts on failure, in seconds.'
      )
      loop_backoff_start_ms: conint(ge=0) | None = Field(
          100, description='Backoff start for `loop` restarts on failure, in ms.'
      )
      recv_slice_ms: conint(ge=0) | None = Field(
          200,
          description='How long `recv_while` parks between early-abort checks, in ms.',
      )
      recv_timeout_secs: conint(ge=0) | None = Field(
          60,
          description='How long a blocking `recv` waits before timing out, in seconds.',
      )
      reload_grace_secs: conint(ge=0) | None = Field(
          5,
          description='How long the supervisor waits for a cooperative exit, in seconds.',
      )
      reload_poll_ms: conint(ge=0) | None = Field(
          200,
          description='How long the blocking-call helper waits between reload checks, in ms.',
      )
      session_buffer: conint(ge=0) | None = Field(
          8192,
          description='How many deltas a single endpoint session buffers before drops.',
      )
      test_timeout_secs: conint(ge=0) | None = Field(
          30,
          description='How long a single testing-harness run may take before it is force-stopped\nand every unsettled agent marked timed out, in seconds. `0` disables it.',
      )
      tooling_connect_backoff_cap_secs: conint(ge=0) | None = Field(
          30, description='Backoff cap for tooling reconnects on failure, in seconds.'
      )
      tooling_connect_backoff_start_ms: conint(ge=0) | None = Field(
          100, description='Backoff start for tooling reconnects on failure, in ms.'
      )
      trace_buffer: conint(ge=0) | None = Field(
          4096,
          description='How many trace events the `omw-test` broadcast channel buffers.',
      )
      tui_tab_capacity: conint(ge=0) | None = Field(
          2000,
          description='How many log lines each live-view tab keeps before the oldest are\ndropped. `0` means unlimited.',
      )
      tui_tick_ms: conint(ge=0) | None = Field(
          80,
          description="How long the live view's render loop waits between redraws, in ms. It is\nboth the spinner cadence and the maximum redraw interval.",
      )
      watch_debounce_ms: conint(ge=0) | None = Field(
          200,
          description='How long to coalesce the burst of file events a single save produces,\nin ms.',
      )


  class Config1(BaseModel):
      api_key: Secret | None = None
      base_url: str | None = None
      model: str | None = None
      params: dict[str, Any] | None = Field(
          None,
          description='Default generation params merged into every request body (temperature,\nmax_tokens, reasoning_effort, …). Per-call params override these.',
      )


  class Config21(BaseModel):
      args: list[str] | None = []
      command: str
      env: dict[str, Secret] | None = Field({}, validate_default=True)
      transport: Literal['stdio']


  class Config22(BaseModel):
      auth_token: Secret | None = None
      transport: Literal['http']
      url: str


  class Config2(RootModel[Config21 | Config22]):
      root: Config21 | Config22 = Field(
          ...,
          description='Impl-specific configuration for a single MCP server, selected by\ntransport.',
      )


  class Config3(BaseModel):
      allow_blocking_current_thread: bool | None = False
      allow_ip_name_lookup: bool | None = False
      allow_tcp: bool | None = False
      allow_udp: bool | None = False
      args: list[str] | None = []
      env: dict[str, Secret] | None = Field({}, validate_default=True)
      inherit_args: bool | None = False
      inherit_env: bool | None = False
      inherit_network: bool | None = False
      inherit_stderr: bool | None = False
      inherit_stdin: bool | None = False
      inherit_stdio: bool | None = False
      inherit_stdout: bool | None = False
      initial_cwd: str | None = None
      insecure_random_seed: conint(ge=0) | None = None
      max_random_size: conint(ge=0) | None = None
      preopens: list[Preopen] | None = None


  class EndpointImpls(RootModel[EndpointImpls1 | ImplConfig | None]):
      root: EndpointImpls1 | ImplConfig | None


  class ProviderImpls1(Config1):
      kind: Literal['openai']


  class ProviderImpls(RootModel[dict[str, ProviderImpls1 | ImplConfig]]):
      root: dict[str, ProviderImpls1 | ImplConfig]


  class RuntimeImpls1(Config3):
      kind: Literal['wasm']


  class RuntimeImpls(RootModel[dict[str, RuntimeImpls1 | ImplConfig]]):
      root: dict[str, RuntimeImpls1 | ImplConfig]


  class Config(BaseModel):
      agents: dict[str, AgentConfig] | None = Field(
          {},
          description='Agents that OMW is going to run, keyed by agent name.',
          validate_default=True,
      )
      endpoint: EndpointImpls | None = Field(
          None, description='Optional endpoint implementation.'
      )
      memory: dict[str, dict[str, Any]] | None = Field(
          {},
          description="Per-agent seeded memory, keyed by agent name then key. Seeded into the\nagent's memory before its brain first runs, so a test (or a deployment)\ncan fast-forward an agent to a state. Seeded values persist like any\nother memory, including across hot reloads.\n\nA value that is a string is stored verbatim; anything else (object,\narray, number, bool) is JSON-stringified, mirroring `memory-set`. Read it\nback with `memory-get` (raw) or `memory-get-as` (parsed).",
      )
      providers: ProviderImpls | None = Field(
          {}, description='Named provider implementations.', validate_default=True
      )
      runtime: RuntimeImpls | None = Field(
          {}, description='Named runtime implementations.', validate_default=True
      )
      tooling: ToolingImpls | None = Field(
          {}, description='Named tooling implementations.', validate_default=True
      )
      tunables: Tunables | None = Field(
          {
              'allow_unlocked_secrets': False,
              'cancel_pumps_on_reload': True,
              'inbox_bound': 1024,
              'interrupt_budget_ms': 100,
              'loop_backoff_cap_secs': 30,
              'loop_backoff_start_ms': 100,
              'recv_slice_ms': 200,
              'recv_timeout_secs': 60,
              'reload_grace_secs': 5,
              'reload_poll_ms': 200,
              'session_buffer': 8192,
              'test_timeout_secs': 30,
              'tooling_connect_backoff_cap_secs': 30,
              'tooling_connect_backoff_start_ms': 100,
              'trace_buffer': 4096,
              'tui_tab_capacity': 2000,
              'tui_tick_ms': 80,
              'watch_debounce_ms': 200,
          },
          description='Global runtime tunables.',
          validate_default=True,
      )


class OmwTestConfig:
  class After(RootModel[Literal['start'] | dict[str, Any]]):
      root: Literal['start'] | dict[str, Any] = Field(
          ...,
          description='When a scripted step fires: "start" (the default) or a `call`/`inbound` pattern to wait for.',
      )


  class AgentConfig(BaseModel):
      runtime: str = Field(
          ..., description="Which named runtime implementation this agent's brain uses."
      )
      script: str = Field(..., description="The agent's brain script.")


  class FsPerms(Enum):
      read_only = 'read_only'
      read_write = 'read_write'


  class ImplConfig(BaseModel):
      model_config = ConfigDict(
          extra='allow',
      )
      kind: str = Field(..., description='Which implementation this is.')


  class InboundMessage(BaseModel):
      content: str | None = None
      reasoning: str | None = None
      role: str | None = 'user'


  class MockToolCallSpec(BaseModel):
      arguments: Any
      id: str
      name: str


  class Preopen(BaseModel):
      guest_path: str
      host_path: str
      perms: FsPerms | None = None


  class ResourceContentUpdate(BaseModel):
      after: After | None = None
      content: str
      error: str | None = Field(
          None, description='Yield this error for the step instead of an `Updated`.'
      )
      pending: bool | None = Field(
          False,
          description='Never yield: the step stays pending forever (for cancellation tests).',
      )
      uri: str


  class ResourceInfo(BaseModel):
      description: str | None = None
      mime_type: str | None = None
      name: str
      uri: str


  class ResourceListUpdate(BaseModel):
      after: After | None = None
      error: str | None = Field(
          None, description='Yield this error for the step instead of a `ListChanged`.'
      )
      pending: bool | None = Field(
          False,
          description='Never yield: the step stays pending forever (for cancellation tests).',
      )
      resources: list[ResourceInfo]


  class ScriptedToolCall(BaseModel):
      after: After | None = None
      error: str | None = Field(
          None, description='Fail the call with this error (after the gate and delay).'
      )
      name: str
      pending: bool | None = Field(
          False,
          description='Never complete: the call stays pending forever (for cancellation tests).',
      )
      result: str | None = ''
      structured_content: Any | None = None


  class Secret(RootModel[str]):
      root: str


  class SessionEnd(BaseModel):
      abort: conint(ge=0) | None = None
      close: conint(ge=0) | None = None


  class Tool(BaseModel):
      description: str | None = None
      input_schema: Any = Field(..., description="JSON Schema for the tool's arguments.")
      name: str
      output_schema: Any | None = Field(
          None,
          description="Optional JSON Schema for the tool's structured output (MCP\n`outputSchema`). Absent when the tool declares no output shape.",
      )


  class Tunables(BaseModel):
      allow_unlocked_secrets: bool | None = Field(
          False,
          description='Permit secrets to stay unlocked (pageable) when `mlock` fails, e.g.\ninside containers where the outer `RLIMIT_MEMLOCK` cannot be raised.\nDefault `false` (fail-closed). Only enable where the weaker guarantee\nis acceptable.',
      )
      cancel_pumps_on_reload: bool | None = Field(
          True,
          description='Cancel open pumps (streams, timers, resources, tool calls) on reload.\n`false` keeps them across reload.',
      )
      inbox_bound: conint(ge=0) | None = Field(
          1024,
          description='How many events a single agent inbox buffers before sends fail.',
      )
      interrupt_budget_ms: conint(ge=0) | None = Field(
          100,
          description='Uninterrupted runtime execution allowed after grace expires, in ms.',
      )
      loop_backoff_cap_secs: conint(ge=0) | None = Field(
          30, description='Backoff cap for `loop` restarts on failure, in seconds.'
      )
      loop_backoff_start_ms: conint(ge=0) | None = Field(
          100, description='Backoff start for `loop` restarts on failure, in ms.'
      )
      recv_slice_ms: conint(ge=0) | None = Field(
          200,
          description='How long `recv_while` parks between early-abort checks, in ms.',
      )
      recv_timeout_secs: conint(ge=0) | None = Field(
          60,
          description='How long a blocking `recv` waits before timing out, in seconds.',
      )
      reload_grace_secs: conint(ge=0) | None = Field(
          5,
          description='How long the supervisor waits for a cooperative exit, in seconds.',
      )
      reload_poll_ms: conint(ge=0) | None = Field(
          200,
          description='How long the blocking-call helper waits between reload checks, in ms.',
      )
      session_buffer: conint(ge=0) | None = Field(
          8192,
          description='How many deltas a single endpoint session buffers before drops.',
      )
      test_timeout_secs: conint(ge=0) | None = Field(
          30,
          description='How long a single testing-harness run may take before it is force-stopped\nand every unsettled agent marked timed out, in seconds. `0` disables it.',
      )
      tooling_connect_backoff_cap_secs: conint(ge=0) | None = Field(
          30, description='Backoff cap for tooling reconnects on failure, in seconds.'
      )
      tooling_connect_backoff_start_ms: conint(ge=0) | None = Field(
          100, description='Backoff start for tooling reconnects on failure, in ms.'
      )
      trace_buffer: conint(ge=0) | None = Field(
          4096,
          description='How many trace events the `omw-test` broadcast channel buffers.',
      )
      tui_tab_capacity: conint(ge=0) | None = Field(
          2000,
          description='How many log lines each live-view tab keeps before the oldest are\ndropped. `0` means unlimited.',
      )
      tui_tick_ms: conint(ge=0) | None = Field(
          80,
          description="How long the live view's render loop waits between redraws, in ms. It is\nboth the spinner cadence and the maximum redraw interval.",
      )
      watch_debounce_ms: conint(ge=0) | None = Field(
          200,
          description='How long to coalesce the burst of file events a single save produces,\nin ms.',
      )


  class UsageSpec(BaseModel):
      completion_tokens: conint(ge=0) | None = None
      prompt_tokens: conint(ge=0) | None = None
      total_tokens: conint(ge=0) | None = None


  class Config2(BaseModel):
      delay_ms: conint(ge=0) | None = Field(
          10, description='How long each scripted step waits before firing.'
      )
      initial_resource_contents: dict[str, str] | None = Field(
          {},
          description='Canned resource content keyed by URI, returned by `read-resource`.',
      )
      initial_resource_list: list[ResourceInfo] | None = Field(
          [],
          description='The resource list `list-resources` starts from.',
          validate_default=True,
      )
      resource_content_updates: list[ResourceContentUpdate] | None = Field(
          None,
          description='Ordered, one-by-one content updates driven by `subscribe-resource`.',
      )
      resource_list_updates: list[ResourceListUpdate] | None = Field(
          None,
          description='Ordered, full-replacement resource-list updates driven by\n`subscribe-resource-list`.',
      )
      tool_calls: list[ScriptedToolCall] | None = Field(
          None,
          description='Ordered `call-tool` results; consumed one per call and name-verified.',
      )
      tools: list[Tool] | None = Field([], validate_default=True)


  class Config3(BaseModel):
      allow_blocking_current_thread: bool | None = False
      allow_ip_name_lookup: bool | None = False
      allow_tcp: bool | None = False
      allow_udp: bool | None = False
      args: list[str] | None = []
      env: dict[str, Secret] | None = Field({}, validate_default=True)
      inherit_args: bool | None = False
      inherit_env: bool | None = False
      inherit_network: bool | None = False
      inherit_stderr: bool | None = False
      inherit_stdin: bool | None = False
      inherit_stdio: bool | None = False
      inherit_stdout: bool | None = False
      initial_cwd: str | None = None
      insecure_random_seed: conint(ge=0) | None = None
      max_random_size: conint(ge=0) | None = None
      preopens: list[Preopen] | None = None


  class Config4(BaseModel):
      allow_blocking_current_thread: bool | None = False
      allow_ip_name_lookup: bool | None = False
      allow_tcp: bool | None = False
      allow_udp: bool | None = False
      args: list[str] | None = []
      env: dict[str, Secret] | None = Field({}, validate_default=True)
      inherit_args: bool | None = False
      inherit_env: bool | None = False
      inherit_network: bool | None = False
      inherit_stderr: bool | None = False
      inherit_stdin: bool | None = False
      inherit_stdio: bool | None = False
      inherit_stdout: bool | None = False
      initial_cwd: str | None = None
      insecure_random_seed: conint(ge=0) | None = None
      interpreter: str | None = None
      max_random_size: conint(ge=0) | None = None
      preopens: list[Preopen] | None = None


  class Config5(BaseModel):
      allow_blocking_current_thread: bool | None = False
      allow_ip_name_lookup: bool | None = False
      allow_tcp: bool | None = False
      allow_udp: bool | None = False
      args: list[str] | None = []
      env: dict[str, Secret] | None = Field({}, validate_default=True)
      inherit_args: bool | None = False
      inherit_env: bool | None = False
      inherit_network: bool | None = False
      inherit_stderr: bool | None = False
      inherit_stdin: bool | None = False
      inherit_stdio: bool | None = False
      inherit_stdout: bool | None = False
      initial_cwd: str | None = None
      insecure_random_seed: conint(ge=0) | None = None
      interpreter: str | None = None
      max_random_size: conint(ge=0) | None = None
      preopens: list[Preopen] | None = None


  class Config6(BaseModel):
      allow_blocking_current_thread: bool | None = False
      allow_ip_name_lookup: bool | None = False
      allow_tcp: bool | None = False
      allow_udp: bool | None = False
      args: list[str] | None = []
      env: dict[str, Secret] | None = Field({}, validate_default=True)
      inherit_args: bool | None = False
      inherit_env: bool | None = False
      inherit_network: bool | None = False
      inherit_stderr: bool | None = False
      inherit_stdin: bool | None = False
      inherit_stdio: bool | None = False
      inherit_stdout: bool | None = False
      initial_cwd: str | None = None
      insecure_random_seed: conint(ge=0) | None = None
      interpreter: str | None = None
      max_random_size: conint(ge=0) | None = None
      preopens: list[Preopen] | None = None


  class Request(BaseModel):
      after: After | None = Field(
          None,
          description='Ordering gate: absent or `"start"` fires as soon as the model is\nsubscribed; a `call`/`inbound` pattern waits for a matching trace event.',
      )
      messages: list[InboundMessage] | None = None
      model: str
      params: Any | None = Field(
          None,
          description='Opaque generation params the client submitted (temperature, …).',
      )
      session_end: SessionEnd | None = Field(
          None,
          description='Opt-in scripted ending: `{ close = N }` ends the session normally after\n`N` received deltas, `{ abort = N }` aborts it. Absent, the mock drains\nsilently as before.',
      )
      session_id: str = Field(
          ...,
          description='A user-written label for the session, so concurrent sessions can be told\napart. The mock correlates it with the opaque session UUID the host and\nbrain see and records both in the snapshot; it is never asserted or\nmatched. Reusing a label after its session closed is a new session with a\nnew UUID.',
      )
      stream: bool | None = Field(
          False,
          description='Whether the client asked for SSE. Informational only; the mock drains\nthe same session either way.',
      )
      tools: list[Tool] | None = Field([], validate_default=True)


  class RuntimeImpls1(Config3):
      kind: Literal['wasm']


  class RuntimeImpls2(Config4):
      kind: Literal['rhai']


  class RuntimeImpls3(Config5):
      kind: Literal['js']


  class RuntimeImpls4(Config6):
      kind: Literal['python']


  class RuntimeImpls(
      RootModel[
          dict[
              str,
              RuntimeImpls1 | RuntimeImpls2 | RuntimeImpls3 | RuntimeImpls4 | ImplConfig,
          ]
      ]
  ):
      root: dict[
          str, RuntimeImpls1 | RuntimeImpls2 | RuntimeImpls3 | RuntimeImpls4 | ImplConfig
      ]


  class ToolingImpls1(Config2):
      kind: Literal['mock']


  class ToolingImpls(RootModel[dict[str, ToolingImpls1 | ImplConfig]]):
      root: dict[str, ToolingImpls1 | ImplConfig]


  class Turn(BaseModel):
      content: str | None = Field(
          None, description='Plain content emitted before the terminal finish reason.'
      )
      error: str | None = Field(
          None,
          description="Fail the stream with this error, after emitting the turn's deltas.",
      )
      pending: bool | None = Field(
          False,
          description='Never yield: the stream stays pending forever (for cancellation tests).',
      )
      reasoning: str | None = Field(
          None, description='Reasoning/thinking content emitted as its own delta.'
      )
      tool_call: MockToolCallSpec | None = Field(
          None,
          description='A tool call, which makes the terminal finish reason `"tool_calls"`.',
      )
      usage: UsageSpec | None = Field(
          None, description='Token accounting attached to the terminal delta.'
      )


  class Config1(BaseModel):
      models: list[str] | None = Field(
          [],
          description='The model names `list-models` returns. Defaults to `["mock-model"]`.',
      )
      turns: list[Turn] | None = Field(
          None,
          description='Scripted turns; one is popped per `chat` (repeating the last once\nexhausted).',
      )


  class Config7(BaseModel):
      requests: list[Request] | None = None


  class EndpointImpls1(Config7):
      kind: Literal['mock']


  class EndpointImpls(RootModel[EndpointImpls1 | ImplConfig | None]):
      root: EndpointImpls1 | ImplConfig | None


  class ProviderImpls1(Config1):
      kind: Literal['mock']


  class ProviderImpls(RootModel[dict[str, ProviderImpls1 | ImplConfig]]):
      root: dict[str, ProviderImpls1 | ImplConfig]


  class Config(BaseModel):
      agents: dict[str, AgentConfig] | None = Field(
          {},
          description='Agents that OMW is going to run, keyed by agent name.',
          validate_default=True,
      )
      endpoint: EndpointImpls | None = Field(
          None, description='Optional endpoint implementation.'
      )
      memory: dict[str, dict[str, Any]] | None = Field(
          {},
          description="Per-agent seeded memory, keyed by agent name then key. Seeded into the\nagent's memory before its brain first runs, so a test (or a deployment)\ncan fast-forward an agent to a state. Seeded values persist like any\nother memory, including across hot reloads.\n\nA value that is a string is stored verbatim; anything else (object,\narray, number, bool) is JSON-stringified, mirroring `memory-set`. Read it\nback with `memory-get` (raw) or `memory-get-as` (parsed).",
      )
      providers: ProviderImpls | None = Field(
          {}, description='Named provider implementations.', validate_default=True
      )
      runtime: RuntimeImpls | None = Field(
          {}, description='Named runtime implementations.', validate_default=True
      )
      tooling: ToolingImpls | None = Field(
          {}, description='Named tooling implementations.', validate_default=True
      )
      tunables: Tunables | None = Field(
          {
              'allow_unlocked_secrets': False,
              'cancel_pumps_on_reload': True,
              'inbox_bound': 1024,
              'interrupt_budget_ms': 100,
              'loop_backoff_cap_secs': 30,
              'loop_backoff_start_ms': 100,
              'recv_slice_ms': 200,
              'recv_timeout_secs': 60,
              'reload_grace_secs': 5,
              'reload_poll_ms': 200,
              'session_buffer': 8192,
              'test_timeout_secs': 30,
              'tooling_connect_backoff_cap_secs': 30,
              'tooling_connect_backoff_start_ms': 100,
              'trace_buffer': 4096,
              'tui_tab_capacity': 2000,
              'tui_tick_ms': 80,
              'watch_debounce_ms': 200,
          },
          description='Global runtime tunables.',
          validate_default=True,
      )


"""Type stubs for the `omw` global the bundled `omw-wasm-python-interpreter`
component injects into every python brain script.

Vendor `omw.all.py` (the generated deployment/testing config models, namespaced
under `OmwConfig` / `OmwTestConfig`, plus this file) next to your `brain.py` and
point your type checker at it. See docs/runtime/python.md for the prose
reference.
"""

from typing import Any, Literal, Optional

Role = Literal["system", "user", "assistant", "tool"]

class ToolCall:
  id: str
  name: str
  arguments: Any

class ChatMessage:
  role: Role
  content: Optional[str]
  reasoning: Optional[str]
  tool_call: Optional[ToolCall]

class Usage:
  prompt_tokens: Optional[int]
  completion_tokens: Optional[int]
  total_tokens: Optional[int]

class ChatDelta:
  content: Optional[str]
  reasoning: Optional[str]
  tool_call: Optional[ToolCall]
  finish_reason: Optional[str]
  usage: Optional[Usage]

class Tool:
  name: str
  description: Optional[str]
  input_schema: Any
  output_schema: Any

class ChatResult:
  content: Optional[str]
  reasoning: Optional[str]
  tool_calls: list[ToolCall]
  finish_reason: Optional[str]
  usage: Optional[Usage]

class ToolResult:
  name: str
  arguments: Any
  content: Any
  structured_content: Any

class ResourceInfo:
  uri: str
  name: str
  description: Optional[str]
  mime_type: Optional[str]

class ResourceContent:
  uri: str
  mime_type: Optional[str]
  content: str

class EventEnvelope:
  id: str
  kind: str
  payload: Any

class Provider:
  name: str
  def chat(
    self,
    model: str,
    messages: list[ChatMessage],
    tools: list[Tool],
    params: Any = ...,
  ) -> ChatResult: ...
  def chat_stream(
    self,
    model: str,
    messages: list[ChatMessage],
    tools: list[Tool],
    params: Any = ...,
  ) -> str: ...
  def is_open(self, uuid: str) -> bool: ...
  def cancel(self, uuid: str) -> None: ...
  def list_models(self) -> list[str]: ...
  def kind(self) -> str: ...

class Tooling:
  name: str
  def list_tools(self) -> list[Tool]: ...
  def call_tool(self, tool: str, arguments: Any) -> str: ...
  def call_tool_blocking(self, tool: str, arguments: Any) -> ToolResult: ...
  def is_open(self, uuid: str) -> bool: ...
  def cancel(self, uuid: str) -> None: ...
  def kind(self) -> str: ...
  def list_resources(self) -> list[ResourceInfo]: ...
  def read_resource(self, uri: str) -> ResourceContent: ...
  def subscribe_resource_list(self) -> str: ...
  def subscribe_resource(self, uri: str) -> str: ...
  def unsubscribe_resource_list(self, uuid: str) -> None: ...
  def unsubscribe_resource(self, uuid: str) -> None: ...

class ProviderNamespace:
  def get(self, name: str) -> Provider: ...

class ToolingNamespace:
  def get(self, name: str) -> Tooling: ...

class Host:
  def log(self, level: str, message: str) -> None: ...
  def whoami(self) -> str: ...
  def time_now(self) -> int: ...
  def time_format(self, ts: int, format: str) -> str: ...
  def wait_until(self, ts: int) -> str: ...
  def wait_for(self, ms: int) -> str: ...
  def wait_cron(self, spec: str) -> str: ...
  def sleep_for(self, ms: int) -> None: ...
  def sleep_until(self, ts: int) -> None: ...
  def sleep_cron(self, spec: str) -> None: ...
  def cancel_timer(self, uuid: str) -> None: ...
  def subscribe_agent(self, agent: str) -> str: ...
  def unsubscribe_agent(self, uuid: str) -> None: ...
  def subscribe_lifecycle(self) -> str: ...
  def unsubscribe_lifecycle(self, uuid: str) -> None: ...
  def send_agent(self, agent: str, payload: str) -> None: ...
  def recv(self) -> EventEnvelope: ...
  def try_recv(self) -> Optional[EventEnvelope]: ...
  def new_uuid(self) -> str: ...
  def base64_encode(self, bytes: Any) -> str: ...
  def base64_decode(self, data: str) -> bytes: ...
  def memory_get(self, key: str) -> Optional[str]: ...
  def memory_get_as(self, key: str) -> Any: ...
  def memory_set(self, key: str, value: str) -> None: ...
  def memory_set_as(self, key: str, value: Any) -> None: ...
  def memory_remove(self, key: str) -> bool: ...
  def subscribe_endpoint(self, model: str) -> str: ...
  def unsubscribe_endpoint(self, uuid: str) -> None: ...
  def stream_endpoint(self, session: str, delta: Any) -> None: ...

class Omw:
  host: Host
  provider: ProviderNamespace
  tooling: ToolingNamespace

omw: Omw
