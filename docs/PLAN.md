# Plan

This file says what is being worked on now: the milestones of the current stage, their status, how
each one is checked, and the known debt. Direction beyond this stage is in
[`ROADMAP.md`](ROADMAP.md); decisions are in [`adr/`](adr/README.md). A *stage* is a step of the
roadmap; a *milestone* is a small, verifiable step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- **Design**: done on 2026-11-11, ADRs 0001 to 0009. The facts behind it, with their sources, are
  in [`survey.md`](survey.md); what Aerith asks of uwui is in [`uwui-needs.md`](uwui-needs.md). A
  session in uwui's repository is deciding uwui's platform targets (macOS, iOS, tvOS, Android TV),
  HDR and the video element; its answers are recorded in `uwui-needs.md` when they come.
- **Stage 1, the player on the desktop**
  ([ADR-0003](adr/0003-mpv-in-its-own-process.md)): planned. Next: M1.1.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | The workspace exists and every check runs in CI | Planned |
| [M1.2](#m12--the-rivals-measured-and-the-corpus) | The rivals are measured and the bar's figures set | Planned |
| [M1.3](#m13--mpv-driven-through-aerith-player) | Aerith plays a file through mpv in its own process | Planned |
| [M1.4](#m14--mpv-inside-aeriths-window-on-linux) | mpv's picture sits inside Aerith's window on Linux, under a test interface | Planned |
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

### M1.4 — mpv inside Aerith's window on Linux

- **Goal.** mpv's picture plays inside Aerith's window, under a test interface drawn with wgpu.
- **Delivers.** Aerith's Wayland compositor for mpv: mpv's buffers placed in a subsurface under the
  interface's surface, moved and resized with it; the test interface over it (controls, a
  translucent panel).
- **Done when.** The corpus's SDR clips meet the cadence and sync floors at 60 Hz, 240 Hz and VRR
  on the high tier, inside the window.
- **How to test.** The bench, through `agent-headless` where it can, then on the owner's display.
- **Measured.** Cadence, sync, power and memory against mpv alone.
- **By hand.** The owner watches the clips on the desktop.

### M1.5 — HDR, Dolby Vision and passthrough on Linux

- **Goal.** HDR and Dolby Vision clips reach the owner's monitor as mpv alone shows them; audio
  passes through intact.
- **Delivers.** The colour description carried to the subsurface; the SDR interface over HDR video;
  passthrough checked by capturing what the audio output receives; the lowest laptop measured.
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

- Contribute to mpv the Vulkan render API its maintainers described, so that the video becomes a
  texture of the interface ([ADR-0003](adr/0003-mpv-in-its-own-process.md)).
- The player screen as uwui's screen of reference for glass over moving content.

## Known debt

None yet: no code exists.

Finished stages move to `docs/history.md`, in a few lines each, so that this file holds the current
stage only, under about 2,000 words.
