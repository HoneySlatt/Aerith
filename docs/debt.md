# Known debt

The known debt of the current stage: what is left undone or unmeasured, why it stays, and where it
is to be paid. It outgrew a page of [`PLAN.md`](PLAN.md), which links here. Each item is paid by
the milestone that touches its code, or triaged at the stage's audit (the project method's
`audits.md`, *Debt*).

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
- The player's report uses mpv's names for codecs, formats and colours. It tells Dolby Vision
  only for the selected video track, and only when mpv reshapes the picture or the container
  declares it: a file whose Dolby Vision metadata mpv cannot read (the corpus's
  `dv7-1080p.ts`) shows as HDR10.
- The relay's automated tests cover a fake compositor and lavapipe's shared memory only. Real
  dma-bufs and a real compositor were exercised by hand on Umbriel and KWin. Its checks follow
  wlroots 0.20, and colour management's KWin's and Umbriel's too: niri (Smithay), the owner's
  compositor since 2026-11-16, may refuse what they accept; M1.4d checks it.
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
- M1.4c's floors were measured mostly under other sessions' load (2 to 15 threads): quiet are
  only the 23.976 fps runs at 240 Hz (and the 24 fps ones but their last seconds), and under VRR
  Aerith's 24 to 50 fps runs and mpv alone's 25 to 50 fps ones. Due before Stage 1's
  audit, by the owner's choice of 2026-11-14: both series again on a quiet machine
  (`BENCH_SECONDS=60 scripts/bench-window.sh <corpus> "<display>" rule`, about 30 min each), and
  the floor's 30 minutes, not taken.
- The relay's probe is tested against the fake compositor only for a window that changes: no
  bench run toggles fullscreen, so KWin's automatic VRR turning on mid-film was not run on the
  desktop. Its thresholds rest on one display at 240 Hz; other rates, 60 Hz at M1.5 first, are
  unmeasured. A VRR turned on with no change of the window or its display goes unseen until the
  next change ([ADR-0021](adr/0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md)).
- The relay's tests with the real mpv run under a headless sway, whose presentations carry no
  hardware times: the switch to display sync is run there only by hand, under KWin's virtual
  backend ([CONTRIBUTING](../CONTRIBUTING.md#the-test-window)).
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
  KWin and the fake compositor all send it; niri is checked at M1.4d.
- On a compositor without colour management (niri, on the desktop and the lowest laptop), mpv dithers to 8 bits
  alone and inside Aerith alike. Where the compositor offers it, the relay passes it on from
  M1.5a, and their frames are the same ([figures](measurements.md#through-the-relay-without-a-screen)).
- The window's thread wakes 52 to 135 times a second while a film plays under audio sync, 280 to
  580 under display sync at 240 Hz, for events addressed to mpv on the socket it shares with the
  relay. `wl-proxy` would spare it ([ADR-0015](adr/0015-a-relay-for-mpvs-wayland-surface.md)). By
  the owner's choice of 2026-11-12, kept, and reopened at Stage 3 with the CPU time of uwui's
  thread during a film.
- mpv's window ready from the start costs 84 MiB while idle on the high tier; the lowest laptop is
  measured at M1.5.
- The relay's thread wakes about 6 times for each frame of a 24 fps film (142 a second) under audio
  sync, and 853 to 1,083 times a second under display sync at 240 Hz (M1.4c), the rule at a fixed
  rate; no breakdown and no budget yet.
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
  CI's minute explores less. From `e3d0612` (M1.4b) to M1.5a, the target's prelude bound fifo as
  `xdg_wm_base`, so every session ended in the prelude: the fuzzing of those milestones reached
  almost none of the relay's handlers. M1.5a fixed it and made a refused prelude fail at once.
- Once, under load, the existing test `an_output_the_compositor_removes_is_removed_for_mpv`
  panicked inside wayland-backend (`client_impl/mod.rs:795`, "NULL user data on object
  wl_output"), as an output went away; 300 runs alone and 100 loaded runs did not repeat it. If
  the race lies in the relay's handling of a removed output, a monitor unplugged during a film
  could panic the relay's thread. M1.4d reproduces and fixes it.
- `scripts/test-check.sh` copies `.git` with tar: in a git worktree, where `.git` is a file, its
  cases stage scratch files in the worktree's real index (seen again at M1.5a).
- `scripts/check.sh` grew from 33 s to over three minutes: `scripts/test-check.sh` runs the whole
  test suite, `at_rest`'s 9 s included, in six of its copies. A lighter way for those copies is the
  owner's call ([figures](measurements.md#the-checks)).
