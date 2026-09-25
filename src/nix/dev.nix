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

      buildPackage =
        {
          crate,
          program,
          variant,
          format,
        }:
        let
          features = if variant == null then "" else "runtime-${variant}";

          featureArgs = if variant == null then "" else "--features ${features}";

          depArgs = staticDepArgs // {
            cargoExtraArgs = "-p ${crate} ${featureArgs} --target ${staticTarget}";
          };

          suffix = if variant == null then "" else "-${variant}";

          unwrapped = craneLib.buildPackage (
            depArgs
            // {
              cargoArtifacts = craneLib.buildDepsOnly depArgs;
              pname = program;
              meta.mainProgram = program;
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
                  name = program;
                  paths = [
                    omw-unwrapped
                  ];
                  meta.mainProgram = program;
                }
              )
              {
                omw-unwrapped = unwrapped;
              };

          tarball =
            pkgs.runCommand "${program}${suffix}-${system}.tar.gz"
              {
                nativeBuildInputs = [ pkgs.gnutar ];
              }
              ''
                mkdir -p staging
                cp -L "${unwrapped}/bin/${program}" "staging/${program}${suffix}-${system}"
                tar -czf "$out" -C staging "${program}${suffix}-${system}"
              '';
        in
        if format == "unwrapped" then
          unwrapped
        else if format == "wrapped" then
          wrapped
        else if format == "tarball" then
          tarball
        else
          builtins.throw "unknown format ${format}";

      binaries = [
        {
          crate = "omw-cli";
          program = "omw";
        }
        {
          crate = "omw-test";
          program = "omw-test";
        }
      ];

      variants = [
        {
          key = null;
          suffix = "";
        }
        {
          key = "rhai";
          suffix = "-rhai";
        }
        {
          key = "js";
          suffix = "-js";
        }
      ];

      formats = [
        {
          key = "unwrapped";
          suffix = "-unwrapped";
        }
        {
          key = "wrapped";
          suffix = "";
        }
        {
          key = "tarball";
          suffix = "-tarball";
        }
      ];

      buildMatrix =
        {
          filterPackages ? _: true,
          mapPackages ?
            {
              package,
              binary,
              variant,
              format,
              ...
            }:
            {
              name = "${binary.program}${variant.suffix}${format.suffix}";
              value = package;
            },
        }:
        builtins.listToAttrs (
          builtins.map mapPackages (
            builtins.filter filterPackages (
              builtins.map
                (
                  {
                    binary,
                    variant,
                    format,
                  }:
                  {
                    inherit binary variant format;
                    package = buildPackage {
                      crate = binary.crate;
                      program = binary.program;
                      variant = variant.key;
                      format = format.key;
                    };
                  }
                )
                (
                  lib.cartesianProduct {
                    binary = binaries;
                    variant = variants;
                    format = formats;
                  }
                )
            )
          )
        );
    in
    {
      inherit
        rust
        env
        nativeBuildInputs
        shellHook
        staticTarget
        binaries
        variants
        formats
        buildPackage
        buildMatrix
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
      overlay = final: prev: (makePackages final).buildMatrix { };
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

          devScript = pkgs.writeShellApplication {
            name = "dev";
            runtimeInputs = external;
            text = ''nu "$(flake-root)/src/nix/dev.nu" "$@"'';
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

      packages =
        let
          matrix = packages.buildMatrix { };
        in
        matrix
        // {
          omw-config-schema =
            pkgs.runCommand "omw-config-schema.json"
              {
                nativeBuildInputs = [ matrix.omw ];
              }
              ''
                omw schema --output "$out"
              '';

          docs =
            pkgs.runCommand "omw-docs"
              {
                src = self;
                nativeBuildInputs = [ pkgs.mdbook ];
              }
              ''
                mdbook build -d "$out" "$src/docs"
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
            (pkgs.nixosOptionsDoc {
              documentType = "mdbook";
              options = eval.options;
              transformOptions =
                opt:
                opt
                // {
                  visible = opt.visible or true && (builtins.head opt.loc) != "_module";
                  declarations = [ ];
                };
            }).optionsCommonMark;
        };

      apps = packages.buildMatrix {
        filterPackages = { format, ... }: format.key != "tarball";
        mapPackages =
          {
            package,
            binary,
            variant,
            format,
            ...
          }:
          {
            name = "${binary.program}${variant.suffix}${format.suffix}";
            value = {
              type = "app";
              program = lib.getExe package;
              meta.description = "OMW = OpenAI + MCP + WASM";
            };
          };
      };

      checks = packages.buildMatrix {
        filterPackages = { format, ... }: format.key == "unwrapped";
        mapPackages =
          {
            package,
            binary,
            variant,
            ...
          }:
          rec {
            name = "${binary.program}${variant.suffix}-static";
            value =
              pkgs.runCommand name
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
          };
      };
    };
}
