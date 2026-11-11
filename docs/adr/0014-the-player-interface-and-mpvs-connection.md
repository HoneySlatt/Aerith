# 0014. The player's interface: commands and events; mpv over its standard input

- Status: Accepted
- Date: 2026-11-11

## Context

Milestone M1.3 drives mpv, a program apart ([ADR-0003](0003-mpv-in-its-own-process.md)), through
`aerith-player`, whose contract [ADR-0004](0004-a-player-engine-per-platform.md) sets. What bears on
its shape:

- The interface's thread never waits on an engine ([ARCHITECTURE](../ARCHITECTURE.md)). At rest,
  Aerith wakes nothing up: 0 wake-ups a second of its own ([ROADMAP](../ROADMAP.md#budgets)).
- Tokens never reach a command line or a log ([AGENTS](../../AGENTS.md#rules-the-code-depends-on)).
  A stream's URL and headers often carry one.
- mpv 0.42 offers its JSON IPC two ways: a socket file it creates (`--input-ipc-server`, which the
  bench and Stremio's and Jellyfin's mpv use), or a connection it inherits
  (`--input-ipc-client=fd://N`). In the second, mpv quits when the connection closes.
- One engine exists. The others (Media3, AVFoundation) come at their stages, one per platform.
- The corpus is 11 GB, made from a 740 MB download ([ADR-0012](0012-the-test-corpus.md)); CI's
  budget is 20 minutes a job.

## Decision

**One concrete `Player`, no engine trait.** Each platform has one engine, chosen when Aerith is
built; a trait would have a single implementation. `Player` exists on Linux; Windows adds its
engine at M1.6.

**Commands return at once; what the engine does comes back as events.** `open`, `play`, `pause`,
`seek` and `stop` hand the command to the player's own thread and return. The events (`Started`,
`Paused`, `Clock`, `Report`, `PictureFailed`, `Ended`, `Refused`, `EngineFailed`) arrive on a
standard channel. Two threads drive mpv, both blocked until something arrives: one reads mpv's
messages, one writes the commands and turns mpv's messages into events.

**mpv reads its IPC on its standard input**, one end of a socket pair
(`--input-ipc-client=fd://0`). No socket file exists, that another program could open: mpv's IPC
can run programs. No loop waits for a file to appear. mpv quits by itself when Aerith ends, however
it ends.

**The position is a clock.** mpv is not asked to send the position, which changes at every frame.
The player reads it when its relation to time changes (the first frame, a seek, a pause, a stream
waiting for data) and sends a `Clock`: a position, the instant it was read, and whether it runs.
The interface computes the position at any moment from it.

**What mpv does is read from the properties it is asked to observe**, which mpv sends when they
change: the decoder, the decoded and drawn colours, the sound decoded and sent. They make the
`Report`. When the source has a picture that mpv dropped, because its video output failed, the
player says so (`PictureFailed`).

**Secrets travel over the connection only.** The headers are set (`http-header-fields`) before
each load, emptied for a source without any; mpv writes no log; the `Debug` of a source hides
header values and the URL's query. Only `http` and `https` streams are accepted: mpv knows other
protocols (`av://`, `edl://`) that reach the machine's devices and files.

**mpv runs with nothing of the person's or of its own interface**: no configuration, script, key
binding, terminal or resume file. Its picture is discarded (`--vo=null`) for tests and measures, or
shown through Wayland only (`gpu-next`, `waylandvk` or `wayland`), never by taking the screen
through DRM. Its caches go to a folder Aerith names.

**The tests run in three layers:**

- against a fake mpv on the other end of the socket pair, for every event, error and limit, on
  every system CI runs;
- against the real mpv, on clips mpv's own encoder makes in a few milliseconds, in CI: the
  development shell names it (`AERITH_TEST_MPV`, the same build as the bench's mpv). Each mpv runs in
  bubblewrap, the person's home replaced, sound devices and session out of reach, and each test
  checks that mpv wrote nothing in that home;
- on the whole corpus, by hand, where it exists: the test is ignored unless asked for.

## Alternatives considered

- **A socket file (`--input-ipc-server`)**: what everyone does, but a file any program of the
  person can open, a loop that waits for it, a path limited to 108 bytes, files left behind by a
  crash, and an mpv that outlives a killed Aerith.
- **A request-and-answer interface**, each command waiting for mpv's answer: simpler to call, but
  the interface's thread would wait on mpv.
- **Callbacks instead of a channel**: no thread to forward events to an interface's loop, but the
  caller's code would run on the player's thread and could stall it. A channel is the standard
  library's; a callback can be added when the interface's loop needs one.
- **Observing the position**: no arithmetic for the interface, but a wake-up of Aerith at each
  frame, 24 to 60 a second.
- **Headers as per-file options of `loadfile`**: no option outliving its source, but a
  comma-separated text to escape; a list set as a property is exact.
- **An engine trait now**: ready for the second engine, but one implementation, and the shape of
  the second unknown.
- **The corpus in CI**: the real clips on every change, but 740 MB to fetch and hours to make, or
  a cache to keep, for each run.

## Consequences

- Dropping a player waits up to two seconds for mpv to quit, then kills it: the interface drops it
  off its thread, or at exit.
- An mpv that freezes without exiting is not detected: commands go unanswered. A watchdog needs a
  timer while a command waits; it is recorded as debt.
- The interface's loop needs to wake when an event arrives from another thread
  ([uwui-needs](../uwui-needs.md#general)).
- Codec, format and colour names are mpv's; a second engine will need common ones.
- mpv fetches streams itself, outside the I/O layer of Stage 2: the core must check a stream's host
  against the I/O layer's rules before it opens it.
- On Windows, mpv takes the connection as an inherited named pipe (`handle://`) with overlapped
  I/O; M1.6 builds it.
- CI's shell downloads mpv (752 MiB with what it needs, shared in part with the rest).
- The corpus test, ignored by CI, is run at each milestone that touches playback, and its result
  recorded in [`measurements.md`](../measurements.md).
