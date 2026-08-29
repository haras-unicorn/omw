{
  description = "OMW = OpenAI + MCP + WASM";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";

    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";

    crane.url = "github:ipetkov/crane";

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-parts,
      crane,
      ...
    }@inputs:
    let
      makePackages =
        pkgs:
        let
          rust = (inputs.rust-overlay.lib.mkRustBin { } pkgs).stable.latest.default.override {
            extensions = [
              "rustfmt"
              "clippy"
              "rust-analyzer"
              "rust-src"
            ];
          };

          craneLib = crane.mkLib pkgs;

          cargoToml = builtins.fromTOML (builtins.readFile ./src/omw/Cargo.toml);

          src = craneLib.cleanCargoSource self;

          depArgs = {
            inherit src;
            strictDeps = true;
            pname = cargoToml.package.name;
            version = cargoToml.package.version;
            cargoExtraArgs = "-p omw";
          };

          vendor = craneLib.vendorCargoDeps depArgs;

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
                  paths = [ omw-unwrapped ];
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
            vendor
            unwrapped
            package
            ;
        };
    in
    flake-parts.lib.mkFlake { inherit inputs; } {
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

          external = with pkgs; [
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
            markdown-link-check
            cspell
            prettier
            vscode-langservers-extracted
            yaml-language-server
            cargo-edit
          ];

          devScriptText = pkgs.writeText "omw-dev.nu" ''
            def "main" [] {
              dev -h
            }

            def "main run" [] {
              cd (flake-root)
              cargo run --bin omw
            }

            def "main format" [] {
              cd (flake-root)
              prettier --write .
              nixfmt ...(fd '.*\.nix$' . | lines)
              cargo fmt --all
              cargo clippy --fix --allow-dirty
            }

            def "main test" [] {
              if ($env.NIX_BUILD_TOP? | is-empty) {
                cargo clippy --all-features -- -D warnings
                cargo test --all-features
              }
            }

            def "main lint" [] {
              cd (flake-root)
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
                cargo clippy --all-features -- -D warnings
                cargo test --all-features
              }
            }
          '';

          packages = makePackages pkgs;

          devScript = pkgs.writeShellApplication {
            name = "dev";
            runtimeInputs = external ++ [ packages.rust ];
            text = ''nu ${devScriptText} "$@"'';
          };
        in
        {
          devShells = {
            default = pkgs.mkShell {
              packages = external ++ [
                packages.rust
                devScript
              ];
              shellHook = ''
                mkdir -p .cargo
                ln -sf "${packages.vendor}/config.toml" .cargo/config.toml
              '';
            };
          };

          apps =
            let
              packages = makePackages pkgs;

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
              packages = makePackages pkgs;

              docs =
                pkgs.runCommand "omw-docs"
                  {
                    src = self;
                    nativeBuildInputs = [ pkgs.mdbook ];
                  }
                  ''
                    mdbook build -d "$out" "$src/docs"
                  '';
            in
            {
              inherit docs;

              default = packages.package;
              unwrapped = packages.unwrapped;

              omw = packages.package;
              omw-unwrapped = packages.unwrapped;
            };
        };
    };

  nixConfig = {
    extra-substituters = [
      "https://haras.cachix.org"
    ];
    extra-trusted-public-keys = [
      "haras.cachix.org-1:/HIo1JYqOIH1Nwk1EGXhuPPvDW0WekxIbY5CiXUZbYw="
    ];
  };
}
