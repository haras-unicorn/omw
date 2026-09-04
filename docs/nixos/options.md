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

Environment variables exported at the start of the service script, before omw
starts\. They are also visible to `envsubst` when interpolating the
configuration, which is the intended way to supply API keys and other secrets\.

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

## services\.omw\.mode

Which mode to run omw in: `run` executes every agent once, `loop` keeps running
them, restarting agents that fail\.

_Type:_ one of “run”, “loop”

_Default:_

```nix
"loop"
```

## services\.omw\.settings

The omw configuration provided as an attribute set, rendered to TOML at build
time\. Mutually exclusive with `services.omw.settingsFile`\. Environment
variables of the form `$VAR` or `${VAR}` are substituted into the config before
omw reads it (see `services.omw.environment`), which is the intended way to
supply API keys and other secrets at runtime\.

_Type:_ null or TOML value

_Default:_

```nix
null
```

## services\.omw\.settingsFile

Path to an omw configuration file (TOML)\. Mutually exclusive with
`services.omw.settings`\. Environment variables are substituted into the file
before omw reads it\.

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
the rhai runtime) or `rhai` (adds the bundled rhai interpreter via the
`omw-rhai` package)\. Overridable with `package`\.

_Type:_ one of “default”, “rhai”

_Default:_

```nix
"default"
```
