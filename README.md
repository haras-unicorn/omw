# OMW

<!-- ANCHOR: body -->

OMW = OpenAI + MCP + WASM.

`omw` is an agent runtime.

## Installation

`omw` is packaged as a Nix flake. Run it directly without installing:

```sh
nix run github:haras-unicorn/omw
```

or build the `omw` binary with:

```sh
nix build github:haras-unicorn/omw
```

### Releases

Prebuilt binaries for `x86_64-linux` and `aarch64-linux` are attached to each
[GitHub release] as tarballs containing the `omw` binary.

```sh
curl -L -o omw.tar.gz \
  https://github.com/haras-unicorn/omw/releases/latest/download/omw-x86_64-linux.tar.gz
tar -xzf omw.tar.gz
./omw-x86_64-linux
```

[GitHub release]: https://github.com/haras-unicorn/omw/releases

### NixOS and home-manager

Add the flake as an input and apply its overlay so that `omw` is available in
your system configuration:

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    omw.url = "github:haras-unicorn/omw";
  };

  outputs =
    { nixpkgs, omw, ... }:
    {
      nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
        modules = [
          { nixpkgs.overlays = [ omw.overlays.default ]; }
        ];
      };
    };
}
```

Then add `pkgs.omw` to your packages, either in NixOS:

```nix
{ pkgs, ... }:
{
  environment.systemPackages = [ pkgs.omw ];
}
```

or with home-manager:

```nix
{ pkgs, ... }:
{
  home.packages = [ pkgs.omw ];
}
```

### Binary cache

Builds are cached on the [haras cachix cache]. When the flake is used directly
(for example with `nix run github:haras-unicorn/omw`), the cache is configured
automatically through the flake's `nixConfig`. To use it when the package comes
from an overlay, add the following to your nix configuration:

```nix
{
  nix.settings = {
    substituters = [ "https://haras.cachix.org" ];
    trusted-public-keys = [
      "haras.cachix.org-1:/HIo1JYqOIH1Nwk1EGXhuPPvDW0WekxIbY5CiXUZbYw="
    ];
  };
}
```

[haras cachix cache]: https://app.cachix.org/cache/haras

<!-- ANCHOR_END: body -->

## Documentation

The documentation is available at <https://haras-unicorn.github.io/omw/>.
