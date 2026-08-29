{ inputs, self, ... }:

let
  makePackages =
    pkgs:
    let
      qwen-3-5-600M = pkgs.fetchurl {
        name = "qwen-3-5-600M.gguf";
        url = "https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF/resolve/main/Qwen3.5-0.8B-UD-Q4_K_XL.gguf";
        hash = "sha256-MXfr1nr+RDg3TaGeaQvBuYdW9+D+qSQOG+QEM2FWp7U=";
      };

      rust = (inputs.rust-overlay.lib.mkRustBin { } pkgs).stable.latest.default.override {
        extensions = [
          "rustfmt"
          "clippy"
          "rust-analyzer"
          "rust-src"
        ];
        targets = [ "wasm32-wasip2" ];
      };

      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
        _:
        (inputs.rust-overlay.lib.mkRustBin { } pkgs).stable.latest.default.override {
          targets = [ "wasm32-wasip2" ];
        }
      );

      cargoToml = builtins.fromTOML (builtins.readFile "${self}/src/native/omw/Cargo.toml");

      witFilter = path: _type: builtins.match ".*wit$" path != null;

      witOrCargo = path: type: (witFilter path type) || (craneLib.filterCargoSources path type);

      src = inputs.nixpkgs.lib.cleanSourceWith {
        src = self;
        filter = witOrCargo;
        name = "source";
      };

      env =
        let
          default = {
            ZSTD_SYS_USE_PKG_CONFIG = "1";
          };
        in
        {
          inherit default;

          dev = default // {
            OMW_TEST_WASM_ENGINE_NON_NATIVE = "1";
            OMW_TEST_WASM_RUNTIME_NON_NATIVE = "1";
            OMW_TEST_OPENAI_LLAMACPP = "1";
            OMW_TEST_MCP_EVERYTHING = "1";

            OMW_TEST_OPENAI_LLAMACPP_GGUF = qwen-3-5-600M;
            OMW_TEST_OPENAI_LLAMACPP_MODEL = qwen-3-5-600M.name;
          };

          ci = default // {
            OMW_TEST_WASM_ENGINE_NON_NATIVE = "0";
            OMW_TEST_WASM_RUNTIME_NON_NATIVE = "0";
            OMW_TEST_OPENAI_LLAMACPP = "0";
            OMW_TEST_MCP_EVERYTHING = "0";
          };

          build = default // {
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";

            OMW_TEST_WASM_ENGINE_NON_NATIVE = "0";
            OMW_TEST_WASM_RUNTIME_NON_NATIVE = "0";
            OMW_TEST_OPENAI_LLAMACPP = "0";
            OMW_TEST_MCP_EVERYTHING = "0";
          };
        };

      nativeBuildInputs = [
        pkgs.pkg-config
        pkgs.wasm-tools
      ];

      buildInputs = [
        pkgs.openssl
        pkgs.zstd.dev
      ];

      depArgs = {
        inherit
          src
          nativeBuildInputs
          buildInputs
          ;
        env = env.build;
        strictDeps = true;
        pname = cargoToml.package.name;
        version = cargoToml.package.version;
        cargoExtraArgs = "-p omw";
      };

      vendor = craneLib.vendorCargoDeps depArgs;

      shellHook = ''
        mkdir -p .cargo
        ln -sf "${vendor}/config.toml" .cargo/config.toml
      '';

      unwrapped = craneLib.buildPackage (
        depArgs
        // {
          cargoArtifacts = craneLib.buildDepsOnly depArgs;
          pname = "omw";
          meta.mainProgram = "omw";
        }
      );

      package =
        pkgs.callPackage
          (
            {
              symlinkJoin,
              omw-unwrapped,
            }:
            symlinkJoin {
              name = "omw";
              paths = [
                omw-unwrapped
              ];
              meta.mainProgram = "omw";
            }
          )
          {
            omw-unwrapped = unwrapped;
          };
    in
    {
      inherit
        rust
        env
        nativeBuildInputs
        buildInputs
        shellHook
        unwrapped
        package
        ;
    };
