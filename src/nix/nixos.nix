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
          ];
          default = "default";
          description = ''
            Which package flavor to run: `default` (the crates.io-equivalent
            build, without the rhai runtime) or `rhai` (adds the bundled rhai
            interpreter via the `omw-rhai` package). Overridable with `package`。
          '';
        };

        package = lib.mkOption {
          type = lib.types.package;
          default =
            if cfg.variant == "rhai" then
              self.packages.${pkgs.stdenv.hostPlatform.system}.omw-rhai
            else
              self.packages.${pkgs.stdenv.hostPlatform.system}.default;
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
            The omw configuration as an attribute set, rendered to TOML at build
            time. Mutually exclusive with `services.omw.settingsFile`. Environment
            variables of the form `$VAR` or `''${VAR}` are substituted into the config
            before omw reads it (see `services.omw.environment`), which is the
            intended way to supply API keys and other secrets at runtime.
          '';
        };

        settingsFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = ''
            Path to an omw configuration file (TOML). Mutually exclusive with
            `services.omw.settings`. Environment variables are substituted into the
            file before omw reads it.
          '';
        };

        environment = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = { };
          description = ''
            Environment variables exported at the start of the service script,
            before omw starts. They are also visible to `envsubst` when
            interpolating the configuration, which is the intended way to supply
            API keys and other secrets.
          '';
        };

        environmentFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = "Path to a systemd `EnvironmentFile` for the service.";
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
            User the service runs as. When both `services.omw.user` and
            `services.omw.group` are null, a dynamic user is allocated.
          '';
        };

        group = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = ''
            Group the service runs as. When both `services.omw.user` and
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

        systemd.services.omw = {
          description = "OMW agent runtime";
          wantedBy = [ "multi-user.target" ];
          after = [ "network-online.target" ];
          wants = [ "network-online.target" ];

          path = [
            pkgs.envsubst
            cfg.package
          ];

          script =
            let
              exports = lib.concatMapStringsSep "\n" (
                name: "export ${lib.escapeShellArg name}=${lib.escapeShellArg cfg.environment.${name}}"
              ) (builtins.attrNames cfg.environment);

              args = lib.escapeShellArgs (
                [
                  "--config"
                  "/dev/stdin"
                  cfg.mode
                ]
                ++ cfg.extraArgs
              );

              settings =
                if cfg.settings != null then settingsFormat.generate "omw.toml" cfg.settings else cfg.settingsFile;
            in
            ''
              ${exports}
              envsubst < ${settings} | exec omw ${args}
            '';

          serviceConfig = {
            Type = "simple";
            Restart = "on-failure";
            User = lib.mkIf (cfg.user != null) cfg.user;
            Group = lib.mkIf (cfg.group != null) cfg.group;
            DynamicUser = lib.mkIf (cfg.user == null && cfg.group == null) true;
            EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;
            StateDirectory = lib.mkIf (cfg.stateDir != null) cfg.stateDir;
          };
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
