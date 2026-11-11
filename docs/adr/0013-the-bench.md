# 0013. The bench: a Rust tool that drives each rival's engine, in silence

- Status: Accepted
- Date: 2026-11-11

## Context

The quality bar's figures are set by measuring the rivals first
([ADR-0008](0008-the-quality-bar.md)): mpv alone, Kodi, Stremio and Jellyfin Desktop. The same
bench will measure Aerith at milestones M1.4 and M1.5. The rivals differ (read in their sources on
2026-11-11):

- mpv speaks JSON over a socket and publishes its counters of dropped, delayed and mistimed frames
  and its audio-video difference.
- Kodi speaks JSON-RPC on TCP, which a fresh profile opens on `127.0.0.1:9090`. It publishes no
  frame counter and no sync figure there: they reach only its overlay. Its debug log records the
  frames dropped at output and its audio engine's sync error when it checks or corrects it.
- Stremio's Linux shell and Jellyfin Desktop embed libmpv through the render API in OpenGL (not
  gpu-next). Neither opens mpv's socket nor reads an `mpv.conf`. Each injects a bridge into every
  page its web view opens: Stremio's passes mpv commands, Jellyfin's runs an mpv command line. mpv
  lets `input-ipc-server` be set while it runs.

The owner's machine has a real sound card. The owner decided that the bench plays no sound and
touches neither the sound server nor the card.

## Decision

**The bench is a Rust crate of the workspace, `aerith-bench`**, with one dependency, `serde_json`
(MIT or Apache-2.0): both players speak JSON, and parsing it by hand was the alternative weighed,
more code to test for no gain; M1.3's IPC with mpv needs it too. The owner agreed to it. The crate
is tested by `cargo test` in `scripts/check.sh`, and will measure Aerith through `aerith-player`
later. A script, `scripts/bench-rivals.sh`, runs its set of measures on every rival.

**Each rival is driven through its engine:**

- mpv alone, with `--no-config`, gpu-next, safe hardware decoding and Wayland contexts only, over
  its socket;
- Kodi over JSON-RPC, started with `--debug` for its log of drops and sync, `--windowing=wayland`
  and `--audio-backend=pipewire`;
- Stremio and Jellyfin Desktop open a page the bench writes, in place of their interface. The page
  asks their mpv to open a socket, and the bench drives that mpv as it drives mpv alone. Their own
  options (decoding, sync, sound channels) stay as they ship.

**What each run measures**: the time from the load command to the engine's first-frame marker (mpv's
`playback-restart` event, Kodi's `Player.OnAVStart`), from a file and from HTTP on the loopback
interface; the engine's frame counters and sync each second; the PSS and CPU time of the rival's
whole tree and the GPU's power each second, against ten seconds of idle power; what the engine says
it decodes and outputs. Each rival starts with a fresh profile, fullscreen: its shader caches are empty, so the first frame
includes compiling them, as on a first start. Every result names the
rival's store path, which gives its version, and the date. A run writes `result.json`, versioned by
a `format` field, atomically.

**Every program of a run starts in a bubblewrap sandbox**, the rivals and the bench's sound server
alike: `/dev/snd` is empty, so even listing the cards (Jellyfin Desktop does, through ALSA) opens
nothing; the owner's home is replaced by the run's own, so what a program writes under `$HOME`
(Stremio's streaming server writes `~/.stremio-server`) stays in the run's folder; the clip's
folder is bound back read-only. A rival also gets process numbers of its own, so the kernel ends
every helper it started when it ends or when the bench dies. The sound server cannot: PipeWire
checks each client by its process, which it must see.

**The sound goes to the bench's own PipeWire**, whose only output is a null sink paced by the
system clock, with the system's PipeWire and WirePlumber configuration hidden. Its WirePlumber has
every device disabled; the bench waits until it names the sink the default. The rivals reach it
through the environment; PulseAudio clients are pointed nowhere and JACK clients may not start a
server. The sampler also checks, each second and at the end, that no process of the run holds a
sound device: a run where one does is void, exits with its own code, and the set stops.

**The rivals reach only Wayland**: toolkits are pinned to it, and mpv to its Wayland contexts.
When it could not use Wayland, mpv fell back on DRM and asked the kernel for the screen; the kernel
refused, as the compositor holds it.

## Alternatives considered

- **A shell script with jq and socat**: no compilation and no crate, but four drivers, JSON streams
  and percentiles in shell grow fragile and hard to test, and it would be written again to measure
  Aerith.
- **Python**: quick to write, but a second language in the repository, with its own lints and
  tests.
- **Driving Stremio and Jellyfin through their own interface** (Chromium's debugging protocol, a
  websocket crate): it would include their interface in the time to the first frame. But it needs
  input or a debugging channel per rival, and their mpv's counters would still be out of reach.
- **The owner's real sound output**: the sound card's latency would enter the sync figure, but
  every run would play aloud and need the owner present.
- **mpv's null audio output for everything**: silent, but not every rival can be given it, and
  Kodi has no such option.

## Consequences

- The sync figure is the engine's own estimate without the card's latency. A capture of the
  screen and the sound would measure the real one; the corpus's beat allows it later.
- For Stremio and Jellyfin, the time to the first frame is their engine's, not their interface's.
- Kodi's frame figure is a lower bound, and its sync is read only when its audio engine logs it.
  Its decoder's drops and its renderer's skips are not measured.
- The GPU's power is the whole card's, compositor included: the run's idle reading is subtracted,
  and nothing else may run during a measure.
- Stremio's streaming server is not free software: `flake.nix` admits that one unfree package,
  in the bench's shell only.
- The rivals reach the network as they ship: Kodi checks its add-on repository, Stremio starts its
  streaming server. Cutting the network would also cut the loopback the stream runs use.
- `--mpv-option` refuses the options the bench sets itself (output, sound, IPC, configuration).
- The bench needs the owner's desktop session for real figures (`owner-desktop`); in
  `agent-headless` it runs without a GPU, to check that it works.
