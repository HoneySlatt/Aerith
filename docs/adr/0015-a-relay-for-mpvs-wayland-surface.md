# 0015. mpv's picture through a relay on the interface's own Wayland connection

- Status: Accepted; amended by 0018 (what mpv is offered, how its connection is bounded, what
  wakes), 0020 (what mpv is offered, what of mpv's state reaches the plane) and 0024 (colour
  management)
- Date: 2026-11-11

## Context

[ADR-0003](0003-mpv-in-its-own-process.md) places mpv's buffers in a subsurface of Aerith's window
on Linux, through a small Wayland compositor of Aerith's. It leaves open how that compositor is
built. Milestone M1.4a builds it. What bears on it, read on 2026-11-11 in the sources named:

- **A subsurface and its parent must live on the same client connection.** mpv's surface must
  therefore be created again on the connection that holds Aerith's window.
- **The interface owns that connection.** uwui's host is winit 0.30 with wgpu 30, as SlimeEngine
  uses them. winit opens the connection through libwayland and hands out its `wl_display*` through
  `raw-window-handle`. uwui's ADR-0235, in SlimeEngine, makes "the small compositor that serves a
  child process" the application's, and the plane "composed by the system by default".
- **A compositor's error is fatal to the whole connection** (`wayland.xml`, `wl_display.error`).
  wlroots raises such errors on values the client chooses: a viewport's size, a dmabuf's
  dimensions, a sync point. A request of mpv's forwarded unchanged could therefore close Aerith's
  window. mpv decodes hostile files.
- **What mpv 0.42 needs** (`video/out/wayland_common.c`):
  - required: `wl_compositor` v4, `xdg_wm_base`, `wl_output` v2 and `wp_viewporter`;
  - everything else is optional.

  Without fifo-v1 it paces itself on frame callbacks, with a timeout of 1.05 refresh. It stops
  drawing when `xdg_toplevel` (v6) says *suspended*. The main surface takes the pointer unless told
  otherwise.
- **Mesa's Vulkan driver** shares mpv's connection (`wsi_common_wayland.c`, Mesa 26.3):
  - hardware drivers send dmabufs, with a sync object when the compositor offers one;
  - software drivers (lavapipe) send shared memory.
- **libwayland-client takes an inherited socket from `WAYLAND_SOCKET`** before it looks at
  `WAYLAND_DISPLAY`.
- **The tools that do this** (Chromium's GPU process, gamescope's nested mode, ChromeOS's
  Sommelier) end the untrusted client's protocol and issue requests of their own, built and
  checked.
- **wayland-rs** 0.31 (MIT, the Smithay project) is the Rust binding that winit itself uses:
  - its client can join a foreign `wl_display*` with an event queue of its own
    (`Backend::from_foreign_display`);
  - its server has a backend in pure Rust that bounds each message to 4096 bytes and 28 file
    descriptors.

  Neither side is fuzzed upstream.

## Decision

The owner's decision of 2026-11-11.

**Aerith's relay ends mpv's Wayland protocol and issues its own requests, on the interface's own
connection.** It is a part of `aerith-player`, on Linux. It joins the connection that holds the
window with a queue of its own (wayland-client, libwayland backend), from the display and parent
surface the interface hands over as `raw-window-handle` values. mpv's main surface becomes a
subsurface of that window.

**mpv speaks to the relay only.** mpv gets one end of a socket pair as `WAYLAND_SOCKET`, with no
socket file. `WAYLAND_DISPLAY` and `DISPLAY` are removed from its environment. The relay's side
facing mpv is a Wayland server in pure Rust (wayland-server without its `system` feature).

**Every request of mpv's is checked and issued again by the relay.**
- The relay keeps the state of each object of mpv's.
- It checks each request against what the compositor would refuse, and issues its own on the
  window's connection.
- A request that fails a check ends mpv's connection with a protocol error, never the window's. mpv
  quits, and the player reports `EngineFailed`.
- Buffers pass as file descriptors and are never mapped by Aerith.
- Frame callbacks and presentation feedback are the real compositor's, passed back to mpv.

**What mpv is offered is listed, not filtered.**
- Offered: the core surface objects, shared memory, dmabuf with its feedback, sync objects,
  presentation time, viewporter, fractional scale and content type. Colour management joins at
  M1.5.
- `xdg_wm_base` v6 is played by the relay: it sends mpv the plane's size, and *suspended* when the
  plane is hidden.
- mpv gets no seat, clipboard, subcompositor, idle inhibitor or decoration. mpv's input region is
  forced empty, so the pointer reaches the interface.
- mpv's connection has a ceiling on objects, file descriptors and queued messages. The relay's
  server side is fuzzed.

**The plane is sized by the relay.**
- The relay owns the viewport that scales mpv's last buffer to the plane's rectangle, so a moving
  or resizing plane follows the interface's frame without waiting for mpv.
- mpv is told the new size once it settles.

## Alternatives considered

- **Forwarding mpv's requests unchanged**: less code, but one bad value from mpv, a bug or an
  exploit, closes Aerith's window. That breaks ADR-0003's "a hostile file in the decoders reaches
  neither the interface nor the tokens".
- **The interface behind an in-process proxy** (the crate `wl-proxy`, 0.1, MIT or Apache-2.0). The
  proxy owns the compositor connection, and the interface and mpv both talk to it.
  - For: the interface's thread sleeps while mpv's frames flow, and the forwarding code is
    generated.
  - Against: every message of the interface (frames at 240 Hz, input) crosses one more thread. The
    crate is ten months old, with one maintainer and no fuzzing, and uwui would have to connect to
    it.
  - It is the fallback if M1.4a measures too many wake-ups of the interface's thread during
    playback.
- **Smithay's compositor handlers**: a framework for compositors that draw their clients, heavier,
  with no release since June 2025.
- **Compositing mpv's buffers into Aerith's own frame**: a full 4K write at each frame, and the
  plane of ADR-0003 and uwui's ADR-0235 lost.
- **Decoding the Wayland wire format ourselves**: no dependency, but each protocol's code generated
  from its XML by Aerith; more code to keep for nothing more.

## Consequences

- Four crates join `aerith-player`, with the owner's agreement:
  - `wayland-client` (with `system`), `wayland-server`, `wayland-backend`, `wayland-protocols`, all
    MIT;
  - `raw-window-handle` 0.6 (MIT, Apache-2.0 or Zlib).

  libwayland-client (MIT, C) is linked; any Wayland interface loads it anyway. A check keeps
  wayland-server off its C backend.
- Each protocol offered to mpv is written by hand, with its checks. Colour management at M1.5 adds
  the largest.
- **The interface's thread wakes for compositor events addressed to mpv while a film plays.** The
  relay's queue shares the window's socket: whichever thread reads it wakes. M1.4a measures it;
  never paused or stopped.
- The interface must close the plane on its own thread before its window goes. Dropping the player
  closes it too.
- A paused or stopped player wakes none of the relay's threads either.
- mpv can still open the compositor's default socket by itself until it runs in a sandbox, already
  debt due before Stage 2 ([PLAN](../PLAN.md#known-debt)).
- Where the video plane sits, above a black root with the interface above it or under a
  transparent root, is measured at M1.4b and decided then with a uwui session. The relay serves
  both.
