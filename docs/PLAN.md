# Plan

This file says what is being worked on now: the milestones of the current stage, their status, how
each one is checked, and the known debt. Direction beyond this stage is in
[`ROADMAP.md`](ROADMAP.md); decisions are in [`adr/`](adr/README.md). A *stage* is a step of the
roadmap; a *milestone* is a small, verifiable step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- **Design**: done on 2026-11-11, ADRs 0001 to 0010. The facts behind it, with their sources, are
  in [`survey.md`](survey.md); what Aerith asks of uwui is in [`uwui-needs.md`](uwui-needs.md). A
  session in uwui's repository is deciding uwui's platform targets (macOS, iOS, tvOS, Android TV),
  HDR and the video element; its answers are recorded in `uwui-needs.md` when they come.
- **Stage 1, the player on the desktop**
  ([ADR-0003](adr/0003-mpv-in-its-own-process.md)): under way. M1.1 done on 2026-11-11: the Cargo
  workspace, the toolchain pinned ([ADR-0011](adr/0011-a-pinned-stable-toolchain.md)), every check
  in CI on Linux, and the Rust checks on Windows. M1.2 under way: the corpus
  ([ADR-0012](adr/0012-the-test-corpus.md)) and the bench ([ADR-0013](adr/0013-the-bench.md)) are
  built and run; mpv alone and part of Kodi are measured on the high tier. Waiting on the owner:
  the rest of the rivals on the desktop, the lowest laptop, and the bar's figures. By the owner's
  choice (2026-11-11), M1.3 went ahead meanwhile. The owner says when the machine is free for
  M1.2's runs. M1.3 done on 2026-11-11
  ([ADR-0014](adr/0014-the-player-interface-and-mpvs-connection.md)), its first frame measured on
  the desktop. That measure found that the bench's first frame for mpv alone includes part of
  mpv's start: M1.2's figures for it are to be taken again once the bench waits. M1.4 designed on
  2026-11-11 ([ADR-0015](adr/0015-a-relay-for-mpvs-wayland-surface.md),
  [ADR-0016](adr/0016-the-clock-carries-the-engines-rate.md),
  [ADR-0017](adr/0017-a-test-interface-on-winit-and-wgpu.md)) and split in two by the owner. M1.4a
  done on 2026-11-12 ([ADR-0018](adr/0018-what-building-the-relay-changed.md)). M1.4b done on
  2026-11-12 ([ADR-0019](adr/0019-the-video-above-a-black-root.md) to
  [ADR-0021](adr/0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md)). M1.4c under way:
  the rule built and tested ([ADR-0022](adr/0022-the-relay-tells-the-display-s-rate-when-the-player-asks.md),
  [ADR-0023](adr/0023-the-bench-measures-aerith-through-its-test-window.md)); its measure on the
  owner's desktop waits for a quiet machine.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | The workspace exists and every check runs in CI | Done |
