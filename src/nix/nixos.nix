{ self, ... }:

let
  module =
    {
      config,
      lib,
      pkgs,
      ...
    }:
    let
      cfg = config.services.omw;

      settingsFormat = pkgs.formats.toml { };
    in
    {
      options.services.omw = {
        enable = lib.mkEnableOption "the omw agent runtime";

        variant = lib.mkOption {
          type = lib.types.enum [
            "default"
            "rhai"
            "js"
          ];
          default = "default";
          description = ''
            Which package variant to run: `default` (the crates.io-equivalent
            build, without the rhai runtime), `rhai` (adds the bundled rhai
            interpreter via the `omw-rhai` package) or `js` (adds the bundled
            js interpreter via the `omw-js` package). Overridable with `package`.
          '';
        };

        package = lib.mkOption {
          type = lib.types.package;
          default =
            if cfg.variant == "rhai" then
              self.packages.${pkgs.stdenv.hostPlatform.system}.omw-rhai
            else if cfg.variant == "js" then
              self.packages.${pkgs.stdenv.hostPlatform.system}.omw-js
            else
              self.packages.${pkgs.stdenv.hostPlatform.system}.omw;
          description = "The omw package to run.";
        };

        mode = lib.mkOption {
          type = lib.types.enum [
            "run"
            "loop"
          ];
          default = "loop";
          description = ''
            Which mode to run omw in: `run` executes every agent once, `loop` keeps
            running them, restarting agents that fail.
          '';
        };

        settings = lib.mkOption {
          type = lib.types.nullOr settingsFormat.type;
          default = null;
          description = ''
            The omw configuration provided as an attribute set, rendered to TOML
            at build time. Mutually exclusive with `services.omw.settingsFile`.
            Secrets are layered at runtime from `OMW__`-prefixed environment
            variables (see `services.omw.environment`), so API keys never have
            to live in the Nix store.
          '';
        };

        settingsFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = ''
            Path to an omw configuration file (TOML). Mutually exclusive with
            `services.omw.settings`.
          '';
        };

        environment = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = { };
          description = ''
            Environment variables for the service (becomes systemd
            `Environment=`). `OMW__`-prefixed variables layer over the file
            configuration, e.g. `OMW__PROVIDERS__OPENAI__API_KEY` overrides
            `providers.openai.api_key`, which is the intended way to supply
            API keys and other secrets.
          '';
        };

        environmentFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = "Path to a systemd `EnvironmentFile` for the service.";
        };

        hardening = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = ''
            Enable systemd hardening (`NoNewPrivileges`, `ProtectSystem=strict`,
            …). Safe for stdio MCP servers (including ones wrapping commands in
            `bwrap`) and node-based servers. Sets `LimitMEMLOCK=infinity`
            alongside the empty capability set (inside containers the outer
            `RLIMIT_MEMLOCK` still wins — set
            `OMW__TUNABLES__ALLOW_UNLOCKED_SECRETS=true` in the environment
            there instead). Deliberately omits
            `MemoryDenyWriteExecute`, which would break the wasmtime JIT and
            nodejs MCP servers. Set to false if the sandbox gets in the way;
            `services.omw.serviceConfig` can override individual keys either way.
          '';
        };

        readOnlyPaths = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = ''
            Extra paths exposed read-only inside the sandbox
            (`BindReadOnlyPaths=`). Needed with hardening when brains or the
            config file live outside the state directory (e.g. `/etc`).
          '';
        };

        readWritePaths = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = ''
            Extra paths exposed read-write inside the sandbox
            (`ReadWritePaths=`). Needed with hardening when a filesystem MCP
            tooling works outside the state directory.
          '';
        };

        serviceConfig = lib.mkOption {
          type = lib.types.attrs;
          default = { };
          description = ''
            Extra systemd `serviceConfig` merged last, so it wins over the
            module defaults (including the hardening set). Escape hatch for
            anything the module does not model explicitly.
          '';
        };

        extraArgs = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "Extra arguments passed to the omw command line after the mode.";
        };

        user = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = ''
            The user the service runs as. When both `services.omw.user` and
            `services.omw.group` are null, a dynamic user is allocated.
          '';
        };

        group = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = ''
            The group the service runs as. When both `services.omw.user` and
            `services.omw.group` are null, a dynamic user is allocated.
          '';
        };

        stateDir = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = ''
            Name of the state directory created for the service
            (`StateDirectory=`). When set, the directory is created under
            `/var/lib` and the service can persist state there, e.g. the workspace
            of a filesystem MCP tooling.
          '';
        };
      };

      config = lib.mkIf cfg.enable {
        assertions = [
          {
            assertion = !(cfg.settings == null && cfg.settingsFile == null);
            message = "omw requires settings via either services.omw.settings or services.omw.settingsFile.";
          }
          {
            assertion = !(cfg.settings != null && cfg.settingsFile != null);
            message = "services.omw.settings and services.omw.settingsFile are mutually exclusive.";
          }
        ];

        systemd.services.omw =
          let
            settingsPath =
              if cfg.settings != null then settingsFormat.generate "omw.toml" cfg.settings else cfg.settingsFile;

            args = lib.escapeShellArgs (
              [
                cfg.mode
                "--config"
                (toString settingsPath)
              ]
              ++ cfg.extraArgs
            );
          in
          {
            description = "OMW agent runtime";
            wantedBy = [ "multi-user.target" ];
            after = [ "network-online.target" ];
            wants = [ "network-online.target" ];

            path = [ cfg.package ];

            script = "exec omw ${args}";

            environment = cfg.environment;

            serviceConfig = lib.mkMerge [
              {
                Type = "simple";
                Restart = "on-failure";
                RestartSec = "5s";
                User = lib.mkIf (cfg.user != null) cfg.user;
                Group = lib.mkIf (cfg.group != null) cfg.group;
                DynamicUser = lib.mkIf (cfg.user == null && cfg.group == null) true;
                EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;
                StateDirectory = lib.mkIf (cfg.stateDir != null) cfg.stateDir;
                WorkingDirectory = lib.mkIf (cfg.stateDir != null) "/var/lib/${cfg.stateDir}";
              }
              (lib.mkIf cfg.hardening {
                NoNewPrivileges = true;
                PrivateDevices = false;
                PrivateIPC = true;
                ProtectClock = true;
                ProtectControlGroups = true;
                ProtectHome = true;
                ProtectKernelModules = true;
                ProtectProc = "invisible";
                ProtectSystem = "strict";
                RemoveIPC = true;
                RestrictAddressFamilies = [
                  "AF_UNIX"
                  "AF_NETLINK"
                  "AF_INET"
                  "AF_INET6"
                ];
                RestrictRealtime = true;
                RestrictSUIDSGID = true;
                LockPersonality = true;
                SystemCallArchitectures = "native";
                UMask = "0077";
                LimitMEMLOCK = "infinity";
                CapabilityBoundingSet = "";
                AmbientCapabilities = "";
                BindReadOnlyPaths = lib.mkIf (cfg.readOnlyPaths != [ ]) cfg.readOnlyPaths;
                ReadWritePaths = lib.mkIf (cfg.readWritePaths != [ ]) cfg.readWritePaths;
              })
              cfg.serviceConfig
            ];
          };
      };
    };
in
{
  flake.nixosModules = {
    default = module;
    omw = module;
  };
}