in
{
  systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];

  flake.overlays =
    let
      overlay =
        final: prev:
        let
          packages = makePackages final;
        in
        {
          omw = packages.package;
          omw-unwrapped = packages.unwrapped;
        };
    in
    {
      default = overlay;
      omw = overlay;
    };

  perSystem =
    { pkgs, lib, ... }:
    let
      packages = makePackages pkgs;

      flake-root = pkgs.writeShellApplication {
        name = "flake-root";
        text = ''
          current="$PWD"
          while [[ "$current" != "/" ]]; do
            if [[ -f "$current/flake.nix" ]]; then
              echo "$current"
              exit 0
            fi
            current="$(dirname "$current")"
          done
          echo "no flake.nix found" >&2
          exit 1
        '';
      };

      external =
        with pkgs;
        [
          flake-root
          git
          nushell
          nil
          nixfmt
          markdownlint-cli
          marksman
          mdbook
          taplo
          fd
          delta
          cachix
          release-plz
          docker
          markdown-link-check
          cspell
          prettier
          vscode-langservers-extracted
          yaml-language-server
          cargo-edit
          packages.rust
        ]
        ++ packages.nativeBuildInputs
        ++ packages.buildInputs;

      devScriptText = pkgs.writeText "omw-dev.nu" ''
        def "main" [] {
          dev -h
        }

        def --wrapped "main run" [...args: string] {
          cd (flake-root)
          $in | cargo run --bin omw -- run ...($args)
        }

        def --wrapped "main loop" [...args: string] {
          cd (flake-root)
          $in | cargo run --bin omw -- loop ...($args)
        }

        def --wrapped "main schema" [...args: string] {
          cd (flake-root)
          $in | cargo run --bin omw -- schema ...($args)
        }

        def "main format" [] {
          cd (flake-root)
          open --raw (nix build --no-link --print-out-paths ".#options")
            | save -f "./docs/nixos/options.md"
          open --raw (nix build --no-link --print-out-paths ".#schema")
            | save -f "./assets/schema.json"
          prettier --write .
          nixfmt ...(fd '.*\.nix$' . | lines)
          cargo fmt --all
          cargo clippy --fix --allow-dirty
        }

        def "main test" [] {
          cd (flake-root)
          cargo clippy --all-features -- -D warnings
          cargo test --all-features
        }

        def "main test fast" [] {
          cd (flake-root)
          with-env {
            OMW_TEST_WASM_ENGINE_NON_NATIVE: "0"
            OMW_TEST_WASM_RUNTIME_NON_NATIVE: "0"
            OMW_TEST_OPENAI_LLAMACPP: "0"
            OMW_TEST_MCP_EVERYTHING: "0"
          } {
            cargo clippy --all-features -- -D warnings
            cargo test --all-features
          }
        }

        def --wrapped "main test nixos" [test: string, ...args: string] {
          cd (flake-root)
          (nix build
            $".#checks.(uname | get machine)-linux.($test)"
            ...($args))
        }

        def "main lint" [] {
          cd (flake-root)
          if ((open --raw ./docs/nixos/options.md
            | str trim)
            != (open --raw (nix build --no-link --print-out-paths ".#options")
            | prettier --parser markdown
            | str trim)) {
            print -e "options.md doesn't match generated"
            exit 1
          }
          if ((open --raw ./assets/schema.json
            | str trim)
            != (open --raw (nix build --no-link --print-out-paths ".#schema")
            | prettier --parser json
            | str trim)) {
            print -e "schema.json doesn't match generated"
            exit 1
          }
          prettier --check .
          cspell lint . --no-progress
          nixfmt --check ...(fd '.*\.nix$' . | lines)
          markdownlint --ignore-path .markdownignore .
          if ($env.NIX_BUILD_TOP? | is-empty) {
            (markdown-link-check
              --config .markdown-link-check.json
              --quiet
              ...(fd '.*.md' . | lines))
            (taplo lint
              --schema "https://raw.githubusercontent.com/release-plz/release-plz/refs/tags/release-plz-v0.3.148/.schema/latest.json"
              .release-plz.toml)
          }
          cargo fmt --all -- --check
          cargo clippy --all-features -- -D warnings
          cargo test --all-features
        }
      '';

      devScript = pkgs.writeShellApplication {
        name = "dev";
        runtimeInputs = external;
        text = ''nu ${devScriptText} "$@"'';
      };
    in
    {
      devShells = {
        default = pkgs.mkShell {
          env = packages.env.dev;
          packages = external ++ [ devScript ];
          shellHook = packages.shellHook;
        };

        ci = pkgs.mkShell {
          env = packages.env.ci;
          packages = external ++ [ devScript ];
          shellHook = packages.shellHook;
        };
      };

      apps =
        let
          makeApp = package: description: {
            type = "app";
            program = lib.getExe package;
            meta.description = "OMW = OpenAI + MCP + WASM";
          };

          app = makeApp packages.package "OMW = OpenAI + MCP + WASM";

          unwrapped = makeApp packages.unwrapped "OMW = OpenAI + MCP + WASM (unwrapped)";
        in
        {
          default = app;
          unwrapped = unwrapped;

          omw = app;
          omw-unwrapped = unwrapped;
        };

      packages =
        let
          docs =
            pkgs.runCommand "omw-docs"
              {
                src = self;
                nativeBuildInputs = [ pkgs.mdbook ];
              }
              ''
                mdbook build -d "$out" "$src/docs"
              '';

          schema =
            pkgs.runCommand "omw-schema.json"
              {
                nativeBuildInputs = [ packages.package ];
              }
              ''
                omw schema --output "$out"
              '';

          options =
            let
              eval = lib.evalModules {
                modules = [
                  lib.types.noCheckForDocsModule
                  self.nixosModules.default
                  {
                    _module.args.pkgs = pkgs;
                  }
                ];
              };
            in
            pkgs.nixosOptionsDoc {
              documentType = "mdbook";
              options = eval.options;
              transformOptions =
                opt:
                opt
                // {
                  visible = opt.visible or true && (builtins.head opt.loc) != "_module";
                  declarations = [ ];
                };
            };
        in
        {
          inherit docs schema;

          options = options.optionsCommonMark;

          default = packages.package;
          unwrapped = packages.unwrapped;

          omw = packages.package;
          omw-unwrapped = packages.unwrapped;
        };
    };
}