| [M1.2](#m12--the-rivals-measured-and-the-corpus) | The rivals are measured and the bar's figures set | In progress |
| [M1.3](#m13--mpv-driven-through-aerith-player) | Aerith plays a file through mpv in its own process | Done |
| [M1.4a](#m14a--the-relay-and-the-test-interface) | mpv's picture plays inside Aerith's test window on Linux, through the relay | Done |
| [M1.4b](#m14b--inside-the-window-measured) | The floors measured inside the window; the plane's order and display sync decided | Done |
| [M1.4c](#m14c--display-sync-by-the-displays-rate) | The cadence floor holds inside the window, display sync chosen by the display's rate | In progress |
| [M1.5](#m15--hdr-dolby-vision-and-passthrough-on-linux) | HDR, Dolby Vision and passthrough hold on Linux, on both tiers | Planned |
| [M1.6](#m16--windows) | The same on Windows | Planned |

## Stage 1 — The player on the desktop

Aerith plays any file or stream on Linux and Windows with mpv's image and sound, its picture inside
Aerith's window under a test interface, measured against the rivals. Planned by
[ADR-0003](adr/0003-mpv-in-its-own-process.md) and [ADR-0009](adr/0009-the-order-of-the-stages.md).

### M1.1 — The workspace and its checks

- **Goal.** Any agent can build, check and test Aerith with one command, and CI runs it.
- **Delivers.** A Cargo workspace with one crate, `aerith-player`, holding only its documentation;
  the toolchain pinned; `flake.nix` with every tool the checks use; `scripts/check.sh` (format,
  lints as errors, tests, documentation, `cargo deny` with `deny.toml` allowing only licences
  compatible with MPL-2.0, links); CI on GitHub running it; `CONTRIBUTING.md`.
- **Done when.** `nix develop -c scripts/check.sh` passes locally and CI is green.
- **How to test.** `nix develop -c scripts/check.sh`.
- **Measured.** The check's time, warm and cold; CI's job time (under 20 min).
- **Result.** Done on 2026-11-11, CI green; with it a check of every file's MPL-2.0 header, of
  `AGENTS.md`'s length, `scripts/test-check.sh` proving that each check fails when it should, and
  a Windows job ([figures](measurements.md#m11--the-workspace-and-its-checks)).

### M1.2 — The rivals measured and the corpus

- **Goal.** Each budget of the quality bar has a figure, set from the rivals.
- **Delivers.** The test corpus decided (an ADR): clips Aerith generates where possible (SDR,
  HDR10, HLG, HDR10+ and Dolby Vision profiles 5, 7 and 8 metadata, 23.976 to 60 fps, E-AC3,
  TrueHD, DTS, DTS-HD), each with its source and licence. A bench script that plays the corpus in
  mpv alone, Kodi, Stremio and Jellyfin Desktop on the high tier and the lowest laptop, and records
  cadence, sync, press-to-first-frame, memory and power. The figures in `docs/measurements.md`; the
  ROADMAP's budgets updated.
- **Done when.** Each budget of the ROADMAP has its figure, and the owner has seen them.
- **How to test.** The bench script, documented in `CONTRIBUTING.md`.
- **Measured.** Every rival figure, with its version, settings and date.
- **By hand.** The owner runs the lowest laptop's measures if the agent cannot reach it.
- **Where it stands** (2026-11-11). Built and run: `scripts/corpus.sh`, whose corpus passes its own
  checks; `aerith-bench` and `scripts/bench-rivals.sh`, tried on every rival in
  `agent-headless` and on the owner's desktop. Measured on the high tier, by the `short` set: mpv
  alone's 25 runs and Kodi's first 7, beside other sessions' builds
  ([figures](measurements.md#m12--the-rivals-on-the-high-tier)). The owner then needed the machine
  and the set was stopped. Left, each on the owner's go:
  1. on the desktop, with nothing else running:
     `owner-desktop -- nix develop .#bench -c scripts/bench-rivals.sh short ~/.cache/aerith-bench/corpus "<screen>" kodi stremio jellyfin`,
     then the `desktop` set (10 minutes of cadence per rate; the floor's 30 minutes is the
     owner's call), and mpv's three HDR10 first-frame runs again on an idle machine;
  2. cadence at 60 Hz, and with VRR if the owner wants it: the owner switches the monitor's mode;
  3. the lowest laptop, by the owner ([CONTRIBUTING](../CONTRIBUTING.md#on-the-lowest-laptop));
  4. the bar's figures, set from all of these with the owner: the power margin, the image
     tolerance, the first frame's lead.

### M1.3 — mpv driven through `aerith-player`

- **Goal.** Aerith opens, plays, pauses, seeks and stops a file or a stream through mpv, and reports
  what mpv actually did.
- **Delivers.** mpv started as a program apart, without the person's configuration or scripts,
  driven over its IPC; the `aerith-player` interface ([ADR-0004](adr/0004-a-player-engine-per-platform.md));
  headers for streams kept out of the command line; tests against null outputs.
- **Done when.** The tests pass on the corpus with null audio and video; a crash of mpv is reported,
  never a crash of Aerith.
- **How to test.** `cargo test -p aerith-player`.
- **Measured.** Press-to-first-frame against M1.2's figure; at rest, no wake-up of Aerith's own.
- **Result.** Done on 2026-11-11 ([ADR-0014](adr/0014-the-player-interface-and-mpvs-connection.md),
  [figures](measurements.md#m13--mpv-driven-through-aerith-player)): files and `http(s)` streams
  played, paused, sought and stopped, no secret on a command line, 0 wake-ups at rest, asserted.
  Its first frame was slower than mpv alone's; M1.4a's window ready before Play brought it to
  about 70 ms.

### M1.4a — The relay and the test interface

- **Goal.** mpv's picture plays inside Aerith's test window on Linux, through Aerith's relay, with
  mpv's window and video output ready before Play.
- **Delivers.**
  - The relay in `aerith-player` ([ADR-0015](adr/0015-a-relay-for-mpvs-wayland-surface.md)): mpv's
    requests checked and issued again on the interface's connection, a plane the interface places,
    sizes and hides, mpv started with its window ready.
  - The clock with its rate, read again every 10 s while playing
    ([ADR-0016](adr/0016-the-clock-carries-the-engines-rate.md)).
  - `aerith-testbed` ([ADR-0017](adr/0017-a-test-interface-on-winit-and-wgpu.md)): controls and a
    translucent panel over the video, in either order of the surfaces.
  - The relay counts what the compositor presented of mpv's frames, for M1.4b's cadence.
- **Done when.** The tests pass: the relay against a fake mpv, where a bad request ends mpv's
  connection and never the window's; mpv through the relay in `agent-headless`; the fuzz target.
  The corpus's SDR clips play inside the testbed on the owner's desktop. The first frame is
  measured against mpv alone settled.
- **How to test.** `cargo test`; the testbed in `agent-headless --sway`, with lavapipe; on the
  owner's display, briefly, on the owner's go.
- **Measured.** The first frame; at rest, 0 wake-ups of the player's and the relay's threads; the
  interface thread's and the relay's wake-ups a second while a film plays; mpv's memory while idle
  with its window ready; the check's and CI's time before and after.
- **Result.** Done on 2026-11-12: the clock
  ([ADR-0016](adr/0016-the-clock-carries-the-engines-rate.md)); the relay, reviewed, corrected and
  fuzzed ([ADR-0018](adr/0018-what-building-the-relay-changed.md)); the testbed
  ([ADR-0017](adr/0017-a-test-interface-on-winit-and-wgpu.md)). On the owner's display, the
  corpus's SDR clips played inside the testbed, the first frame in about 70 ms with caches kept
  ([figures](measurements.md#inside-the-testbed-on-the-desktop)).

### M1.4b — Inside the window, measured

- **Goal.** The cadence and sync floors hold inside the window; the order of the surfaces and
  display sync are decided with figures.
- **Delivers.**
  - The bench measuring Aerith through the testbed.
  - Before the orders are compared: the relay passes mpv's opaque region, or the reason Umbriel
    never scanned the video out is found (mpv's buffer format on RADV first).
  - The image floor: Aerith's frames compared with mpv alone's.
  - The two orders of the surfaces, the video above a black root with the interface above it, or
    under a transparent root: GPU power against mpv alone, whether Umbriel scans the video out,
    hitches over 60 toggles of the controls. The owner decides with these and a uwui session.
  - Display sync, mpv's default audio sync against `display-resample`, at 240 Hz and with VRR, on
    23.976, 25 and 59.94 fps: presents a second, corrections, power. The owner decides the rule
    with these.
- **Done when.** The floors are measured at 240 Hz and VRR on the high tier, inside the window,
  against mpv alone; the two decisions are ADRs. By the owner's choices of 2026-11-12: 60 Hz moved
  to M1.5, on the lowest laptop, as the desktop's mode comes from its NixOS configuration, which
  Umbriel cannot change while it runs; meeting the cadence floor moved to M1.4c, which builds the
  rule of display sync decided here.
- **How to test.** The bench, on the owner's display.
- **Measured.** Cadence, sync, power and memory against mpv alone.
- **By hand.** The owner watches the clips, and switches the monitor to 60 Hz and to VRR.
- **Result.** Done on 2026-11-12, all at 240 Hz and under VRR on the high tier
  ([figures](measurements.md#m14b--inside-the-window-measured)):
  - the relay passes mpv's opaque region, so the compositor can scan the video out, and offers
    mpv the compositor's fifo ([ADR-0020](adr/0020-what-measuring-the-relay-changed.md)); it tells
    mpv its outputs only after its scale and first commit, which KWin needed;
  - the bench measures Aerith through the testbed, and reads the compositor's presentation
    feedback for mpv alone and Aerith alike (`scripts/bench-window.sh`);
  - Aerith about matches mpv alone in cadence, sync and power, on Umbriel and KWin, its tree 5 to
    7 MiB larger; its picture differs by one step of 8-bit dithering, until the relay offers
    colour management (M1.5);
  - the cadence floor is not met under mpv's default audio sync at a fixed rate, in mpv alone as
    in Aerith; display sync meets it there and defeats VRR;
  - the owner's decisions: the video above a black root
    ([ADR-0019](adr/0019-the-video-above-a-black-root.md)), to be confirmed in a uwui session;
    display sync at a fixed rate and audio sync under VRR
    ([ADR-0021](adr/0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md)), built at M1.4c;
  - M1.4a's open questions answered: ADR-0018 validated; `rustix` kept as a direct dependency of
    the player; a minute of fuzzing in CI; the window thread's wake-ups kept until Stage 3;
  - the closing review's findings fixed, or recorded below.

### M1.4c — Display sync by the display's rate

- **Goal.** The cadence floor holds inside the window, at a fixed rate and under VRR, by the rule
  of [ADR-0021](adr/0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md).
- **Delivers.** The relay telling a fixed rate from VRR by the presentation times of mpv's frames;
  the player switching mpv's `video-sync` when the answer changes, under audio sync until it is
  known; tests against a fake compositor that presents at whole refreshes and between them.
- **Done when.** The corpus's SDR clips meet the cadence and sync floors at 240 Hz and under VRR
  on the high tier, inside the window, on a quiet machine.
- **How to test.** `cargo test`; `scripts/bench-window.sh` on the owner's display.
- **Measured.** Cadence, sync, power and wake-ups against mpv alone under each sync; how long the
  detection takes; the floor's 30 minutes, if the owner wants them.
- **By hand.** The owner turns VRR on and off.
- **Where it stands** (2026-11-14). Built, tested and checked
  ([ADR-0022](adr/0022-the-relay-tells-the-display-s-rate-when-the-player-asks.md)): the relay's
  probe (`src/relay/rate.rs`), its thresholds set from M1.4b's logs
  ([figures](measurements.md#the-detection-calibrated-on-m14bs-logs)); the engine thread
  switching `video-sync`; tests against the fake compositor at whole refreshes, between them, and
  with VRR turning on at fullscreen; the bench reading mpv's `display-sync-active`, and its `rule`
  part. Run under KWin's virtual backend: a fixed rate told in about 1.3 s, then display sync. The
  owner's go of 2026-11-14 amended ADR-0013 by
  [ADR-0023](adr/0023-the-bench-measures-aerith-through-its-test-window.md). Left: on a quiet
  machine, by the owner's choice of 2026-11-14, `BENCH_SECONDS=60 scripts/bench-window.sh
  <corpus> "<display>" rule` at 240 Hz (about 30 min), then under VRR (about 30 min); the floor's
  30 minutes, if the owner wants them; the closing review.

### M1.5 — HDR, Dolby Vision and passthrough on Linux

- **Goal.** HDR and Dolby Vision clips reach the owner's monitor as mpv alone shows them; audio
  passes through intact.
- **Delivers.** The colour description carried to the subsurface; the SDR interface over HDR video;
  passthrough checked by capturing what the audio output receives; the lowest laptop measured,
  cadence and display sync at 60 Hz there included (from M1.4b).
- **Done when.** Every floor of the bench holds on both tiers on Linux.
- **How to test.** The bench; the bitstream comparison script.
- **Measured.** All of Stage 1's budgets on Linux.
- **By hand.** The owner compares the HDR clips with mpv alone on the HDR monitor.

### M1.6 — Windows

- **Goal.** The same on Windows.
- **Delivers.** The Windows montage decided (an ADR) and built; the bench on Windows installed on a
  disk of its own.
- **Done when.** Every floor of the bench holds on Windows on the high tier.
- **How to test.** The bench on Windows, run by the owner or with the owner's go.
- **Measured.** All of Stage 1's budgets on Windows.
- **By hand.** The owner runs Windows; agents never start the virtual machines.

## Ideas

- Measure sync for real: capture the screen's beat with a photodiode or a camera, and the sound
  through a loopback, against the corpus's 20 ms beeps.

- Contribute to mpv the Vulkan render API its maintainers described, so that the video becomes a
  texture of the interface ([ADR-0003](adr/0003-mpv-in-its-own-process.md)).
- The player screen as uwui's screen of reference for glass over moving content.
- The logo drawn again as a true vector, the same flower, without the raster's stray specks: crisp
  at every size and a fraction of its 765 KB ([ADR-0010](adr/0010-the-logo.md)).

## Known debt

- The bench loads a clip as soon as the rival's engine answers: its first frame for mpv alone
  includes part of mpv's start ([figures](measurements.md#against-m12-on-the-desktop)). The bench
  is to wait until the engine has settled, and M1.2's first frames to be taken again; it changes
  M1.2's method, on the owner's go.
- The bench's sync figure is each engine's own estimate, without the sound card's latency; a
  capture of the corpus's beat would measure the real one ([ADR-0013](adr/0013-the-bench.md)).
- Kodi's frame figure is a lower bound (its log's output drops only), and its sync is read only
  when its audio engine logs it.
- For Stremio and Jellyfin Desktop, the time to the first frame is their engine's, not their
  interface's: the bench loads the clip itself.
- E-AC3 with Atmos has no source in the corpus ([ADR-0012](adr/0012-the-test-corpus.md)).
- An mpv that freezes without exiting is not detected: its commands go unanswered
  ([ADR-0014](adr/0014-the-player-interface-and-mpvs-connection.md)).
- mpv fetches streams itself, outside the I/O layer, and FFmpeg sends the headers it was given,
  but `Authorization`, to the hosts a server redirects to: Stage 2 checks a stream's host, and its
  redirects, against the I/O layer's rules before giving it a token.
- mpv runs as the person, with the session's bus (through which the keyring answers) and the home
  in reach: the separate process isolates crashes, not tokens. Due before Stage 2 stores a token:
  run mpv in a sandbox, as its tests do, with no bus, no home but its cache, and the network only
  for its stream ([ADR-0014](adr/0014-the-player-interface-and-mpvs-connection.md)).
- `reap` polls every 10 ms, up to 2 s, when mpv ends or is closed; a crash or a close only.
- A source that is a playlist is stopped, but mpv may start fetching its first entry, without the
  source's headers, before the stop arrives
  ([ADR-0014](adr/0014-the-player-interface-and-mpvs-connection.md)).
- The player's report uses mpv's names for codecs, formats and colours, and says nothing of Dolby
  Vision yet (M1.5).
- The relay's automated tests cover a fake compositor and lavapipe's shared memory only. Real
  dma-bufs and a real compositor were exercised by hand, a few minutes, on Umbriel only. Its
  checks follow wlroots 0.20: other compositors may refuse what wlroots accepts.
- `tests/plane.rs`, the relay with the real mpv, runs only with `AERITH_TEST_WAYLAND=1`: it needs
  lavapipe, and Mesa is 1.12 GB to add to every shell and CI job.
- The relay's fuzz target runs a minute in CI, in a job of its own, by the owner's choice of
  2026-11-12; not in `scripts/check.sh`.
- A hostile mpv can make Aerith hold about 410 file descriptors at a peak, and wayland-rs panics
  on two malformed messages that the relay refuses first: to raise upstream
  ([ADR-0018](adr/0018-what-building-the-relay-changed.md)).
- `aerith-testbed` is removed once Stage 3's interface on uwui plays a film
  ([ADR-0017](adr/0017-a-test-interface-on-winit-and-wgpu.md)). It pins winit to `=0.30.13`, whose
  viewport on the window's surface it relies on.
- winit's `wayland-dlopen`, which the testbed asks for, makes every workspace build load
  libwayland-client at run time, the player's included: the development shell sets
  `LD_LIBRARY_PATH` for it, and for libxkbcommon and the Vulkan loader.
- Under mpv's default audio sync at a fixed rate, a film whose rate is near a whole fraction of
  the display's misses the cadence floor (23.976 and 59.94 fps at 240 Hz), in mpv alone as in
  Aerith, with fifo or without. ADR-0021's rule mends it, built at M1.4c.
- The bench reads mpv alone's presentation from mpv's own feedback: most frames without fifo, one
  in two under it, so mpv alone's cadence is not measured on a compositor with fifo. The relay's
  count, for Aerith, has every frame. Every M1.4b figure includes `WAYLAND_DEBUG=client`'s cost,
  about 1 KB written a frame, not measured; each configuration was measured once.
- The bench's frames compared are mpv's own drawing, again, at its window's size: they cannot show
  what the compositor does to the plane (the relay's viewport, the opaque region).
- Umbriel scans out the order above for most frames, not all (301 of 1,385 in one run); why is not
  known.
- A compositor that offers fractional scaling but never sends a subsurface's preferred scale would
  leave mpv never told its outputs, and so the display's rate
  ([`surface.rs`](../crates/aerith-player/src/relay/surface.rs), `fractional_scale_owed`). Umbriel,
  KWin and the fake compositor all send it.
- Without colour management, which the relay offers from M1.5, mpv inside Aerith dithers to 8
  bits, and mpv alone does not: their frames differ by one step of 8 bits.
- The window's thread wakes 52 to 135 times a second while a film plays under audio sync, 280 to
  580 under display sync at 240 Hz, for events addressed to mpv on the socket it shares with the
  relay. `wl-proxy` would spare it ([ADR-0015](adr/0015-a-relay-for-mpvs-wayland-surface.md)). By
  the owner's choice of 2026-11-12, kept, and reopened at Stage 3 with the CPU time of uwui's
  thread during a film.
- mpv's window ready from the start costs 84 MiB while idle on the high tier; the lowest laptop is
  measured at M1.5.
- The relay's thread wakes about 6 times for each frame of a 24 fps film (142 a second) under audio
  sync, and 926 to 1,422 times a second under display sync at 240 Hz, the rule at a fixed rate
  since M1.4c; no breakdown and no budget yet.
- ADR-0015's "mpv is told the new size once it settles" is built without a timer: one size in
  flight, the latest after it. An animated resize thus makes mpv draw at several sizes in turn;
  morphs at Stage 3 may want a settle delay.
- The relay bounds each wake-up in requests, not in bytes sent on the window's connection. Before
  libwayland 1.23, a full buffer is a fatal error of the whole connection; the relay assumes 1.23 or
  later, the version the system's winit loads.
- No picture when mpv's Vulkan context fails after it connected, or when the compositor's
  linux-dmabuf is older than version 4: mpv's OpenGL fallback cannot open a second connection
  ([ADR-0018](adr/0018-what-building-the-relay-changed.md)).
- The 600 s run of the fuzz target explored inputs of 198 bytes at most, about a dozen messages;
  CI's minute explores less.
- `scripts/test-check.sh` copies `.git` with tar: in a git worktree, where `.git` is a file, its
  cases stage scratch files in the worktree's own index.
- `scripts/check.sh` grew from 33 s to over three minutes: `scripts/test-check.sh` runs the whole
  test suite, `at_rest`'s 9 s included, in six of its copies. A lighter way for those copies is the
  owner's call ([figures](measurements.md#the-checks)).

- This file holds about 3,400 words, over the 2,000 it aims at: the debt above is most of it.

Finished stages move to `docs/history.md`, in a few lines each, so that this file holds the current
stage only, under about 2,000 words.
