# Measurements

The figures each milestone measured, with the machine, the versions and the date. The budgets they
answer to are in the [quality bar](ROADMAP.md#budgets).

## M1.1 — The workspace and its checks

Measured on 2026-11-11 on the high tier ([ROADMAP](ROADMAP.md#hardware)), Rust 1.99.1, cargo-deny
0.20.2, lychee 0.24.2. The workspace holds one crate, documentation only, and no dependency.

| What | Figure | How |
|---|---|---|
| `scripts/check.sh`, cold | 8.7 s | `time nix develop -c scripts/check.sh` after `rm -rf target`; the shell already built, the advisory database already fetched once |
| `scripts/check.sh`, warm | 7.9 to 8.8 s | the same, three runs in a row |
| Its last step, `scripts/test-check.sh` | about 7 s of the above | 19 scratch copies, one per case |
| The other steps, warm | 1.1 to 1.5 s | the same, with `AERITH_CHECK_INNER=1`, which skips the last step |
| `cargo deny --locked check` alone | 0.8 to 1.0 s | mostly the advisory database's fetch |
| CI, Linux job | 42 s | GitHub's own time for the `check` job of run 42938126550, from a fresh runner: Nix installed, toolchain downloaded |
| CI, Windows job | 35 s | GitHub's own time for the `windows` job of the same run, toolchain installed by rustup |

Budget: each CI job under 20 minutes (a floor). Each job's timeout is set to it. The local check has
no budget of its own yet.

## M1.2 — The rivals on the high tier

Measured on 2026-11-11 from 04:29 to 04:45 CET, on the high tier ([ROADMAP](ROADMAP.md#hardware)),
by `scripts/bench-rivals.sh short` ([CONTRIBUTING](../CONTRIBUTING.md#the-set-of-measures)), with
the corpus of [ADR-0012](adr/0012-the-test-corpus.md). The display: DP-3, 3840 × 2160 at
240.016 Hz, scale 1.5, VRR off on the monitor, SDR desktop. Sound into the bench's silent PipeWire
([ADR-0013](adr/0013-the-bench.md)). Every rival with its own defaults, a fresh profile and empty
shader caches, fullscreen, in the bench's sandbox. "Threads" are the machine's logical CPUs: 16.

- mpv 0.42.0 (`mpv-with-scripts-0.42.0`), FFmpeg 9.0.2, with `--no-config --vo=gpu-next
  --gpu-context=waylandvk,wayland --hwdec=auto-safe --ao=pipewire`: it used Vulkan and `vaapi`.
- Kodi 21.3 (`kodi-21.3`, Wayland build), with `--windowing=wayland --audio-backend=pipewire -fs
  --debug`; its debug log costs some CPU: it used `ff-hevc-vaapi`, and drew at 5120 × 2880, twice
  the scaled desktop's 2560 × 1440, which the compositor scales down.

**Limits.** The series was cut by the owner's use of the machine after mpv's 25 runs and Kodi's
seven: Kodi's cadence and report runs, Stremio and Jellyfin Desktop are not measured on this tier
yet. Other sessions ran builds and a load test meanwhile: the "others" column gives the threads they
used during each run. Runs are short (10 s to 2 min): they show what each engine does, not what
30 minutes would.

### The time to the first frame

From the load command to the engine's marker ([ADR-0013](adr/0013-the-bench.md)): mpv's
`playback-restart`, Kodi's `Player.OnAVStart`. The clip is `hdr10-2160p-hevc10-eac3`, its first
256 MiB in the page cache; a stream is HTTP on 127.0.0.1, served by the bench.

| Rival | From | Runs, ms | Median | Other programs, threads |
|---|---|---|---|---|
| mpv | file | 1648, 489, 617 | 617 | 11.2, 4.9, 9.1 |
| mpv | stream | 460, 1216, 690 | 690 | 1.1, 14.9, 6.9 |
| Kodi | file | 315, 342, 355 | 342 | 0.2, 0.2, 0.1 |
| Kodi | stream | 391, 420, 357 | 391 | 0.3, 0.1, 0.2 |

On the other clips, with the machine nearly idle (under 0.6 thread used by others), mpv's first
frame came after 390 to 445 ms from a file: 4K HEVC, 1080p H.264 and every sound codec alike. Its
three runs on the HDR10 clip ran beside a load test of 5 to 15 threads, and are to be taken again.

### Cadence, sync, memory, CPU and power

| Rival | Clip | Played | Dropped | Decoder dropped | Delayed | Sync, max | Memory, peak PSS | CPU, % of one thread | GPU, idle → playing | Others, threads |
|---|---|---|---|---|---|---|---|---|---|---|
| mpv | heavy-hdr10-2160p-hevc10-truehd | 120 s | 0 | 0 | 0 | 0.01 ms | 262 MiB | 5.2 % | 35.2 → 41.8 W (+6.6) | 0.2 |
| mpv | cadence-23.976-1080p-h264-aac | 120 s | 0 | 0 | 0 | 0.01 ms | 169 MiB | 2.7 % | 35.6 → 37.8 W (+2.2) | 1.4 |
| mpv | cadence-59.94-1080p-h264-aac | 120 s | 0 | 0 | 0 | 0.01 ms | 170 MiB | 4.9 % | 34.3 → 41.1 W (+6.8) | 0.3 |
| mpv | cadence-25-1080p-h264-aac | 60 s | 0 | 0 | 0 | 0.04 ms | 168 MiB | 3.0 % | 35.3 → 37.8 W (+2.5) | 3.2 |
| Kodi | heavy-hdr10-2160p-hevc10-truehd | 120 s | 0 (logged) | – | – | – | 430 MiB | 10.7 % | 35.4 → 49.9 W (+14.5) | 0.2 |

- mpv kept `video-sync=audio`, its default: display sync was off, so mistimed frames are not
  counted. Its sync figure is its own estimate against the silent sink, which has no hardware
  latency.
- Kodi's "dropped" counts its log's `dropped in output` lines, a lower bound; its audio engine
  logged no sync error after the first two seconds.
- The GPU's power is the whole card's, the desktop's included: the idle figure is read for 10 s
  just before each run.
- These runs predate the sandbox's own home and process numbers ([ADR-0013](adr/0013-the-bench.md));
  the sound devices were already out of the rivals' reach.

### What the engines report

- **HDR on this desktop.** For every HDR clip (HDR10, HDR10+, HLG, Dolby Vision 5, 8.1 and 8.4,
  and *Cosmos Laundromat*), mpv's `video-target-params` was gamma 2.2 in BT.709: mpv tone-mapped to
  SDR, because the desktop is SDR. Kodi reported `VideoPlayer.HdrType` `hdr10` and drew the same
  SDR desktop. What reaches an HDR output is M1.5's.
- **Dolby Vision 7.** mpv opened both profile 7 samples, decoded with `vaapi`; Kodi was not run on
  them on this tier. In `agent-headless`, Kodi ended each clip shorter than a second (profile 7,
  DTS:X, TrueHD Atmos) without reporting its first frame.
- **Sound.** mpv sent every codec to the sink as PCM in the clip's own layout: 5.1 for E-AC3,
  TrueHD and DTS, 7.1 for DTS-HD MA and TrueHD Atmos. Kodi reported the E-AC3 stream's 5.1
  (`Player.Process(audiochannels)`); its sink opens in stereo, Kodi's default speaker setting.

### In `agent-headless`, without a GPU

The same set, run under `agent-headless` on the quick corpus to check the bench, found these
defaults of the other rivals (figures without a GPU are not tier figures):

- Stremio's mpv decodes in software: its shell never sets `hwdec`.
- Jellyfin Desktop's mpv uses `vaapi-copy` and downmixes to stereo by default.

## M1.3 — mpv driven through `aerith-player`

Measured on 2026-11-11 on the high tier ([ROADMAP](ROADMAP.md#hardware)), with mpv 0.42.0
(`mpv-0.42.0`, the build the bench's `mpv-with-scripts` wraps), FFmpeg 9.0.2, Rust 1.99.1, in
`nix develop`, the machine nearly idle (load average 0.5 to 0.8 on 16 threads). The picture and the
sound discarded (`--vo=null --ao=null`), so mpv decodes in software and draws nothing; each mpv in
bubblewrap ([CONTRIBUTING](../CONTRIBUTING.md#the-players-tests)). The figures show the player's
behaviour and its cost beside mpv's, not a tier figure: those need the picture on the desktop
([below](#against-m12-on-the-desktop)).

### The time to the first frame

From the load to the first frame (`open` to `Event::Started` through the player, `loadfile` to
`playback-restart` for mpv alone), each started a second before, the bench's clip
`hdr10-2160p-hevc10-eac3` in the page cache, by the corpus test's `time_to_the_first_frame`
([CONTRIBUTING](../CONTRIBUTING.md#the-players-tests)). One run of each left out, then seven,
alternated; two passes.

| From | Through the player | mpv alone, the player's options | mpv alone, its defaults |
|---|---|---|---|
| file, pass 1 | 225, 205, 176, 183, 157, 171, 168 ms; median 176 | 231, 172, 152, 171, 149, 153, 183 ms; median 171 | 243, 130, 116, 138, 98, 96, 80 ms; median 116 |
| file, pass 2 | 234, 188, 164, 186, 160, 162, 163 ms; median 164 | 223, 149, 155, 170, 146, 153, 184 ms; median 155 | 245, 127, 119, 135, 99, 88, 81 ms; median 119 |
| stream, pass 1 | 178, 170, 176, 177, 174, 184, 179 ms; median 177 | 190, 152, 197, 156, 166, 174, 149 ms; median 166 | 112, 103, 93, 94, 108, 108, 123 ms; median 108 |
| stream, pass 2 | 179, 167, 170, 174, 178, 170, 180 ms; median 174 | 192, 153, 179, 157, 178, 175, 164 ms; median 175 | 117, 103, 105, 106, 104, 128, 106 ms; median 106 |

- **The player's own cost** is the difference with mpv alone run with the same options: 5, 9, 11
  and −1 ms between medians, within the runs' spread.
- **mpv's options cost more.** With the built-in scripts and bindings turned off as the player
  does, mpv shows its first frame 36 to 69 ms later than with its defaults, in these conditions.
  No single option does it: mpv alone given each option by itself, eight runs each, showed its
  first frame in 80 to 130 ms after its first runs, as with its defaults, while the six options of
  the built-in scripts together, or all of the player's, brought it to 140 to 200 ms. That probe
  is not in the repository; `time_to_the_first_frame` keeps the comparison of the whole sets. Why
  is not known. It does not hold on the desktop, with the picture shown
  ([below](#against-m12-on-the-desktop)).
- With its defaults, mpv alone gets faster run after run (243 to 80 ms), and with the player's
  options it does not. The first run of each series is the slowest.

### Against M1.2, on the desktop

Measured on 2026-11-11 from 13:33 to 13:40 CET on the owner's desktop, through `owner-desktop`, the
machine idle (load average about 1), the display as M1.2 recorded it (DP-3, 3840 × 2160 at
240.016 Hz). The clip `hdr10-2160p-hevc10-eac3`, from a file unless said. Every mpv drew through
gpu-next on `waylandvk` and decoded with `vaapi`, as each reported, with a fresh home and empty
shader caches. The bench's runs play into its silent PipeWire; the player and the last probe
discard the sound.

| What | Runs, ms |
|---|---|
| mpv alone, the bench's way (M1.2's method): its defaults, fullscreen | 514.9, 453.3, 443.9 |
| The same, as a stream | 442.5, 471.9, 447.8 |
| The same, in a window (`fullscreen=no`) | 458.0, 479.3, 458.7 |
| The same, its on-screen controller off (`osc=no`) | 411.3, 436.2, 427.3 |
| The same, the six built-in scripts' options of the player off | 412.7, 412.0 |
| The same, the player's input options | 440.0, 444.5 |
| The same, `ytdl=no load-auto-profiles=no osc=no` | 429.5, 449.8 |
| mpv alone, the bench's way, all the player's options, in a window | 202.6, 211.7, 215.4 |
| The same, fullscreen | 215.2, 222.0, 210.4 |
| mpv alone, its defaults, fullscreen, the clip loaded 2 s after mpv's IPC opened | 149.1, 139.0, 125.9 |
| Through the player, `play --window`, the clip opened 1 s after the player started | 248.4, 243.6, 248.2, 247.4, 248.9 |

- **The bench's figure for mpv alone includes part of mpv's start.** The bench loads the clip as
  soon as mpv's IPC opens. Loaded once mpv has settled, mpv alone shows its first frame in 126 to
  149 ms; loaded at once, in 444 to 515 ms with its built-in scripts, and in 203 to 222 ms with
  all of them off. That fits mpv holding the first file until its scripts have started; no single
  group of options does it. So M1.2's figures for mpv alone (617 and 690 ms under load, 390 to
  445 ms at rest) measure a start, not a press of Play on a started mpv. Whether Kodi's carry a
  start too is not known.
- **Through the player, the first frame comes about 100 ms after a settled mpv alone's.** The
  probable cause, not verified: in `Video::Window`, mpv opens its window and its video output at
  the first load, while mpv alone (`force-window`) had them open before.
- **The cost of the player's options seen with the picture discarded does not hold here**: on the
  display, with mpv loading at once, those options make the first frame faster.
- The run with mpv loaded 2 s after it started was driven from a shell over `socat`, outside the
  repository, as the bench cannot wait yet; its command line was the bench's, with `--ao=null`.

### The corpus

Every clip opened paused, sought to 0.2 s, played and stopped, by the corpus test. The time is to
the first frame, from a paused start; what follows is mpv's report.

| Clip | First frame | What mpv reported |
|---|---|---|
| audio-dts-5.1 | 100 ms | h264 1920×1080 bt.709/bt.1886; dts 5.1(side) 48 kHz |
| audio-dtshd-ma-7.1 | 31 ms | h264 1920×1080 bt.709/bt.1886; dts 7.1(wide) 48 kHz |
| audio-dtsx-7.1.4 | 13 ms | h264 1920×1080 bt.709/bt.1886; dts 7.1 48 kHz |
| audio-eac3-5.1 | 34 ms | h264 1920×1080 bt.709/bt.1886; eac3 5.1(side) 48 kHz |
| audio-truehd-5.1 | 28 ms | h264 1920×1080 bt.709/bt.1886; truehd 5.1(side) 48 kHz |
| audio-truehd-atmos-7.1 | 12 ms | h264 1920×1080 bt.709/bt.1886; truehd 7.1 48 kHz |
| cadence-23.976 to cadence-60 (7 clips) | 27 to 34 ms | h264 1920×1080 bt.709/bt.1886; aac stereo 48 kHz |
| dv5-2160p-hevc10-eac3 | 233 ms | hevc 3840×2160 bt.2020/pq; eac3 5.1(side) 48 kHz |
| dv7-1080p (MPEG-TS) | 63 ms | hevc 1920×1080 bt.2020/pq; no sound track |
| dv7-2160p | 93 ms | hevc 3840×2160 bt.2020/pq; no sound track |
| dv8.1-2160p-hevc10-eac3 | 206 ms | hevc 3840×2160 bt.2020/pq; eac3 5.1(side) 48 kHz |
| dv8.4-2160p-hevc10-eac3 | 191 ms | hevc 3840×2160 bt.2020/pq; eac3 5.1(side) 48 kHz |
| hdr10-2160p-hevc10-eac3 | 203 ms | hevc 3840×2160 bt.2020/pq; eac3 5.1(side) 48 kHz |
| hdr10plus-2160p-hevc10-eac3 | 219 ms | hevc 3840×2160 bt.2020/pq; eac3 5.1(side) 48 kHz |
| heavy-hdr10-2160p-hevc10-truehd | 216 ms | hevc 3840×2160 bt.2020/pq; truehd 5.1(side) 48 kHz |
| hlg-2160p-hevc10-eac3 | 209 ms | hevc 3840×2160 bt.2020/hlg; eac3 5.1(side) 48 kHz |
| real-cosmos-laundromat-hdr10 | 65 ms | hevc 2048×858 bt.2020/pq; no sound track |
| sdr-2160p-hevc10-eac3 | 182 ms | hevc 3840×2160 bt.709/bt.1886; eac3 5.1(side) 48 kHz |
| hdr10-2160p-hevc10-eac3, as a stream | 185 ms | the same as from the file |

- The audio clips' sounds are short (the fetched DTS:X and TrueHD Atmos samples last under a
  second): their first frames come early. Their pictures last two frames: the seek to 0.2 s stops
  on the last one, at 41 ms, and the shortest end as they play again.
- `dv7-1080p`, an MPEG-TS clip under half a second, ends when mpv seeks into it; the player reports
  it as `Ended(Finished)`.
- mpv's `video-params` say nothing of Dolby Vision: the profile 5, 7 and 8 clips report their
  base layer's colour. Reporting Dolby Vision is M1.5's.

### At rest

The player's two threads' context switches, read in `/proc/self/task/<tid>/status` by the test
`at_rest`, over 2 s in each state after a second for the change to settle, on a 320×240 clip mpv
makes, beside mpv's own threads, in three runs:

| State | The player's threads | mpv's threads |
|---|---|---|
| Playing | 0 | 210 to 219 a second |
| Paused | 0 | 2 a second |
| Stopped, idle | 0 | 1 a second |

The floor holds: 0 wake-ups of Aerith's own, paused or stopped, which the test asserts. While
playing, the test prints the figure without asserting it: 0 in every run, as the position is sent
as a clock. mpv's are mpv's; the bench compares them with mpv alone.

### The checks

| What | Before M1.3 | After | How |
|---|---|---|---|
| `scripts/check.sh` | 33.3 s | 126.9 s | the whole check, warm, load about 1; before: the starting commit in a worktree of its own, load 8 to 11 |
| Its steps without `scripts/test-check.sh` | – | 18.5 s | with `AERITH_CHECK_INNER=1` |
| `cargo test -p aerith-player` | – | about 15 s | of which 9 s for `at_rest`, 5 s for the real-mpv tests, 2 s for the unit tests |
| CI, Linux job | 71 s | 205 s | GitHub's own times for the `check` job of runs 42963265100 and 42985548274: Nix installed, mpv downloaded, then `scripts/check.sh` |
| CI, Windows job | 40 s | 50 s | the `windows` job of the same runs |

`scripts/test-check.sh` runs the whole test suite in six of its scratch copies, `at_rest`
included: most of the growth is there.

## M1.4a — The relay and the test interface

### The clock under display sync

Measured on 2026-11-11 on the owner's desktop through `owner-desktop`, for
[ADR-0016](adr/0016-the-clock-carries-the-engines-rate.md). Setup:
- mpv 0.42.0 alone, through `--no-config --vo=gpu-next --gpu-context=waylandvk --hwdec=auto-safe
  --ao=null --video-sync=display-resample`, fullscreen;
- Umbriel, with RADV;
- a fresh home, the corpus's cadence clips.

A throwaway script drove mpv over a socket. It loaded the clip 2 s after mpv started, waited 10 s
for display sync to settle, then read `time-pos` every second for 150 s and fitted a line through
the readings. It observed `video-speed-correction` throughout.

| Output | Clip | `video-speed-correction` | Its changes, after settling | Fitted rate of the position | Above the correction | Furthest reading from the line | Dropped / mistimed |
|---|---|---|---|---|---|---|---|
| DP-3, 240.016 Hz | cadence-23.976 | 1.001067 | 0 in 150 s | 1.001246 | +0.018 % | 33 ms | 0 / 5 |
| DP-3, 240.016 Hz | cadence-25 | 0.99317 | 0 in 150 s | 0.993351 | +0.018 % | 22 ms | 0 / 1 |

An earlier pass with two readings only, 120 s apart, ran on DP-2 at 179.999 Hz, where mpv opened
by mistake. Its corrections were 1.000994 for the 23.976 and 59.94 fps clips and 0.993098 for
25 fps, with no change after settling.

- **At real time, the clock drifts** by +0.12 % (23.976 fps) to −0.67 % (25 fps): up to 24 s an
  hour.
- **At the correction's rate, the clock drifts by +0.018 %**, the same on both clips: about 11 ms
  a minute.
- The readings lie up to one frame from the line. mpv's position moves by whole frames.
- mpv made 5 and 1 corrections of its timing (`mistimed-frame-count`) in the two runs.

**In `agent-headless`.** The headless sway offers no `linux-dmabuf`, so mpv on RADV fails
(`VK_ERROR_SURFACE_LOST_KHR`). lavapipe works only when named with `--vulkan-device`. It could
not keep 60 Hz at 1080p, even with mpv's `fast` profile. Timing figures there are not tier figures.

### The relay against a fake compositor

Measured on 2026-11-11 on the high tier, in `nix develop`. Setup:
- `tests/at_rest.rs`: the player shows its picture in a plane of a fake compositor's window
  (`tests/common/host.rs`), which answers every commit at once;
- mpv 0.42.0 draws through the relay with lavapipe, on a 320 × 240 clip at 24 frames a second;
- Aerith's threads counted: `aerith-mpv`, `aerith-mpv-read` and `aerith-relay`.

| State | Aerith's threads' wake-ups | mpv's |
|---|---|---|
| Playing | 124.5 to 145.0 a second, in four runs | 17,603 to 18,605 a second: lavapipe's rendering threads |
| Paused | 0 in 2 s | 4 in 2 s |
| Stopped | 0 in 2 s | 2 in 2 s |

These are not tier figures: lavapipe draws on the CPU, and the fake compositor presents at once.
They show the floor at rest holds with the relay. The playing figures on the owner's display come
with the testbed.

**The fuzz target** (`crates/aerith-player/fuzz`, `cargo fuzz run relay`):
- 600 s: 134,631 executions, 224 a second;
- 10,441 edges covered;
- no crash, leak or timeout.

Trial runs before it found three defects, fixed with tests
([ADR-0018](adr/0018-what-building-the-relay-changed.md)).

### Inside the testbed, on the desktop

Measured on 2026-11-11 from 22:35 to 22:42 CET on the owner's desktop, through `owner-desktop`.
Setup:
- the display: DP-3 at 240.016 Hz, scale 1.5, SDR, Umbriel;
- `aerith-testbed` fullscreen ([CONTRIBUTING](../CONTRIBUTING.md#the-test-window)), the pinned mpv
  0.42.0 through the relay;
- the sound discarded, a fresh home;
- the source opened 1 s after the player started.

Every mpv decoded with `vaapi` and drew through gpu-next. Scripts outside the repository drove the
runs (`/tmp/m14-desk`, thrown away).

**The corpus's SDR clips**, 8 s each:
- In the order below: `sdr-2160p-hevc10-eac3`, the seven `cadence-*` clips and `audio-eac3-5.1`.
- In the order above: `sdr-2160p-hevc10-eac3`, `cadence-23.976` and `cadence-59.94`.

Each played to its end with 0 frames discarded. At most one frame committed was not yet presented
when the run stopped.

**The five other SDR clips**: `audio-dts-5.1`, `audio-dtshd-ma-7.1`, `audio-dtsx-7.1.4`,
`audio-truehd-5.1` and `audio-truehd-atmos-7.1`, 4 s each in the order below.
- When: on 2026-11-12. The Steam client ran (no game), and the machine's state differed from the
  day before.
- Each played and the testbed exited 0, with 0 frames discarded.
- The pictures of `audio-dtsx-7.1.4` and `audio-truehd-atmos-7.1` last under a second
  ([above](#the-corpus)); they ended there.
- First frames with empty caches: 547 to 715 ms, against about 160 ms for the cadence clips the day
  before. The refresh gaps were wider too: of a clip's 100 frames, 10 refreshes about half the
  time, 9 or 11 about 40 %, 8 or 12 a few. These runs are not compared with the others.

**The time to the first frame**, the clip `hdr10-2160p-hevc10-eac3` (mpv tone-maps it to this SDR
desktop):

| What | Caches | Runs, ms |
|---|---|---|
| The testbed, through the relay, mpv's window ready | empty at each run | 203.4, 206.7, 207.7, 196.6, 202.3 |
| The same | kept between runs (`--cache`) | 519.0, 234.0, 97.4, 69.6, 70.4, 69.2 |
| mpv alone, `--no-config --force-window --hwdec=auto-safe --ao=null`, fullscreen, the clip loaded 2 s after start, over a socket | kept between runs | 241.3, 79.5, 80.6, 79.7, 80.8 |
| Through the player at M1.3, mpv's own window opened at the load ([above](#against-m12-on-the-desktop)) | empty at each run | 243.6 to 248.9 |

Readings:
- **Settled, with its caches, Aerith shows the first frame in about 70 ms**, against about 80 ms
  for mpv alone. The runs differ: the testbed opens 1 s after the player starts, with the
  player's options; mpv alone loads 2 s after its start, with fewer options.
- **With empty caches at each run**, the player went from 244–249 ms at M1.3 to 197–208 ms with
  mpv's window ready. M1.3's mpv alone, also with empty caches, took 126–149 ms. Today's first
  mpv-alone run, with an empty cache, took 241 ms. The two sessions' mpv-alone figures disagree,
  with different options; why is not known.
- The first runs with kept caches are slower: 519 and 234 ms through the testbed, 241 ms for mpv
  alone. The GPU driver's cache fills too. Why the testbed's first run (519 ms) is slower than
  its runs with empty caches (about 203 ms) is not known.

**While playing**, `cadence-23.976` for 60 s, controls hidden, counted over the last 55 s:

| Order | The window's thread | The relay's thread | mpv's frames presented | Presented without a copy (`zero_copy`) |
|---|---|---|---|---|
| Below | 50.2 wake-ups a second | 141.8 a second | 23.96 a second | 0 |
| Above | 50.9 a second | 141.6 a second | 23.96 a second | 0 |

- **The window's thread wakes about twice a frame** for the compositor's events addressed to mpv,
  on the socket it shares with the relay
  ([ADR-0018](adr/0018-what-building-the-relay-changed.md)).
- **Umbriel never scanned the video out**, in either order. Its log shows no change from
  composited during the testbed's runs. For mpv alone it showed `direct scanout on` while idle, and
  `off (color_conversion)` on the HDR clip.

  The probable cause: the relay does not pass mpv's opaque region. Umbriel then cannot cull the
  black root under the video, and has two buffers to show. To fix before M1.4b compares the orders.
- **Refresh gaps between consecutive presented frames**, at 240.016 Hz. A 23.976 fps frame should
  last 10 refreshes, with one of 11 about every 3.9 s.

  | Order | 10 refreshes | 9 | 11 | Total frames |
  |---|---|---|---|---|
  | Below | 1,280 | 71 | 86 | 1,443 |
  | Above | 1,260 | 81 | 95 | 1,443 |

  The 59.94 and 60 fps clips showed 3 and 5 refreshes around their 4 in the same way. Whether mpv
  alone does the same is M1.4b's to measure: it paces itself in MAILBOX on Umbriel, which has no
  fifo-v1. It bears on the cadence floor.

**mpv's memory while idle**, PSS 3 s after its start:
- 137 MiB with its window and video output ready (`--force-window=yes`);
- 53 MiB without.

Keeping it ready costs 84 MiB, on this tier, for the whole time Aerith runs.

**The checks' and CI's time** with the relay and the testbed:

| What | Before | After |
|---|---|---|
| CI, Linux job | 3 min 43 s (run 43062941172) | 6 min 30 s (run 43104131678) |
| CI, Windows job | 50 s | 50 s |
| `scripts/check.sh`, warm | 3 min 24 s | 3 min 15 s to 3 min 19 s |

- The check's "before" ran at `df40898`, after the relay, in a scratch clone with its own target
  folder; its "after" with the testbed. M1.3's 126.9 s ran before the relay, at another load.
- The check's CPU time grew from 2 min 12 s to about 4 min 15 s, with the testbed's build. Its
  wall time is dominated by `scripts/test-check.sh`'s copies.
- **At rest through the testbed**, in `agent-headless` with lavapipe, the clip paused after 2 s,
  over the last 4 to 5 s:
  - order below: the window's thread and the relay's woke 0 times;
  - order above: the same, 0 and 0.

  At rest on the desktop was not measured.

## M1.4b — Inside the window, measured

### A first pass on the desktop

Measured on 2026-11-12 from 02:20 to 02:23 CET on the owner's desktop, through `owner-desktop`.
Setup:
- the display: DP-3 at 240.016 Hz, scale 1.5, SDR, Umbriel;
- `aerith-bench` ([CONTRIBUTING](../CONTRIBUTING.md#aerith-inside-its-window)), mpv 0.42.0 alone and
  inside `aerith-testbed`, the relay passing mpv's opaque region (`577cbd1`);
- `cadence-23.976-1080p-h264-aac`, 30 s a run, the sound into the bench's silent PipeWire.

Another session's tests kept 9 to 14 hardware threads busy during these three runs: their power
and CPU are not figures. The cadence and the scanout are.

| Player | Frames scanned out | Gaps off cadence, mpv's feedback | The relay's own count | mpv's dropped / delayed |
|---|---|---|---|---|
| mpv alone | 0 of 652 | 241 of 622 | – | 0 / 0 |
| Aerith, order below | 0 of 705 | 241 of 663 | 277 of 719 | 0 / 0 |
| Aerith, order above | 692 of 722 | 207 of 698 | 222 of 719 | 0 / 0 |

- **Umbriel scans the video out in the order above**, now that the relay passes mpv's opaque region:
  mpv's buffers are `ARGB2101010` or the like, with alpha
  ([`surface.rs`](../crates/aerith-player/src/relay/surface.rs)), so without the region the black
  root under them had to be drawn. **In the order below it never can**: winit's transparent
  surface lies over the video, and wlroots scans out a single buffer only.
- **mpv alone keeps cadence no better than Aerith.** The ideal at this rate is a gap of 10
  refreshes and one of 11 in 93; here about a third of the gaps are 9 or 11 (8, 12 and 13 a few),
  in every player. At M1.4a, on a quieter machine, the relay saw about 11 %
  ([above](#inside-the-testbed-on-the-desktop)): the load counts. mpv's own counters see none of it. It is mpv's own pacing: without fifo-v1, it
  commits each frame when due, and the refresh it lands on varies.
- The relay's count, with feedback on every frame, and the figure read from mpv's own feedback
  agree within a few points.

**The frames compared**, `--frames 5,20,40` on `sdr-2160p-hevc10-eac3` and on
`cadence-23.976-1080p-h264-aac`, captured by mpv as it draws them in its window:
- PSNR 62.1 to 62.9 dB between Aerith's and mpv alone's, on 16-bit PNGs; 40 % of the values
  differ, by at most 257 in 65,535, about one step of 8 bits.
- The cause, read in mpv 0.42's `video/out/vulkan/context_wayland.c`: without the colour
  management protocol, mpv dithers to 8 bits ("Dithering to 8 bit depth" in Aerith's mpv log,
  absent from mpv alone's). The relay offers no colour management until M1.5.
- In `agent-headless`, where neither mpv sees colour management, the frames are the same (PSNR
  infinite).

### The series at 240 Hz

Measured on 2026-11-12 from 02:43 to 03:33 CET on the owner's desktop, through `owner-desktop`, by
`BENCH_SECONDS=60 scripts/bench-window.sh` (`4a65058`). Setup as in the first pass, the machine
quiet but for the first two Aerith runs: 60 s a run, the first 2 s left out, the GPU's idle power
read 10 s before each. The owner stopped the series twice; what was left was taken after, below. The results stay in
`target/bench/window-20261112T014314Z` and `window-20261112T022419Z`, not in the repository.

**Cadence under mpv's default audio sync.** "mpv's feedback" counts gaps between frames that both
had feedback; "the relay" counts every frame of Aerith's. The ideal gap is the display's rate over
the clip's.

| Clip | Player | Gaps off cadence, mpv's feedback | The relay | Scanned out | GPU power over idle | CPU, one core | Peak PSS | Other programs, threads |
|---|---|---|---|---|---|---|---|---|
| cadence-23.976 | mpv | 260 of 1372 | – | 0 of 1380 | 1.72 W | 3.7 % | 181.5 MiB | 1.85 |
| cadence-23.976 | aerith, below | 338 of 1334 | 380 of 1439 | 0 of 1362 | 2.91 W | 4.0 % | 187.6 MiB | 3.3 |
| cadence-23.976 | aerith, above | 388 of 1308 | 449 of 1439 | 1349 of 1349 | 2.98 W | 6.1 % | 188.5 MiB | 13.21 |
| cadence-24 | mpv | 257 of 1381 | – | 0 of 1385 | 3.18 W | 3.1 % | 178.9 MiB | 0.47 |
| cadence-24 | aerith, below | 335 of 1381 | 346 of 1440 | 0 of 1386 | 2.1 W | 3.2 % | 185.1 MiB | 0.43 |
| cadence-24 | aerith, above | 274 of 1380 | 284 of 1439 | 301 of 1385 | 2.29 W | 3.3 % | 185.3 MiB | 0.45 |
| cadence-25 | mpv | 49 of 1429 | – | 0 of 1438 | 2.73 W | 3.2 % | 180.8 MiB | 0.44 |
| cadence-25 | aerith, below | 45 of 1434 | 51 of 1500 | 0 of 1441 | 1.63 W | 3.4 % | 187.3 MiB | 0.4 |
| cadence-25 | aerith, above | 72 of 1445 | 75 of 1500 | 1441 of 1447 | 0.75 W | 3.4 % | 187.2 MiB | 0.49 |
| cadence-29.97 | mpv | 239 of 1728 | – | 0 of 1732 | 4.48 W | 3.6 % | 182.2 MiB | 0.52 |
| cadence-29.97 | aerith, below | 296 of 1722 | 319 of 1799 | 0 of 1730 | 2.77 W | 3.8 % | 188.1 MiB | 0.54 |
| cadence-29.97 | aerith, above | 282 of 1732 | 286 of 1798 | 1728 of 1735 | 2.09 W | 3.8 % | 188.2 MiB | 0.43 |
| cadence-50 | mpv | 234 of 2879 | – | 0 of 2888 | 5.07 W | 5.6 % | 182.1 MiB | 0.42 |
| cadence-50 | aerith, below | 233 of 2822 | 267 of 3000 | 0 of 2860 | 5.3 W | 6.3 % | 188.2 MiB | 1.53 |
| cadence-50 | aerith, above | 218 of 2871 | 234 of 3000 | 2874 of 2885 | 3.55 W | 7.0 % | 181.7 MiB | 2.03 |
| cadence-59.94 | mpv | 805 of 3457 | – | 0 of 3466 | 6.02 W | 6.7 % | 177.6 MiB | 0.78 |
| cadence-59.94 | aerith, below | 687 of 3458 | 741 of 3597 | 0 of 3467 | 5.69 W | 6.7 % | 188.5 MiB | 0.62 |
| cadence-59.94 | aerith, above | 689 of 3428 | 749 of 3595 | 1984 of 3451 | 6.36 W | 6.6 % | 188.5 MiB | 0.64 |
| cadence-60 | mpv | 719 of 3459 | – | 0 of 3468 | 5.31 W | 6.4 % | 181.9 MiB | 0.72 |
| cadence-60 | aerith, below | 754 of 3450 | 780 of 3600 | 0 of 3464 | 6.99 W | 6.7 % | 181.3 MiB | 0.67 |
| cadence-60 | aerith, above | 807 of 3464 | 871 of 3600 | 3458 of 3471 | 7.56 W | 7.0 % | 176.6 MiB | 0.55 |
| sdr-2160p | mpv | 277 of 1257 | – | 0 of 1263 | 2.74 W | 3.8 % | 242.5 MiB | 0.66 |
| sdr-2160p | aerith, below | 232 of 1250 | 253 of 1318 | 0 of 1260 | 2.63 W | 4.0 % | 247.1 MiB | 0.72 |
| sdr-2160p | aerith, above | 275 of 1262 | 285 of 1319 | 1260 of 1266 | 3.39 W | 3.9 % | 247.9 MiB | 0.78 |

- **No player meets the cadence floor at 240 Hz under audio sync.** mpv alone has 3 % (25 fps) to
  23 % (59.94 fps) of its gaps off cadence, Aerith about the same in either order. mpv's own
  counters say 0 dropped and 0 delayed in every run. The gaps off are mostly one refresh short
  then one long (9 and 11 at film rate).
- **The cause is the compositor's pacing protocol.** Umbriel offers neither `wp_fifo_v1` nor
  `wp_commit_timing_v1` (none in its source, `d171ccb`). mpv 0.42 then presents in MAILBOX and
  times each frame itself (`video/out/vulkan/context_wayland.c`, `use_fifo`). The relay offers no
  fifo either: on a compositor that has it, mpv alone would use FIFO, and Aerith's mpv not.
- **The order above scans the video out**, but not always: 301 of 1385 frames at 24 fps and 1984
  of 3451 at 59.94; why Umbriel composited the others is not known.
- **GPU power** differs by a watt or two from run to run, in no steady direction between the
  players. Memory: Aerith's tree is 5 to 7 MiB above mpv alone's.
- The window's thread woke 52 to 135 times a second while a film played, the relay's 142 to 363:
  both grow with the clip's rate.

**Display sync**, `video-sync=display-resample`:

| Clip | Player | Presents a second | Delayed / mistimed (mpv) | GPU power over idle | Window thread / relay, wake-ups a second | Sync, max |
|---|---|---|---|---|---|---|
| cadence-23.976 | mpv | 246 | 168 / 51 | 11.77 W | – | 38.0 ms |
| cadence-23.976 | aerith, below | 240 | 43 / 14 | 11.64 W | 578 / 1422 | 13.1 ms |
| cadence-23.976 | aerith, above | 240 | 3 / 2 | 7.84 W | 279 / 929 | 9.1 ms |
| cadence-25 | mpv | 240 | 6 / 1 | 11.12 W | – | 12.9 ms |
| cadence-25 | aerith, below | 240 | 24 / 5 | 11.07 W | 553 / 1394 | 22.1 ms |
| cadence-25 | aerith, above | 240 | 2 / 1 | 9.5 W | 329 / 991 | 7.8 ms |

- **mpv presents at every refresh** under display sync, the same picture several times: 240 a
  second against 24 to 60 under audio sync. The refresh gaps then say nothing of the picture's
  cadence; mpv's own counters do, and show a handful of corrections a minute.
- **It costs 5 to 9 W more** than audio sync at this rate, and the relay's thread wakes about
  1,000 times a second. The order above, scanned out, costs 2 to 4 W less than the order below
  or mpv alone, which the compositor draws.
- The run on 59.94 fps was taken later, below. 60 Hz and VRR are not measured. The sync's maximum is the engine's own estimate; mpv alone's 38 ms at 23.976 is not explained.

**The controls shown and hidden 60 times**, order above, `cadence-23.976`, 125 s: of the relay's
2,997 gaps, 459 off cadence (15 %), among them 28 of 5 to 8 refreshes, which runs without the
controls almost never show. 1,521 frames were scanned out, while the controls hid. The testbed
drew 1,868 frames, none late. The order below's run stopped after 1.5 s, before the owner's
pause, and was taken again below.

**The rest of the 240 Hz series**, on 2026-11-12 from 05:18 to 05:27 CET, the machine quiet
(`others_cores` 0.3 to 1.5), the same setup:

| Clip | Player | Presents a second | Delayed / mistimed (mpv) | GPU power over idle | Window thread / relay, wake-ups a second | Sync, max |
|---|---|---|---|---|---|---|
| cadence-59.94, display sync | mpv | 240 | 31 / 2 | 13.04 W | – | 15.3 ms |
| cadence-59.94, display sync | aerith, below | 240 | 35 / 6 | 17.42 W | 547 / 1375 | 16.4 ms |
| cadence-59.94, display sync | aerith, above | 240 | 5 / 0 | 10.83 W | 282 / 926 | 13.4 ms |

- Under display sync, the order above again costs least: 2 W under mpv alone, 6.6 W under the
  order below, which the compositor draws at every refresh under winit's transparent surface.
- **The controls shown and hidden 60 times, order below**, `cadence-23.976`, 125 s: of the
  relay's 2,997 gaps, 959 off cadence, nearly all 9 or 11 refreshes, 3 of 8, none shorter. The
  toggles add no hitch in this order; in the order above they added about 28 gaps of 5 to 8
  refreshes. The testbed drew 1,858 frames, none late.
- **The frames compared again**, `--frames 5,20,40`: at 20 and 40 s, the same PSNR as at the first
  pass (62.1 to 62.7 dB). At 5 s, 19.0 and 19.3 dB: mpv alone's on-screen controller showed over
  that frame, as it does after a load, where Aerith's mpv has it off (`--osc=no`). Without the
  bottom 15 % of the picture, where it lies, 62.2 and 62.9 dB. `scripts/bench-window.sh` now
  turns it off in the frames' runs.
