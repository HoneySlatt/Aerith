# Roadmap

Aerith's direction: what it is for, its quality bar, its decisions, and its stages. The current work
is in [`PLAN.md`](PLAN.md); each decision is an ADR in [`adr/`](adr/README.md). Stages are merged,
reordered or dropped as the project learns; each such change is an ADR.

## 1. Vision

A video client for films and series, open source, at the level of the best of its field combined:
the reach of Stremio and Nuvio, the image and sound of mpv and Kodi, the polish of Infuse and the
Apple TV app ([ADR-0001](adr/0001-what-aerith-is.md)). It reads Stremio addons, the person's own
files on a disk or a NAS, and Jellyfin servers. It syncs with a Nuvio account, with Trakt and with
a sync of its own. It runs on Linux, Windows, macOS, iPhone, Apple TV, Android phones and Android
TV ([ADR-0006](adr/0006-seven-platforms.md)), as good at a desk with a mouse as on a couch with a
remote. Its interface, on uwui, has an identity of its own: glass, shapes that morph, springs,
drawn at 240 Hz and still at rest.

## 2. Principles

- The picture and the sound come first: nothing the interface, the network or a plugin does may drop
  a frame or break the sync.
- Aerith states what it observed, never what a file name claims: "Dolby Vision" or "Atmos" is shown
  only when the engine reports it.
- The person's data and secrets are safe: tokens in the system's keyring, writes atomic, formats
  versioned; decoders and plugins never reach Aerith's tokens.
- Nothing runs, polls or draws while there is nothing to do.
- Every feature works on the lowest tier; only detail drops.
- Everything is measured; a budget missed is debt until it is met.

## Quality bar

Accepted by the owner on 2026-11-11 ([ADR-0008](adr/0008-the-quality-bar.md)). Its figures are set
by measuring the rivals first, at M1.2; until the owner accepts them, the budgets below are
starting points.

### How the bar works

- A **floor** is an absolute figure. It gates the milestone that measures it.
- A **lead** is a margin over the best rival, measured. A lead missed is debt, never a gate. Its
  figure is set once the rival is measured; the rival's version is frozen until the next stage
  audit, which measures it again.
- Each rival's figure names its version, its settings and its date.

### References

| Area | Reference | What it is best at |
|---|---|---|
| Image | mpv (gpu-next, libplacebo) | Scaling, tone mapping, Dolby Vision, debanding, display sync |
| Playback on a TV | Kodi | Passthrough, the TV's mode matched to the content, a 10-foot interface |
| Apple platforms | Infuse | Dolby Vision and Atmos on Apple TV, artwork, library, polish |
| Couch interface and motion | The Apple TV app | Focus that lifts and grows, edge-to-edge artwork, fluid motion, legible from across a room |
| Addons and reach | Stremio, Nuvio | Catalogues and streams from addons, accounts, many platforms |
| Library on a server | Jellyfin Desktop | Direct play, passthrough, a server's library |
| Watching together | Syncplay, Jellyfin SyncPlay | Clock offset and drift correction across players |

### Hardware

| Tier | Machine | State |
|---|---|---|
| Desktop, high | The owner's desktop: 8 cores (Ryzen 7 5800X), an RX 6900 XT class GPU, 32 GB, 3840 × 2160 at 240 Hz, HDR monitor; Umbriel on wlroots 0.20. The monitor can do VRR, but the owner turned it off: it flickers. A VRR measure is taken only when the owner turns it on for it | measurable |
| Desktop, lowest | The owner's HP Laptop 17-ca2xxx: AMD Athlon Silver 3050U (2 cores), integrated Vega, 4 GB on one channel; measured on mains power | measurable |
| Phone, high | The owner's Pixel (model recorded with the first measure) | measurable |
| Phone, lowest | The owner's mid-range Android phone (model recorded with the first measure) | measurable |
| Apple TV | The owner's Apple TV (model recorded with the first measure) | measurable |
| Android TV | A box to name and acquire before Stage 6 | unmeasured |
| Mac, iPhone | An Apple-silicon Mac and an iPhone, to acquire before Stage 7 | unmeasured |
| Windows | The owner's desktop, on Windows installed on a disk of its own; named at Stage 1 | unmeasured |

