{
  inputs,
  self,
  lib,
  ...
}:

let
  makePackages =
    pkgs:
    let
      qwen-3-5-600M = pkgs.fetchurl {
        name = "qwen-3-5-600M.gguf";
        url =
          "https://huggingface.co"
          + "/unsloth/Qwen3.5-0.8B-GGUF"
          + "/resolve/main/Qwen3.5-0.8B-UD-Q4_K_XL.gguf";
        hash = "sha256-MXfr1nr+RDg3TaGeaQvBuYdW9+D+qSQOG+QEM2FWp7U=";
      };

      targets = [
        "wasm32-wasip2"
        "x86_64-unknown-linux-musl"
        "aarch64-unknown-linux-musl"
      ];

      system = pkgs.stdenv.hostPlatform.system;

      muslCc = pkgs.pkgsStatic.stdenv.cc;
      staticTarget =
        if pkgs.stdenv.hostPlatform.system == "aarch64-linux" then
          "aarch64-unknown-linux-musl"
        else
          "x86_64-unknown-linux-musl";
      staticTargetLower = builtins.replaceStrings [ "-" ] [ "_" ] staticTarget;
      staticTargetUpper = inputs.nixpkgs.lib.toUpper staticTargetLower;

      rustBase = inputs.rust-overlay.lib.mkRustBin { } pkgs;

      rust = rustBase.stable.latest.default.override {
        extensions = [
          "rustfmt"
          "clippy"
          "rust-analyzer"
          "rust-src"
        ];
        inherit targets;
      };

      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
        _:
        rustBase.stable.latest.default.override {
          inherit targets;
        }
      );

      nativeBuildInputs = [
        pkgs.pkg-config
        pkgs.wasm-tools
        pkgs.mold
      ];

      cargoToml = builtins.fromTOML (builtins.readFile "${self}/src/bin/omw-cli/Cargo.toml");

      witFilter = path: _type: builtins.match ".*wit$" path != null;

      witOrCargo = path: type: (witFilter path type) || (craneLib.filterCargoSources path type);

      env =
        let
          default = {
            CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
            CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
          };

          extraTests = {
            OMW_TEST_WASM_ENGINE_NON_NATIVE = "1";
            OMW_TEST_WASM_RUNTIME_NON_NATIVE = "1";
            OMW_TEST_OPENAI_LLAMACPP = "1";
            OMW_TEST_MCP_EVERYTHING = "1";

            OMW_TEST_OPENAI_LLAMACPP_GGUF = qwen-3-5-600M;
            OMW_TEST_OPENAI_LLAMACPP_MODEL = qwen-3-5-600M.name;
          };

          noExtraTests = {
            OMW_TEST_WASM_ENGINE_NON_NATIVE = "0";
            OMW_TEST_WASM_RUNTIME_NON_NATIVE = "0";
            OMW_TEST_OPENAI_LLAMACPP = "0";
            OMW_TEST_MCP_EVERYTHING = "0";
          };

          caBundle = {
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          };
        in
        {
          dev = default // extraTests;

          ci = default // noExtraTests;

          build = default // noExtraTests // caBundle;

          static =
            default
            // noExtraTests
            // caBundle
            // {
              CARGO_BUILD_TARGET = staticTarget;
              "CC_${staticTargetLower}" = "${muslCc}/bin/${muslCc.targetPrefix}cc";
              "CARGO_TARGET_${staticTargetUpper}_LINKER" = "${muslCc}/bin/${muslCc.targetPrefix}cc";
            };
        };

      src = inputs.nixpkgs.lib.cleanSourceWith {
        src = self;
        filter = witOrCargo;
        name = "source";
      };

      staticDepArgs = {
        inherit src nativeBuildInputs;
        depsBuildBuild = [ muslCc ];
        env = env.static;
        strictDeps = true;
        pname = cargoToml.package.name;
        version = cargoToml.package.version;
        cargoExtraArgs = "-p omw-cli --target ${staticTarget}";
      };

      depArgs = {
        inherit src nativeBuildInputs;
        env = env.build;
        strictDeps = true;
        pname = cargoToml.package.name;
        version = cargoToml.package.version;
        cargoExtraArgs = "-p omw-cli";
      };

      vendor = craneLib.vendorCargoDeps depArgs;

      shellHook = ''
        mkdir -p .cargo
        ln -sf "${vendor}/config.toml" .cargo/config.toml
      '';

      buildVariant =
        variant:
        let
          features = if variant == null then "" else "runtime-${variant}";

          depArgs =
            staticDepArgs
            // lib.optionalAttrs (variant != null) {
              cargoExtraArgs = "-p omw-cli" + " --features ${features}" + " --target ${staticTarget}";
            };

          suffix = if variant == null then "" else "-${variant}";
        in
        rec {
          unwrapped = craneLib.buildPackage (
            depArgs
            // {
              cargoArtifacts = craneLib.buildDepsOnly depArgs;
              pname = "omw";
              meta.mainProgram = "omw";
            }
          );

          wrapped =
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

          tarball =
            pkgs.runCommand "omw${suffix}-${system}.tar.gz"
              {
                nativeBuildInputs = [ pkgs.gnutar ];
              }
              ''
                mkdir -p staging
                cp -L "${unwrapped}/bin/omw" "staging/omw${suffix}-${system}"
                tar -czf "$out" -C staging "omw${suffix}-${system}"
              '';
        };

      default = buildVariant null;

      rhai = buildVariant "rhai";

      js = buildVariant "js";
    in
    {
      inherit
        rust
        env
        nativeBuildInputs
        shellHook
        staticTarget
        ;

      unwrapped = default.unwrapped;
      package = default.wrapped;
      tarball = default.tarball;

      rhai-unwrapped = rhai.unwrapped;
      rhai-package = rhai.wrapped;
      rhai-tarball = rhai.tarball;

      js-unwrapped = js.unwrapped;
      js-package = js.wrapped;
      js-tarball = js.tarball;
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
          omw-tarball = packages.tarball;

          omw-rhai = packages.rhai-package;
          omw-rhai-unwrapped = packages.rhai-unwrapped;
          omw-rhai-tarball = packages.rhai-tarball;

          omw-js = packages.js-package;
          omw-js-unwrapped = packages.js-unwrapped;
          omw-js-tarball = packages.js-tarball;
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
    in
    {
      devShells =
        let
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
              jq
              delta
              cachix
              cargo-semver-checks
              release-plz
              gh
              docker
              markdown-link-check
              cspell
              prettier
              vscode-langservers-extracted
              yaml-language-server
              cargo-edit
              packages.rust
            ]
            ++ packages.nativeBuildInputs;

          devScriptText = pkgs.writeText "omw-dev.nu" ''
            def "main" [] {
              dev -h
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

            def "main lib example" [example: string] {
              cd (flake-root)
              cargo run --all-features -p omw --example $example
            }

            def --wrapped "main test nixos" [test: string, ...args: string] {
              cd (flake-root)
              (nix build
                $".#checks.(uname | get machine)-linux.($test)"
                ...($args))
            }

            def "main format" [] {
              cd (flake-root)
              for crate in (
                (ls ./src/lib | get name)
                ++ (ls ./src/wasm | get name)
              ) {
                mkdir $"($crate)/wit"
                cp -f ./assets/omw.wit $"($crate)/wit"
              }
              open --raw (nix build --no-link --print-out-paths ".#options")
                | prettier --parser markdown
                | save -f "./docs/deployment/nixos/options.md"
              open --raw (nix build --no-link --print-out-paths ".#schema")
                | prettier --parser json
                | save -f "./assets/schema.json"
              prettier --write .
              nixfmt ...(fd '.*\.nix$' . | lines)
              cargo fmt --all
              cargo clippy --all-features --fix --allow-dirty
            }

            def "main lint" [] {
              cd (flake-root)
              for crate in (
                (ls ./src/lib | get name)
                ++ (ls ./src/wasm | get name)
              ) {
                if ((open --raw ./assets/omw.wit)
                  != (open --raw $"($crate)/wit/omw.wit")) {
                  print -e $"($crate)/wit/omw.wit does not match assets/omw.wit"
                  exit 1
                }
              }
              if ((open --raw ./docs/deployment/nixos/options.md
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
                  --schema ("https://raw.githubusercontent.com"
                    + "/release-plz/release-plz"
                    + "/refs/tags/release-plz-v0.3.148/.schema/latest.json")
                  .release-plz.toml)
              }
              cargo fmt --all -- --check
              cargo clippy --all-features -- -D warnings
              cargo test --all-features
              nix flake check --all-systems --show-trace
            }

            def "main update" [] {
              cd (flake-root)
              nix flake update
              cargo update
            }

            def "main release-pr" [] {
              cd (flake-root)
              setup git credentials
              let repo = $"($env.GITHUB_SERVER_URL)/($env.GITHUB_REPOSITORY)"
              (release-plz release-pr
                --git-token $env.GITHUB_TOKEN
                --repo-url $repo
                --forge github
                -o json)
            }

            def "main release" [] {
              cd (flake-root)
              setup git credentials
              (release-plz release
                --git-token $env.GITHUB_TOKEN
                --forge github
                --token $env.CARGO_REGISTRY_TOKEN
                -o json)
            }

            def "main build" [] {
              cd (flake-root)
              mkdir result
              def "make tarball" [variant?: string] {
                let package = if $variant == null {
                  "omw-tarball"
                } else {
                  $"omw-($variant)-tarball"
                }
                let suffix = if $variant == null { "" } else { $"-($variant)" }
                let build = (nix build
                  --no-link
                  --print-out-paths
                  --show-trace
                  $".#($package)") | str trim
                let name = ("result/omw"
                  + $suffix
                  + "-${pkgs.stdenv.hostPlatform.system}"
                  + ".tar.gz")
                ln -sf $build $name
                return $name
              }
              (gh release upload $env.GITHUB_REF_NAME
                (make tarball)
                (make tarball rhai)
                (make tarball js)
                --clobber)
            }

            def "setup git credentials" [] {
              let json = (gh api graphql
                -f query='query { viewer { name login databaseId } }'
                --jq '.data.viewer')
              let name = $json | jq --raw-output '.name // .login'
              let email = $json
                | jq --raw-output ('"\(.databaseId)+\(.login)'
                    + '@users.noreply.github.com"')
              git config --global user.name $name
              git config --global user.email $email
            }
          '';

          devScript = pkgs.writeShellApplication {
            name = "dev";
            runtimeInputs = external;
            text = ''nu ${devScriptText} "$@"'';
          };
        in
        {
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

          rhai = makeApp packages.rhai-package "OMW = OpenAI + MCP + WASM (rhai)";
          rhai-unwrapped = makeApp packages.rhai-unwrapped "OMW = OpenAI + MCP + WASM (rhai, unwrapped)";

          js = makeApp packages.js-package "OMW = OpenAI + MCP + WASM (js)";
          js-unwrapped = makeApp packages.js-unwrapped "OMW = OpenAI + MCP + WASM (js, unwrapped)";
        in
        {
          default = app;
          unwrapped = unwrapped;

          omw = app;
          omw-unwrapped = unwrapped;

          omw-rhai = rhai;
          omw-rhai-unwrapped = rhai-unwrapped;

          omw-js = js;
          omw-js-unwrapped = js-unwrapped;
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
          tarball = packages.tarball;

          omw = packages.package;
          omw-unwrapped = packages.unwrapped;
          omw-tarball = packages.tarball;

          omw-rhai = packages.rhai-package;
          omw-rhai-unwrapped = packages.rhai-unwrapped;
          omw-rhai-tarball = packages.rhai-tarball;

          omw-js = packages.js-package;
          omw-js-unwrapped = packages.js-unwrapped;
          omw-js-tarball = packages.js-tarball;
        };

      checks =
        let
          staticCheck =
            package:
            pkgs.runCommand "omw-check-${lib.getName package}-static"
              {
                nativeBuildInputs = [ pkgs.file ];
              }
              ''
                bin="${lib.getExe package}"
                echo "checking $bin for static linkage"
                file "$bin" | grep -E "static(-pie)? linked"
                if "${pkgs.glibc}/bin/ldd" "$bin" >/dev/null 2>&1; then
                  echo "$bin is dynamically linked" >&2
                  exit 1
                fi
                touch "$out"
              '';
        in
        {
          static = staticCheck packages.unwrapped;
          static-rhai = staticCheck packages.rhai-unwrapped;
          static-js = staticCheck packages.js-unwrapped;
        };
    };
}
