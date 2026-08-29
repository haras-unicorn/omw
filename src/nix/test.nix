{ self, ... }:

let
  marker-script = name: "omw::host::log(\"info\", \"${name}\")";

  tick-script = ''
    loop {
      omw::host::log("info", "tick");
      let id = omw::host::wait_duration(200);
      let e = omw::host::recv();
    }
  '';

  tests = {
    settings = {
      containers.machine = {
        services.omw = {
          settings = {
            runtime.rhai.kind = "rhai";
            agents = [
              {
                name = "alice";
                runtime = "rhai";
                script = "/etc/brain.rhai";
              }
            ];
          };
        };

        environment.etc."brain.rhai".text = marker-script "omw-settings-marker";
      };
      testScript = ''
        start_all()
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "omw-settings-marker"')
        machine.wait_until_fails("systemctl is-active omw.service")
        machine.succeed("test \"$(systemctl show -p Result --value omw.service)\" = success")
      '';
    };

    settings-file = {
      containers.machine = {
        services.omw = {
          settingsFile = "/etc/omw.toml";
        };

        environment.etc."omw.toml".text = ''
          [runtime.rhai]
          kind = "rhai"

          [[agents]]
          name = "alice"
          runtime = "rhai"
          script = "/etc/brain.rhai"
        '';
        environment.etc."brain.rhai".text = marker-script "omw-settings-file-marker";
      };
      testScript = ''
        start_all()
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "omw-settings-file-marker"')
        machine.wait_until_fails("systemctl is-active omw.service")
        machine.succeed("test \"$(systemctl show -p Result --value omw.service)\" = success")
      '';
    };

    envsubst = {
      containers.machine = {
        services.omw = {
          settings = {
            runtime.rhai.kind = "rhai";
            agents = [
              {
                name = "alice";
                runtime = "rhai";
                script = "$SCRIPT_FROM_ENV";
              }
              {
                name = "bob";
                runtime = "rhai";
                script = "$SCRIPT_FROM_ENVFILE";
              }
            ];
          };
          environment = {
            SCRIPT_FROM_ENV = "/etc/brain.rhai";
          };
          # The `environmentFile` is a path into the machine;drop the env file via
          # `environment.etc` alongside the scripts.

          environmentFile = "/etc/omw.env";
        };

        environment.etc."brain.rhai".text = marker-script "omw-envsubst-env-marker";
        environment.etc."brain-bob.rhai".text = marker-script "omw-envsubst-envfile-marker";
        environment.etc."omw.env".text = "SCRIPT_FROM_ENVFILE=/etc/brain-bob.rhai\n";
      };
      testScript = ''
        start_all()
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "omw-envsubst-env-marker"')
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "omw-envsubst-envfile-marker"')
        machine.wait_until_fails("systemctl is-active omw.service")
        machine.succeed("test \"$(systemctl show -p Result --value omw.service)\" = success")
      '';
    };

    loop = {
      containers.machine = {
        services.omw = {
          settings = {
            runtime.rhai.kind = "rhai";
            agents = [
              {
                name = "alice";
                runtime = "rhai";
                script = "/etc/brain.rhai";
              }
            ];
          };
        };

        environment.etc."brain.rhai".text = tick-script;
      };
      testScript = ''
        start_all()
        machine.wait_for_unit("omw.service")
        machine.wait_until_succeeds('test "$(journalctl -u omw.service --no-pager | grep -c -F "tick")" -ge 3')
        machine.succeed("systemctl stop omw.service")
        machine.wait_until_fails("systemctl is-active omw.service")
      '';
    };

    loop-restarts = {
      containers.machine = {
        services.omw = {
          mode = "loop";
          settings = {
            runtime.rhai.kind = "rhai";
            agents = [
              {
                name = "alice";
                runtime = "rhai";
                script = "/etc/does-not-exist.rhai";
              }
            ];
          };
        };
      };
      testScript = ''
        start_all()
        machine.wait_for_unit("omw.service")
        machine.wait_until_succeeds("test \"$(journalctl -u omw.service --no-pager | grep -c -F 'agent alice failed')\" -ge 3")
        machine.succeed("systemctl stop omw.service")
        machine.wait_until_fails("systemctl is-active omw.service")
      '';
    };

    user-and-state = {
      containers.machine = {
        users.users.omw = {
          isSystemUser = true;
          group = "omw";
        };
        users.groups.omw = { };

        services.omw = {
          user = "omw";
          group = "omw";
          stateDir = "omw";
          settings = {
            runtime.rhai.kind = "rhai";
            agents = [
              {
                name = "alice";
                runtime = "rhai";
                script = "/etc/brain.rhai";
              }
            ];
          };
        };

        environment.etc."brain.rhai".text = tick-script;
      };
      testScript = ''
        start_all()
        machine.wait_for_unit("omw.service")
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "tick"')
        machine.wait_until_succeeds('test "$(stat -c %U /proc/$(systemctl show -p MainPID --value omw.service))" = omw')
        machine.wait_until_succeeds("test -d /var/lib/omw")
        machine.succeed("test \"$(stat -c %U:%G /var/lib/omw)\" = omw:omw")
        machine.succeed("systemctl stop omw.service")
        machine.wait_until_fails("systemctl is-active omw.service")
      '';
    };
  };
in
{
  perSystem =
    {
      lib,
      pkgs,
      system,
      ...
    }:
    {
      checks = lib.concatMapAttrs (name: module: {
        "${name}" = pkgs.testers.runNixOSTest {
          name = "omw-${name}";
          imports = [ module ];
          globalTimeout = 30;
          defaults = { lib, ... }: {
            imports = [ self.nixosModules.default ];
            services.omw = {
              enable = true;
              mode = lib.mkDefault "run";
            };
          };
        };
      }) tests;
    };
}
