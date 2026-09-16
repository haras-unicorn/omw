## services\.omw\.enable

Whether to enable the omw agent runtime\.

_Type:_ boolean

_Default:_

```nix
false
```

_Example:_

```nix
true
```

## services\.omw\.package

The omw package to run\.

_Type:_ package

_Default:_

```nix
<derivation omw>
```

## services\.omw\.environment

Environment variables for the service (becomes systemd `Environment=`)\.
`OMW__`-prefixed variables layer over the file configuration, e\.g\.
`OMW__PROVIDERS__OPENAI__API_KEY` overrides `providers.openai.api_key`, which is
the intended way to supply API keys and other secrets\.

_Type:_ attribute set of string

_Default:_

```nix
{ }
```

## services\.omw\.environmentFile

Path to a systemd `EnvironmentFile` for the service\.

_Type:_ null or absolute path

_Default:_

```nix
null
```

## services\.omw\.extraArgs

Extra arguments passed to the omw command line after the mode\.

_Type:_ list of string

_Default:_

```nix
[ ]
```

## services\.omw\.group

The group the service runs as\. When both `services.omw.user` and
`services.omw.group` are null, a dynamic user is allocated\.

_Type:_ null or string

_Default:_

```nix
null
```

## services\.omw\.hardening

Enable systemd hardening (`NoNewPrivileges`, `ProtectSystem=strict`, …)\. Safe
for stdio MCP servers (including ones wrapping commands in `bwrap`) and
node-based servers\. Sets `LimitMEMLOCK=infinity` alongside the empty capability
set (inside containers the outer `RLIMIT_MEMLOCK` still wins — set
`OMW__TUNABLES__ALLOW_UNLOCKED_SECRETS=true` in the environment there instead)\.
Deliberately omits `MemoryDenyWriteExecute`, which would break the wasmtime JIT
and nodejs MCP servers\. Set to false if the sandbox gets in the way;
`services.omw.serviceConfig` can override individual keys either way\.

_Type:_ boolean

_Default:_

```nix
true
```

## services\.omw\.mode

Which mode to run omw in: `run` executes every agent once, `loop` keeps running
them, restarting agents that fail\.

_Type:_ one of “run”, “loop”

_Default:_

```nix
"loop"
```

## services\.omw\.readOnlyPaths

Extra paths exposed read-only inside the sandbox (`BindReadOnlyPaths=`)\. Needed
with hardening when brains or the config file live outside the state directory
(e\.g\. `/etc`)\.

_Type:_ list of string

_Default:_

```nix
[ ]
```

## services\.omw\.readWritePaths

Extra paths exposed read-write inside the sandbox (`ReadWritePaths=`)\. Needed
with hardening when a filesystem MCP tooling works outside the state directory\.

_Type:_ list of string

_Default:_

```nix
[ ]
```

## services\.omw\.serviceConfig

Extra systemd `serviceConfig` merged last, so it wins over the module defaults
(including the hardening set)\. Escape hatch for anything the module does not
model explicitly\.

_Type:_ attribute set

_Default:_

```nix
{ }
```

## services\.omw\.settings

The omw configuration provided as an attribute set, rendered to TOML at build
time\. Mutually exclusive with `services.omw.settingsFile`\. Secrets are layered
at runtime from `OMW__`-prefixed environment variables (see
`services.omw.environment`), so API keys never have to live in the Nix store\.

_Type:_ null or TOML value

_Default:_

```nix
null
```

## services\.omw\.settingsFile

Path to an omw configuration file (TOML)\. Mutually exclusive with
`services.omw.settings`\.

_Type:_ null or absolute path

_Default:_

```nix
null
```

## services\.omw\.stateDir

Name of the state directory created for the service (`StateDirectory=`)\. When
set, the directory is created under `/var/lib` and the service can persist state
there, e\.g\. the workspace of a filesystem MCP tooling\.

_Type:_ null or string

_Default:_

```nix
null
```

## services\.omw\.user

The user the service runs as\. When both `services.omw.user` and
`services.omw.group` are null, a dynamic user is allocated\.

_Type:_ null or string

_Default:_

```nix
null
```

## services\.omw\.variant

Which package flavor to run: `default` (the crates\.io-equivalent build, without
the rhai runtime), `rhai` (adds the bundled rhai interpreter via the `omw-rhai`
package) or `js` (adds the bundled js interpreter via the `omw-js` package)\.
Overridable with `package`\.

_Type:_ one of “default”, “rhai”, “js”

_Default:_

```nix
"default"
```
