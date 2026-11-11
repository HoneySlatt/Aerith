# Architecture

This file describes the parts the decisions call for; it is rewritten to the code as each part is
built. Of Aerith's own parts, only the player exists: `aerith-player` drives mpv on Linux, its
picture discarded or shown in a plane of the interface's window, through the player's relay. The
bench, a tool beside Aerith, exists.

## The workspace

One Cargo workspace at the root; each crate lives under `crates/`. The toolchain is pinned in
`rust-toolchain.toml` ([ADR-0011](adr/0011-a-pinned-stable-toolchain.md)). How to build and check it
is in [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## Parts

- **The player** (`aerith-player`): one interface over an engine per platform
  ([ADR-0004](adr/0004-a-player-engine-per-platform.md)). On Linux and Windows the engine is mpv,
  a program apart driven over its IPC ([ADR-0003](adr/0003-mpv-in-its-own-process.md)); on Android,
  Media3 through a thin Kotlin layer; on Apple, AVFoundation fed by FFmpeg. Built for Linux;
  [below](#the-player-on-linux).
- **The video plane's host**: on Linux, the relay in `aerith-player`
  ([ADR-0015](adr/0015-a-relay-for-mpvs-wayland-surface.md)), a small Wayland server for mpv that
  places mpv's buffers in a subsurface of the interface's window ([below](#the-relay)); on
  Windows, mpv's child window under an interface composed through DirectComposition.
- **The core** (Stage 2): addon protocol, accounts, storage, sources, the I/O layer. It needs no
  window and no GPU.
- **The interface** (Stage 3): on uwui ([ADR-0005](adr/0005-the-interface-on-uwui-a-later-client.md)).

## The player on Linux

How `aerith-player` drives mpv ([ADR-0014](adr/0014-the-player-interface-and-mpvs-connection.md)):

```text
caller ── open, play, pause, seek, stop ──▶ inbox ──▶ engine thread ── JSON commands ──▶ mpv
caller ◀── Event (Started, Clock, Report…) ─────────── engine thread ◀── inbox ◀── reader ◀── mpv
```

- `Player::start` creates a socket pair and starts mpv with one end as its standard input, where
  mpv reads its IPC. `player.rs` holds the interface; `mpv.rs` the rest.
- Each command goes to the engine thread's inbox and returns. The engine thread writes it to mpv.
- The reader thread blocks on the socket and puts each of mpv's messages, one JSON value a line,
  in the same inbox. The engine thread turns them into events on the caller's channel.
- Each `open` is numbered; the engine thread matches mpv's number for the playlist entry each
  load makes to it, and the events of a source carry the open's number.
- Both threads sleep until something arrives: a paused or idle player wakes nothing up. While a
  source plays, the engine thread also wakes every 10 s to read the clock again
  ([ADR-0016](adr/0016-the-clock-carries-the-engines-rate.md)).
- When mpv ends, the reader sees the connection close; the engine thread reaps mpv and sends
  `EngineFailed`. When the player is dropped, it closes the connection; mpv quits.

### The relay

With `Video::Plane`, mpv's picture shows in the interface's window
([ADR-0015](adr/0015-a-relay-for-mpvs-wayland-surface.md)). `src/plane.rs` holds the interface
(`Window`, `Plane`); `src/relay/` the rest.

```text
mpv ── Wayland requests ──▶ relay (mpv's socket, counted) ──▶ relay's server (checked) ── issued again ──▶ window's connection ──▶ compositor
mpv ◀── callbacks, feedback, releases, configures ── relay ◀── the relay's own queue ◀── window's connection ◀── compositor
interface ── place, show, hide, close ──▶ Plane ──▶ relay's state (one lock) ──▶ window's connection
```

- `Player::start` joins the window's connection with a queue of the relay's own, from the
  display and surfaces the interface hands over. On it, the relay makes the plane's surface: a
  subsurface of the parent, below the surface the interface named, with an empty input region and
  a viewport of the relay's. It starts hidden.
- mpv gets the other end of a socket pair as `WAYLAND_SOCKET`. `WAYLAND_DISPLAY` names a path that
  cannot exist, so that a second connection of mpv's (its `wayland` context, when Vulkan fails
  after connecting) fails rather than reach the person's compositor; `DISPLAY` is removed.
- The relay reads mpv's socket itself (`src/relay/socket.rs`) and passes what it reads to its
  Wayland server, in pure Rust, through a socket pair of its own; the server offers a list of
  globals and plays `xdg_wm_base` itself. At the socket, the relay counts every request, whatever
  its object (`wl_display`'s included), every registry and every file descriptor, and ends mpv's
  connection past a ceiling: 4,096 requests or 128 files in one wake-up, 28 files with one
  message, 32,768 registries. A wake-up thus handles a bounded part of what mpv sends.
- The relay keeps the state of each of mpv's objects. Each request is checked against what the
  compositor would refuse (`src/relay/check.rs`), and counted against ceilings on what the relay
  holds for mpv: 4,096 objects, 64 shared-memory pools' files, and 64 files that no request took
  (mpv sent them with requests that take none, which the server keeps queued). Then it stays in
  the relay or is issued again on the window's connection. A refused request ends mpv's
  connection with a protocol error, never the window's; the relay tells the engine thread, which
  ends mpv and sends `EngineFailed`.
- At mpv's commit, the relay issues mpv's whole new state on the plane's surface, checked first as
  the compositor will check it, against the size of the buffer the compositor holds, which may be
  one mpv has destroyed since. The surface runs desynchronized, so mpv's frames show without the
  interface drawing. A place, show or hide of the interface's that changes the plane's own state
  while it shows switches it to synchronized, so that the change appears with the interface's
  next commit; once the compositor drew it, the surface runs desynchronized again.
- The relay's thread, `aerith-relay`, sleeps in `poll` on mpv's socket, the socket pair to its
  server, the window's connection and a stop signal. It follows libwayland's protocol for a
  connection read by several threads: prepare, poll, then read or cancel at once, so that the
  interface's thread never waits on it longer than one read; it never holds its lock while it
  sleeps or reads.
- The two threads share the window's socket, so each wakes for what the compositor sends the
  other:
  - the relay's thread wakes for every event on the window's connection, the interface's own
    included: its input, its frame callbacks, its configures. With no mpv and the plane hidden,
    it wakes about once for each round trip of the interface's (`src/relay/tests.rs` counts it);
  - the interface's thread, polling the same socket, wakes for mpv's frame callbacks, presentation
    feedback and buffer releases while a film plays.

  So the relay's thread sleeps while the player is paused or stopped only if the interface
  receives nothing either: under an interface that animates at 240 Hz, it wakes at least as often.
  It also wakes for each request of mpv's, and once after each change of the interface's that mpv
  is told of (a new size, shown, hidden). This is ADR-0015's design, where the relay's queue
  shares the window's connection; M1.4b measures it while a film plays.
- `Plane::close`, or dropping the player, stops the thread, ends mpv's connection, and destroys
  everything the relay made on the window's connection, before it returns. mpv takes the end of
  its connection as its window closed, and quits: the player reports `EngineFailed`. The
  interface closes the plane before it destroys its window.

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
