# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
{
  # The development shell that agents, the owner and CI use alike: `nix develop`, then
  # `scripts/check.sh`. CONTRIBUTING.md says what each tool is for.
  description = "Aerith: an open-source video client";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Rust's own builds of its toolchains, by version or by date (ADR-0011).
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            # The stable toolchain pinned in rust-toolchain.toml, the one rustup users get too.
            (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
            # Enforces the dependency policy of deny.toml.
            pkgs.cargo-deny
            # scripts/check.sh lists the repository's files with it; cargo deny fetches the
            # advisories with it.
            pkgs.git
            # Checks the links of the documents and of the code's documentation.
            pkgs.lychee
          ];
        };
      });
    };
}
