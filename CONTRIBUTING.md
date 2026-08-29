# Prerequisites

[Nix] is used for managing the development shell.

## Development

To start developing, install [Nix] and run `nix develop .` in the root of the
repository to enter the default development shell of the repository flake.

## Organization

The source code is in the `src` directory.

`omw` is written in [Rust] and thus the flake is organized as a Cargo workspace
containing crates inside the `src` directory.

[Nix]: https://nixos.org
[Rust]: https://rust-lang.org
