{ self, ... }:

let
  marker-script = name: "omw::host::log(\"info\", \"${name}\")";

  tick-script = ''
    loop {
      omw::host::log("info", "tick");
      let id = omw::host::wait_for(200);
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

    env-overlay = {
      containers.machine = {
        services.omw = {
          settings = {
            # The container's own RLIMIT_MEMLOCK (8M) cannot be raised from
            # the unit, so mlock fails: opt into unlocked secrets here.
            tunables.allow_unlocked_secrets = true;
            providers.openai = {
              kind = "openai";
              api_key = "from-file";
              model = "gpt-4o";
            };
            runtime.rhai.kind = "rhai";
            agents = [
              {
                name = "alice";
                runtime = "rhai";
                script = "/etc/brain.rhai";
              }
            ];
          };
          environment = {
            OMW__PROVIDERS__OPENAI__API_KEY = "from-env";
          };
          # The `environmentFile` is a path into the machine; drop the env file
          # via `environment.etc` alongside the script.
          environmentFile = "/etc/omw.env";
        };

        environment.etc."brain.rhai".text = marker-script "omw-env-overlay-marker";
        environment.etc."omw.env".text = "OMW__FROM_ENVFILE=yes\n";
      };
      testScript = ''
        start_all()
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "omw-env-overlay-marker"')
        machine.succeed('systemctl show -p Environment --value omw.service | grep -F "OMW__PROVIDERS__OPENAI__API_KEY=from-env"')
        machine.succeed('systemctl show -p EnvironmentFiles --value omw.service | grep -F "/etc/omw.env"')
        machine.wait_until_fails("systemctl is-active omw.service")
        machine.succeed("test \"$(systemctl show -p Result --value omw.service)\" = success")
      '';
    };

    hardening = {
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

        environment.etc."brain.rhai".text = marker-script "omw-hardening-marker";
      };
      testScript = ''
        start_all()
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "omw-hardening-marker"')
        machine.succeed('test "$(systemctl show -p NoNewPrivileges --value omw.service)" = yes')
        machine.succeed('test "$(systemctl show -p ProtectSystem --value omw.service)" = strict')
        machine.succeed('test "$(systemctl show -p ProtectHome --value omw.service)" = yes')
        machine.succeed('test "$(systemctl show -p PrivateIPC --value omw.service)" = yes')
        machine.succeed('test "$(systemctl show -p RestrictSUIDSGID --value omw.service)" = yes')
        machine.wait_until_fails("systemctl is-active omw.service")
        machine.succeed("test \"$(systemctl show -p Result --value omw.service)\" = success")
      '';
    };

    hardening-off = {
      containers.machine = {
        users.groups.omw = { };
        users.users.omw = {
          isSystemUser = true;
          group = "omw";
        };

        services.omw = {
          hardening = false;
          # DynamicUser implies a lot of protections
          user = "omw";
          group = "omw";
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

        environment.etc."brain.rhai".text = marker-script "omw-hardening-off-marker";
      };
      testScript = ''
        start_all()
        machine.wait_until_succeeds('journalctl -u omw.service --no-pager | grep -F "omw-hardening-off-marker"')
        machine.succeed('test "$(systemctl show -p ProtectSystem --value omw.service)" = no')
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
        machine.wait_until_succeeds("test \"$(journalctl -u omw.service --no-pager | grep -c -F 'agent brain failed startup validation')\" -ge 3")
        machine.succeed("systemctl stop omw.service")
        machine.wait_until_fails("systemctl is-active omw.service")
      '';
    };

    watch = {
      containers.machine = {
        services.omw = {
          extraArgs = [ "--watch" ];
          # The watcher rewrites the brain in place, which needs a writable
          # path under `ProtectSystem=strict`.
          readWritePaths = [ "/etc" ];
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
        environment.etc."brain-new.rhai".text = tick-script;
      };
      testScript = ''
        start_all()
        # The service never exits under --watch: `run` stays up while reloads
        # are armed, restarting the script instead of completing.
        machine.wait_for_unit("omw.service")
        machine.wait_until_succeeds('test "$(journalctl -u omw.service --no-pager | grep -c -F "tick")" -ge 2')
        machine.succeed("systemctl is-active omw.service")

        # Rewrite the script: the watcher must restart the agent on the new
        # script, which keeps ticking.
        machine.succeed("rm -f /etc/brain.rhai; cp /etc/brain-new.rhai /etc/brain.rhai")
        machine.wait_until_succeeds('test "$(journalctl -u omw.service --no-pager | grep -c -F "agent run reloaded")" -ge 1')
        machine.wait_until_succeeds('test "$(journalctl -u omw.service --no-pager | grep -c -F "tick")" -ge 4')
        machine.succeed("systemctl is-active omw.service")
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
              variant = lib.mkDefault "rhai";
            };
          };
        };
      }) tests;
    };
}
