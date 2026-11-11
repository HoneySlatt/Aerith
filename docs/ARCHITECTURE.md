# Architecture

No code exists yet. This file describes the parts the decisions call for; it is rewritten to the
code as each part is built.

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

## Rules that hold across parts

- Decoders written in C, and JavaScript plugins, never run in the process that holds tokens.
- Every network access goes through the core's I/O layer and its rules.
- The interface's thread never waits on a disk, the network or an engine.