There is no audio-video receiver: passthrough is checked by the bitstream Aerith sends, never by a
device's display.

### Budgets

The targets are the starting points; M1.2 measures the rivals and sets each figure. The last column
gives what M1.2 has measured so far on the high tier, from
[`measurements.md`](measurements.md#m12--the-rivals-on-the-high-tier): mpv alone and part of Kodi.
Stremio, Jellyfin Desktop and the lowest laptop are still to measure; each target's figure is set
once they are, with the owner.

| Budget | Kind | Target | Measured by | Checked in | Rivals measured (M1.2) |
|---|---|---|---|---|---|
| At rest | floor | 0 frames drawn, 0 wake-ups a second of Aerith's own, playback paused or stopped | `pidstat -w`, `/proc/<pid>/status` | the player's: its `at_rest` test, in CI, from M1.3; the rest by hand at each milestone touching it | not measured: Aerith's own |
| Cadence | floor | No frame dropped or repeated without a cause shown, over 30 min, for 23.976 to 60 fps content, at 60 Hz, at 240 Hz and with VRR | the player's frame statistics and the compositor's presentation feedback | the Stage 1 bench, by hand | mpv on 240 Hz: 0 dropped, 0 delayed, 2 min at 23.976 and at 59.94, 1 min at 25 (3.2 threads busy elsewhere). By the compositor's feedback (M1.4b), under mpv's default audio sync: 3 % to 23 % of frames off cadence at 240 Hz, in mpv alone as in Aerith; under VRR, on a quiet machine, 2 of about 1,390. 60 Hz not measured |
| Sound and image | floor | Within ±20 ms over 30 min, after seeks and pauses | the engine's own estimate (ADR-0013); a capture of the beat later | the Stage 1 bench | mpv: under 0.05 ms, its estimate without the card's latency; no seek or pause measured |
| Image | floor | The same picture as mpv alone, same version and options, within a tolerance set at M1.2 | captured frames compared | the Stage 1 bench | M1.4b: Aerith's frames, as mpv draws them, differ from mpv alone's by one step of 8-bit dithering (PSNR 62 dB), as mpv dithers without colour management; the tolerance is still to set |
| HDR and Dolby Vision | floor | Each clip of the corpus reaches the display with the colour description expected | the colour description read back on each path | the Stage 1 bench | mpv tone-maps every HDR clip to SDR on the owner's SDR desktop; HDR output is M1.5's |
| Passthrough | floor | The bitstream sent is identical to the source's, for E-AC3, TrueHD, DTS and DTS-HD | a capture of what the audio output receives | by hand at each engine milestone | not measured: M1.5 |
| Power while playing | lead | At most mpv alone's, plus a margin set at M1.2, on the lowest laptop | the GPU's `power1_average`; the CPU's time | by hand | high tier, over idle: mpv +6.6 W, Kodi +14.5 W (4K HDR10 remux); mpv +2.2 W (1080p 23.976). Laptop not measured |
| Memory while playing | floor | The cache bounded in bytes; a 4K remux plays on the lowest laptop without swap | the resident size, the cache's own count | by hand | high tier, peak PSS on a 4K remux: mpv 262 MiB, Kodi 430 MiB. Laptop not measured |
| Press Play to first frame | lead | Ahead of the best rival, for a local file and for a stream on the LAN | the bench, from a file and from HTTP on loopback | by hand | median, 4K HDR10: Kodi 342 ms (file), 391 ms (stream); mpv 617 and 690 ms under load, 390–445 ms on other clips at rest. mpv's include part of its start; settled, 126–149 ms ([M1.3](measurements.md#against-m12-on-the-desktop)) |
| Interface response | floor | Input to visible response within one frame, 4.17 ms at 240 Hz, at the 99th percentile (from Stage 3) | uwui's frame statistics | by hand | – |
| Start | floor | First useful frame within 100 ms warm and 500 ms cold, as uwui's bar (from Stage 3) | `hyperfine`, the first frame's time | by hand | – |
| Stability | floor | 99.9 % of hours without a crash; each crash leaves a report kept on the machine | the crash reports | at each stage audit | – |
| CI | floor | Each job under 20 min | CI's own times | CI | – |

### Without a figure

- Accessibility: screen readers on each platform, the whole program by keyboard and by remote,
  subtitles styled for legibility, reduced motion and transparency honoured, contrast checked.
- Every text ready for translation: no sentence built from pieces, plurals by rule, layouts that hold
  longer text and right-to-left writing.
- Parsers of untrusted data (addon responses, subtitles, file names, plugin output) are fuzzed and
  bounded in size.

## 3. Decisions

### Fixed

| Topic | Decision |
|---|---|
| What Aerith is | A video client: addons, local files, Jellyfin; Nuvio, Trakt and its own sync ([ADR-0001](adr/0001-what-aerith-is.md)) |
| Licence | MPL-2.0 ([ADR-0002](adr/0002-mpl-2-0.md)) |
| Player on the desktop | mpv in its own process, its image inside the window ([ADR-0003](adr/0003-mpv-in-its-own-process.md)) |
| Player engines | mpv, Media3, AVFoundation behind one interface ([ADR-0004](adr/0004-a-player-engine-per-platform.md)) |
| Interface | On uwui, as a later client; no temporary interface ([ADR-0005](adr/0005-the-interface-on-uwui-a-later-client.md)) |
| Platforms | Linux, Windows, macOS, iPhone, Apple TV, Android phones, Android TV ([ADR-0006](adr/0006-seven-platforms.md)) |
| Content and stores | No source by default; store and full variants ([ADR-0007](adr/0007-a-neutral-client-in-two-variants.md)) |
| Quality bar | Floors and leads, figures measured first ([ADR-0008](adr/0008-the-quality-bar.md)) |
| Stages | Their order ([ADR-0009](adr/0009-the-order-of-the-stages.md)) |
| Logo | Aerith's mark, outside the MPL ([ADR-0010](adr/0010-the-logo.md)) |
| Toolchain | Stable Rust pinned; a nightly only for the checks that need one ([ADR-0011](adr/0011-a-pinned-stable-toolchain.md)) |
| Test corpus | Generated where free tools can; Dolby Vision 7, DTS-HD, DTS:X, TrueHD Atmos and real footage fetched and pinned ([ADR-0012](adr/0012-the-test-corpus.md)) |
| The bench | A Rust tool driving each rival's engine, sound silenced, no sound device in reach ([ADR-0013](adr/0013-the-bench.md)) |

### Open

Each is decided by an ADR at the stage where it becomes necessary, not earlier.

| Topic | Candidates | Decide at |
|---|---|---|
| The Windows montage | mpv's child window under DirectComposition; other routes the bench finds | M1.6 |
| Storage | SQLite (rusqlite) with a schema ready for sync: a clock per field, tombstones | Stage 2 |
| The Stremio protocol | Aerith's own types from the specification, stremio-core's as a test oracle | Stage 2 |
| Nuvio sync | Its Supabase API, tested against a pinned self-hosted backend | Stage 2 |
| The look | Directions shown as captures and recordings off screen | Stage 3 |
| Packages | Flatpak, AppImage, Nix, an MSI | Before the first release |
| Metadata for the library | TMDB (its terms: attribution, cache of at most 6 months), TheTVDB | Stage 5 |
| The Apple engine | Aerith's own, or AetherEngine | Stage 7 |
| Aerith's own sync | A last-writer-wins log in SQLite; a small self-hostable server; iroh between devices | Stage 8 |
| Torrents | librqbit; libtorrent | Stage 9 |
| The JavaScript runtime | rquickjs (QuickJS-NG); boa | Stage 9 |
| Stores | Google Play, App Store, Flathub, F-Droid, AltStore PAL | After Stage 5 |

## 4. Stages

### Stage 1 — The player on the desktop

**Goal.** Aerith plays any file or stream on Linux and Windows with mpv's image and sound, its
picture inside Aerith's window under a test interface, measured against the rivals
([ADR-0003](adr/0003-mpv-in-its-own-process.md)).

- The rivals measured and the bar's figures set; the test corpus.
- mpv driven in its own process through `aerith-player`.
- On Linux, Aerith's Wayland compositor for mpv: mpv's buffers in a subsurface under an SDR
  interface, HDR kept.
- On Windows, the montage decided and built.
- Passthrough checked by the bitstream sent.

**Exit.** Every floor of the bench holds on the high tier and the lowest laptop, on Linux and
Windows: the corpus plays with no unexplained frame dropped in 30 min at 60 Hz, 240 Hz and VRR,
sound and image within ±20 ms, HDR and Dolby Vision clips with the colour description expected,
passthrough identical; the figures are in `docs/measurements.md`.

### Stage 2 — The core

**Goal.** Without an interface, Aerith browses Stremio addons, signs in to Nuvio, keeps its data and
ranks sources, driven by tests and a command line.

- The Stremio addon protocol, its parsers fuzzed and bounded.
- The Nuvio account: sign-in by device code, profiles, library, progress, addons; writes that never
  lose what Aerith does not use.
- Storage ready for sync; tokens in the keyring.
- Sources ranked by what the device can decode and the person's languages.
- One I/O layer with its rules (hosts, sizes, times); every text in a catalogue ready for
  translation.

**Exit.** The core passes its tests against local fakes and a pinned self-hosted Nuvio backend; its
parsers survive an hour of fuzzing each; a session browses, resumes and plays through Stage 1's
player from the command line.

### Stage 3 — The interface on the desktop → 0.1

**Starts when** uwui holds the needs marked indispensable for this stage in
[`uwui-needs.md`](uwui-needs.md).

**Goal.** The first usable version: Stremio addons and Nuvio, the player inside the window, the new
look, at a desk and on a couch, on Linux and Windows.

**Exit.** The owner uses it in place of Nuvio for a week; the interface budgets hold; downloaded
directly.

### Stage 4 — The Android phone

**Goal.** Aerith on Android phones, on Media3, with the same core and interface.

**Exit.** The floors hold on the lowest phone; HDR through a SurfaceView; TalkBack works.

### Stage 5 — The local library and Jellyfin

**Goal.** The person's own films and series, from a disk or a NAS, recognised and with their
artwork; Jellyfin servers browsed and played directly, progress reported.

**Exit.** The owner's library and Jellyfin server work on the desktop and the phone; store releases
may follow.

### Stage 6 — Android TV

**Goal.** Aerith on Android TV, driven by the remote, its home channels and voice search.

**Exit.** The floors hold on the named box; Google's TV quality checklist passes.

### Stage 7 — Apple

**Goal.** Aerith on macOS, iPhone and Apple TV, on AVFoundation.

**Exit.** Dolby Vision and Atmos reach the owner's Apple TV and display as the engine reports; the
TV's mode follows the content; App Review accepts the store build.

### Stage 8 — Trakt and Aerith's own sync

**Goal.** History, lists and progress through Trakt; a person's devices in sync without a third
party's account.

**Exit.** A list and a position changed on one device appear on the others, offline changes merged
without loss.

### Stage 9 — The full variant

**Goal.** Built-in torrents, Nuvio's JavaScript plugins in a sandbox, offline downloads.

**Exit.** Plugins run within their limits and reach nothing outside their rules; a torrent plays
while it downloads within a disk quota; a download plays offline.

### Stage 10 — Watching together

**Goal.** People in different places watch the same thing in sync, on any of Aerith's platforms.

**Exit.** Players stay within a figure set at the stage's design, after seeks and pauses, across
engines.

## 5. Release map

| Version | Stage | Content |
|---|---|---|
| 0.1 | 3 | Addons and Nuvio on Linux and Windows, downloaded directly |
| 0.2 | 4 | The Android phone, an APK |
| 0.3 | 5 | The library and Jellyfin; first store releases |
| 0.4 | 6 | Android TV |
| 0.5 | 7 | macOS, iPhone, Apple TV |
| 0.6 | 8 | Trakt and Aerith's own sync |
| 0.7 | 9 | The full variant |
| 0.8 | 10 | Watching together |
