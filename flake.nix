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
              # Stremio's shell is free, but its streaming server is not (ADR-0013). It is the only
              # unfree package admitted, and only the bench shell uses it.
              config.allowUnfreePredicate = pkg: nixpkgs.lib.getName pkg == "stremio-linux-shell";
            }
          )
        );
    in
    {
      devShells = forAllSystems (pkgs: rec {
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
            # The bench's sandbox (ADR-0013), which its tests exercise.
            pkgs.bubblewrap
          ];
        };

        # The bench's shell: `nix develop .#bench`. Everything the default shell holds, plus what
        # makes the test corpus (ADR-0012) and the rivals the bench measures (ADR-0013). CI does not
        # load it: the rivals are large and need a display.
        bench = pkgs.mkShell {
          inputsFrom = [ default ];
          packages = [
            # The corpus: FFmpeg with x264 and x265 encodes it, dovi_tool and hdr10plus_tool add the
            # dynamic metadata, mkvtoolnix muxes, jq writes the HDR10+ metadata, curl fetches the
            # clips the corpus cannot make, DejaVu draws the frame counter.
            pkgs.ffmpeg-full
            pkgs.dovi-tool
            pkgs.hdr10plus_tool
            pkgs.mkvtoolnix-cli
            pkgs.jq
            pkgs.curl
            pkgs.dejavu_fonts
            # The rivals, at the versions flake.lock pins.
            pkgs.mpv
            pkgs.kodi-wayland
            pkgs.stremio-linux-shell
            pkgs.jellyfin-desktop
            # The bench's own silent sound server (ADR-0013); its sandbox comes from the default
            # shell.
            pkgs.pipewire
            pkgs.wireplumber
          ];
          # drawtext needs a font file by path.
          AERITH_BENCH_FONT = "${pkgs.dejavu_fonts}/share/fonts/truetype/DejaVuSansMono.ttf";
        };
      });
    };
}
