# 0018. What building the relay changed: the socket read by the relay, no sync objects, mpv kept from the compositor

- Status: Accepted; amends 0015 (what mpv is offered, how its connection is bounded, what wakes)
- Date: 2026-11-11

## Context

[ADR-0015](0015-a-relay-for-mpvs-wayland-surface.md) set how the relay is built before it was
written. Building it, with a fresh review and a fuzz target, found facts the design did not know:

- **wayland-server 0.3.17's Rust backend keeps, without a bound, the file descriptors a client
  sends with requests that take none** (`src/rs/socket.rs`, `in_fds`). The relay's handlers never
  see them. A review's reproduction made Aerith hold 560 descriptors this way.
- **The server answers `wl_display.sync` and `get_registry` itself.** These two requests never
  reach the relay's handlers, so the handlers' counts cannot bound them. 200,000 registries grew
  Aerith's memory by 21 MB.
- **The backend panics on some malformed messages:**
  - a `wl_registry.bind` without an interface name hits an `unreachable!()`, still there on
    wayland-rs master;
  - a missing non-nullable string makes the generated parser `unwrap`, as in `xdg_toplevel.set_title`.

  The fuzz target found both.
- **A file descriptor from mpv can change after the relay checked it.** A memfd sealed against
  writing after the check, or one backed by huge pages, makes the compositor's `mmap` fail. wlroots
  then raises `invalid_fd` on the window's connection.
- **mpv 0.42 opens a Wayland connection for its clipboard before its video output.** That
  connection took `WAYLAND_SOCKET`, which libwayland unsets after one use. Its video output then
  fell back on `WAYLAND_DISPLAY`, which is the person's compositor in a real session.
- **mpv's `wayland` context**, its OpenGL fallback when Vulkan fails, connects a second time.
- **When its connection ends, mpv takes it as its window closed, and quits.**
- **Mesa's Vulkan driver uses sync objects only when the compositor offers them.** It falls back
  on implicit sync, which every driver supports.

## Decision

**The relay reads mpv's socket itself.** It passes what it reads to its Wayland server through a
socket pair of its own. At that socket it counts every request, whatever its object, every
registry and every file descriptor. It ends mpv's connection past a ceiling:
- 4,096 requests or 128 descriptors in one wake-up;
- 28 descriptors with one message;
- 32,768 registries;
- 64 descriptors left that no request took.

It refuses there, before the server sees them, the messages that make wayland-rs panic.

**A shared-memory pool must live on tmpfs.** The relay seals the pool's descriptor against new
seals and against shrinking before it reads the descriptor's seals and size. A pool on any other
file system is refused.

**mpv is offered no sync objects** (`linux-drm-syncobj-v1`). Mesa then uses implicit sync. A sync
object's descriptor that is not one is an error on the window's connection, and the relay cannot
check it. Offering sync objects waits for a way to validate them.

**mpv cannot reach the compositor by itself:**
- its clipboard is off (`--clipboard-backends-clr`), in every mode;
- `WAYLAND_DISPLAY` names a path that cannot exist, so a second connection of mpv's fails rather
  than fall back on the person's compositor.

**Closing the plane ends mpv.** mpv quits when its connection ends, and the player reports
`EngineFailed`. The interface closes the plane only when its window goes.

## Alternatives considered

- **Bounding descriptors and display requests inside wayland-rs**: the right place, but not ours
  to change from here. To propose upstream; the relay's own bound stays until then.
- **Counting the process's open descriptors after each dispatch**: no copy of mpv's bytes, but
  the whole process's count, and a scan of `/proc` at each wake-up.
- **Dropping mpv's `wayland` fallback context**: one connection only, but no picture on machines
  without a Vulkan driver, against the bar's widest reach.
- **Offering sync objects now**: explicit sync on the hardware path, but one bad descriptor from
  mpv closes the window.
- **Playing on without a picture after the plane closes**: would need mpv to ignore the close of
  its window, which it does not offer.

## Consequences

- Each byte mpv sends is copied once more, through the relay's socket pair: a few kilobytes a
  frame.
- At worst, mpv can make Aerith hold about 410 descriptors at a peak:
  - 156 during one wake-up;
  - 64 queued after it;
  - 64 pools;
  - up to 128 dma-bufs waiting in libwayland's outgoing buffer.

  With the usual soft limit of 1,024, the interface keeps about 600.
- The relay is fuzzed by hand: `crates/aerith-player/fuzz`, with a nightly in a shell of its own,
  `.#fuzz`, as [ADR-0011](0011-a-pinned-stable-toolchain.md) allows. Neither `scripts/check.sh` nor
  CI runs it yet.
- **The relay's thread and the interface's wake for each other's events**, since they share the
  window's socket. The relay's thread therefore sleeps while the player is paused only if the
  interface receives nothing either. M1.4b measures it with the testbed.
- Explicit sync, and a check of a sync object's descriptor, are left for when they are needed.
