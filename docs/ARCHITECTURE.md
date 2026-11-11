# Architecture

This file describes the parts the decisions call for; it is rewritten to the code as each part is
built. Of Aerith's own parts, only `aerith-player`'s crate exists, and it holds only its
documentation. The bench, a tool beside Aerith, exists.

## The workspace

One Cargo workspace at the root; each crate lives under `crates/`. The toolchain is pinned in
`rust-toolchain.toml` ([ADR-0011](adr/0011-a-pinned-stable-toolchain.md)). How to build and check it
is in [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## Parts

- **The player** (`aerith-player`): one interface over an engine per platform
  ([ADR-0004](adr/0004-a-player-engine-per-platform.md)). On Linux and Windows the engine is mpv,
  a program apart driven over its IPC ([ADR-0003](adr/0003-mpv-in-its-own-process.md)); on Android,
  Media3 through a thin Kotlin layer; on Apple, AVFoundation fed by FFmpeg.
- **The video plane's host**: on Linux, Aerith's own small Wayland compositor that receives mpv's
  buffers and places them in a subsurface under the interface; on Windows, mpv's child window under
  an interface composed through DirectComposition.
- **The core** (Stage 2): addon protocol, accounts, storage, sources, the I/O layer. It needs no
  window and no GPU.
- **The interface** (Stage 3): on uwui ([ADR-0005](adr/0005-the-interface-on-uwui-a-later-client.md)).

## The bench

`aerith-bench` is a program apart, not part of what users run ([ADR-0013](adr/0013-the-bench.md)).
It depends on nothing of Aerith yet; at M1.4 it will measure Aerith through `aerith-player`.

- `run` makes one run: `sandbox` is the bubblewrap namespace every program of the run starts in
  (no sound device, the run's own home), `audio` starts the silent sound server, `rivals` starts a
  rival, `engine` drives the rival's engine over
  `json_stream` (mpv's IPC, Kodi's JSON-RPC), `sampler` reads memory, CPU and GPU power each
  second, `serve` serves the clip over HTTP when the run is a stream, and `figures` turns the
  readings into the figures of `result.json`.
- `scripts/corpus.sh` makes the corpus it plays ([ADR-0012](adr/0012-the-test-corpus.md));
  `scripts/bench-rivals.sh` runs its set of measures on every rival.

## Rules that hold across parts

- Decoders written in C, and JavaScript plugins, never run in the process that holds tokens.
- Every network access goes through the core's I/O layer and its rules.
- The interface's thread never waits on a disk, the network or an engine.
