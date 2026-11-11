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
            # The bench's sandbox (ADR-0013), which its tests exercise, and aerith-player's tests
            # too.
            pkgs.bubblewrap
            # aerith-player's relay links libwayland-client (ADR-0015), found through pkg-config.
            pkgs.pkg-config
          ];
          buildInputs = [ pkgs.wayland ];
          # What aerith-testbed's winit and wgpu load when they run (ADR-0017): libwayland-client,
          # which winit opens itself (and the relay with it, in a build of the whole workspace),
          # libxkbcommon for the keyboard, and the Vulkan loader, which finds the system's drivers.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.wayland
            pkgs.libxkbcommon
            pkgs.vulkan-loader
          ];
          # The mpv aerith-player's tests drive: the same build as the bench's mpv, without the
          # wrapper that adds yt-dlp, which the player never loads.
          AERITH_TEST_MPV = "${pkgs.mpv-unwrapped}/bin/mpv";
        };

        # The fuzzing shell: `nix develop .#fuzz`, for aerith-player's fuzz target (its `fuzz/`
        # folder). cargo-fuzz needs a nightly compiler: one pinned by date, which builds nothing that
        # users run, and only here, so that the default shell and CI do not download it (ADR-0011).
        fuzz = pkgs.mkShell {
          packages = [
            (pkgs.rust-bin.nightly."2026-11-10".minimal)
            pkgs.cargo-fuzz
            pkgs.pkg-config
          ];
          buildInputs = [ pkgs.wayland ];
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
