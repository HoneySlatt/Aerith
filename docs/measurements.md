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
  agree within a few points. The relay's count runs about 2 s longer, to the testbed's end.

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

- **Sync holds under audio sync**: mpv's estimate stayed under 0.05 ms in every run, against the
  ±20 ms floor.
- **No player meets the cadence floor at 240 Hz under audio sync.** mpv alone has 3 % (25 fps) to
  23 % (59.94 fps) of its gaps off cadence, Aerith about the same in either order. mpv's own
  counters say 0 dropped and 0 delayed in every run. The gaps off are mostly one refresh short
  then one long (9 and 11 at film rate).
- **The first suspect was the compositor's pacing protocol.** Umbriel offers neither `wp_fifo_v1`
  nor `wp_commit_timing_v1` (none in its source, `d171ccb`). mpv 0.42 then presents in MAILBOX
  and times each frame itself (`video/out/vulkan/context_wayland.c`, `use_fifo`). The relay
  offered no fifo either; it does since `e3d0612`. Measured later under KWin, which has fifo
  ([below](#under-kwin-with-fifo)): fifo mends the cadence at 25 fps only; at 23.976 and
  59.94 fps the cause is audio sync itself.
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

### Under KWin, with fifo

Measured on 2026-11-12 on the owner's desktop in a Plasma session, through `owner-desktop`: DP-3
at 240.02 Hz, scale 1.45, KWin 6.8, which offers `wp_fifo_v1`; the relay passes it to mpv
(`e3d0612`), and mpv 0.42 then presents in FIFO. Baloo, KDE's file indexer, kept a fifth of a core
busy. KWin sends 0 as every presentation's refresh counter: the gaps are counted by time
(`752f18e`).

**The full series** (`BENCH_SECONDS=60 scripts/bench-window.sh ... cadence`, 05:14 to 05:44 CET,
`window-20261112T051424Z`) gave few figures: under fifo, mpv asks for feedback on one frame in
two, so its own feedback leaves almost no pair of frames one after the other to measure (0 to 43
gaps a run); the relay there predated the count by time. mpv alone's run on `cadence-23.976`
played on DP-2, where the focus was. mpv reported 0 frames delayed in every run. The order above
was scanned out by KWin too (611 to 1,802 frames of every run); the order below never.

**Aerith again**, the relay counting by time, every frame (05:49 to 05:58 CET,
`window-kwin-20261112T054906Z`), against the same clips on Umbriel
([above](#the-series-at-240-hz)):

| Clip | Order | Gaps off cadence, KWin with fifo | On Umbriel, without | Scanned out | GPU power over idle |
|---|---|---|---|---|---|
| cadence-23.976 | below | 256 of 1439 | 380 of 1439 | 0 of 709 | 2.51 W |
| cadence-23.976 | above | 191 of 1438 | 449 of 1439 | 671 of 696 | 3.70 W |
| cadence-25 | below | 0 of 1500 | 51 of 1500 | 0 of 725 | 3.16 W |
| cadence-25 | above | 2 of 1500 | 75 of 1500 | 704 of 724 | 3.82 W |
| cadence-59.94 | below | 840 of 3597 | 741 of 3597 | 0 of 1749 | 7.12 W |
| cadence-59.94 | above | 430 of 3597 | 749 of 3595 | 1739 of 1739 | 6.01 W |

- mpv's sync estimate stayed under 0.05 ms in every run.
- **Fifo mends the cadence where the display's rate over the clip's is far from a whole number**:
  at 25 fps (9.6 refreshes a frame), 0 and 2 gaps off of 1,500, against 51 and 75 on Umbriel.
- **It does not where that ratio is near a whole number**: 23.976 fps (10.011) and 59.94 fps
  (4.004) still show a frame one refresh short and the next one long, 12 % to 23 % of gaps. mpv's
  audio sync times each frame by the audio clock, and the frames due just at a refresh's edge fall
  on either side of it. Display sync is what holds such a cadence.
- The order above is scanned out by KWin as by Umbriel, and had fewer gaps off cadence here.
- The run of mpv alone on DP-3 gave no gap: its feedback, one frame in two, leaves none to measure.

### With VRR, under KWin

Measured on 2026-11-12 from 06:24 to 07:12 CET on the owner's desktop in a Plasma session, through
`owner-desktop`: DP-3, adaptive sync on "Automatic" (VRR for fullscreen windows, up to 240.02 Hz),
scale 1.45, KWin 6.8, fifo passed to mpv. Another session kept 0.5 to 8.6 hardware threads busy:
these are not quiet figures. The owner stopped the series once, at its fourth run; it was taken
up where it stopped. `window-20261112T062420Z`, kept outside the repository.

**VRR was on.** At a fixed rate, every interval between two presented frames is a whole number of
refreshes (100 % of them in the KWin runs at 240 Hz above); in this series 2 % to 12 % are, as
frames show between the fixed rate's refreshes.

KWin still sends a refresh period of 4.17 ms and no refresh counter, so refresh gaps say nothing
here. The bench's `timing` figure measures instead the time between two presented frames, `k`
frames apart, against `k` frame periods ([`presentation.rs`](../crates/aerith-bench/src/presentation.rs));
under fifo most spans are of two frames. Read with `aerith-bench presentation <log> <rate>`, from
whole logs, as this series predates the figure: Aerith's rows include about a second of each
clip's start and two seconds after the bench's last reading, which mpv alone's rows leave out, a
bias against Aerith. mpv's sync estimate stayed under 0.1 ms in every run.

| Clip | Player | Spans off by more than half a refresh (2.1 ms) | More than one (4.2 ms) | Worst | Scanned out | GPU power over idle | Other programs, threads |
|---|---|---|---|---|---|---|---|
| cadence-23.976 | mpv alone | 69 of 731 | 23 | 28.0 ms | 726 of 726 | 1.94 W | 5.4 |
| cadence-23.976 | aerith, below | 101 of 721 | 26 | 17.0 ms | 0 of 697 | 2.25 W | 4.9 |
| cadence-23.976 | aerith, above | 89 of 733 | 40 | 14.5 ms | 683 of 708 | 2.08 W | 6.28 |
| cadence-24 | mpv alone | 55 of 729 | 24 | 13.4 ms | 692 of 717 | 1.2 W | 8.58 |
| cadence-24 | aerith, below | 27 of 719 | 18 | 13.1 ms | 0 of 695 | 2.34 W | 1.56 |
| cadence-24 | aerith, above | 39 of 719 | 24 | 14.2 ms | 670 of 695 | 4.53 W | 0.51 |
| cadence-25 | mpv alone | 69 of 794 | 34 | 13.4 ms | 789 of 789 | 2.56 W | 3.87 |
| cadence-25 | aerith, below | 70 of 751 | 41 | 23.3 ms | 0 of 724 | 2.28 W | 3.34 |
| cadence-25 | aerith, above | 113 of 760 | 48 | 22.0 ms | 710 of 734 | 2.11 W | 6.56 |
| cadence-29.97 | mpv alone | 107 of 897 | 44 | 22.7 ms | 891 of 891 | 2.88 W | 5.57 |
| cadence-29.97 | aerith, below | 113 of 899 | 42 | 11.7 ms | 0 of 869 | 3.13 W | 5.56 |
| cadence-29.97 | aerith, above | 140 of 901 | 27 | 15.0 ms | 871 of 871 | 2.98 W | 5.89 |
| cadence-50 | mpv alone | 99 of 1486 | 11 | 9.9 ms | 1475 of 1475 | 4.72 W | 5.34 |
| cadence-50 | aerith, below | 132 of 1508 | 24 | 16.1 ms | 0 of 1457 | 4.18 W | 7.01 |
| cadence-50 | aerith, above | 281 of 1516 | 140 | 35.9 ms | 1467 of 1467 | 4.62 W | 8.34 |
| cadence-59.94 | mpv alone | 204 of 1817 | 91 | 30.8 ms | 1755 of 1755 | 5.03 W | 5.66 |
| cadence-59.94 | aerith, below | 150 of 1801 | 44 | 16.6 ms | 0 of 1739 | 6.65 W | 5.9 |
| cadence-59.94 | aerith, above | 118 of 1798 | 31 | 13.4 ms | 1738 of 1738 | 5.53 W | 5.2 |
| cadence-60 | mpv alone | 178 of 1764 | 12 | 15.1 ms | 1755 of 1755 | 5.15 W | 4.73 |
| cadence-60 | aerith, below | 173 of 1802 | 38 | 16.4 ms | 0 of 1741 | 6.18 W | 5.55 |
| cadence-60 | aerith, above | 120 of 1821 | 19 | 10.1 ms | 1761 of 1761 | 5.53 W | 8.06 |
| sdr-2160p-hevc10-eac3 | mpv alone | 227 of 679 | 115 | 28.0 ms | 671 of 671 | 2.94 W | 5.54 |
| sdr-2160p-hevc10-eac3 | aerith, below | 46 of 660 | 30 | 14.8 ms | 0 of 636 | 2.82 W | 2.33 |
| sdr-2160p-hevc10-eac3 | aerith, above | 27 of 659 | 24 | 12.0 ms | 611 of 636 | 3.26 W | 1.89 |

- **No player keeps an even cadence under VRR here.** 4 % to 19 % of Aerith's spans, and 6 % to
  33 % of mpv alone's, lie more than half a refresh from their frame periods; up to 36 ms for one.
  Aerith does about as well as mpv alone. Under audio sync mpv times each frame itself, and the
  compositor shows it when it comes; display sync under VRR is not measured.
- KWin scanned mpv alone out too, in its own fullscreen window, and the order above; never the
  order below.
- The figures are under another session's load, which disturbs timing: to take again on a quiet
  machine before they count against the floor.

Umbriel's configuration asks for VRR on DP-3 ("always"), yet every Umbriel run before the series
below presented its frames a whole number of 240.016 Hz refreshes apart, as at a fixed rate. The owner
turned VRR on for the series below, and the intervals then varied.

### With VRR, under Umbriel

Measured on 2026-11-12 from 08:46 to 09:31 CET on the owner's desktop under Umbriel, through
`owner-desktop`, by `BENCH_SECONDS=60 scripts/bench-window.sh ... cadence sync`: DP-3 with VRR up
to 240.016 Hz, scale 1.5. A 10 s run of mpv alone checked VRR first: 8 % of its intervals a whole
number of refreshes. Other sessions kept up to 15 hardware threads busy in some runs (last
column); the owner chose to measure all the same. The figures are each run's `result.json`, over
the bench's own window: from 2 s after the clip's first frame to the last reading.
`window-20261112T074630Z`, kept outside the repository.

**Under audio sync:**

| Clip | Player | Spans off by more than half a refresh | More than one | Worst | Sync, max / p99 (mpv's estimate) | Scanned out | GPU power over idle | Other programs, threads |
|---|---|---|---|---|---|---|---|---|
| cadence-23.976 | mpv alone | 372 of 1371 | 52 | 9.5 ms | 0.03 / 0.03 ms | 0 of 1372 | 3.17 W | 14.9 |
| cadence-23.976 | aerith, below | 285 of 1344 | 46 | 18.3 ms | 0.04 / 0.04 ms | 0 of 1345 | 2.92 W | 12.16 |
| cadence-23.976 | aerith, above | 54 of 1384 | 32 | 36.3 ms | 0.01 / 0.01 ms | 1160 of 1385 | 3.88 W | 3.14 |
| cadence-24 | mpv alone | 2 of 1388 | 0 | 3.2 ms | 0.01 / 0.01 ms | 0 of 1389 | 3.26 W | 0.16 |
| cadence-24 | aerith, below | 2 of 1389 | 1 | 4.3 ms | 0.01 / 0.01 ms | 0 of 1390 | 2.61 W | 0.1 |
| cadence-24 | aerith, above | 39 of 1386 | 8 | 15.5 ms | 0.04 / 0.04 ms | 1383 of 1387 | 2.91 W | 2.95 |
| cadence-25 | mpv alone | 41 of 1444 | 25 | 12.2 ms | 0.01 / 0.01 ms | 0 of 1445 | 3.57 W | 0.26 |
| cadence-25 | aerith, below | 128 of 1435 | 16 | 11.6 ms | 0.03 / 0.03 ms | 0 of 1436 | 2.53 W | 8.95 |
| cadence-25 | aerith, above | 69 of 1445 | 15 | 6.3 ms | 0.01 / 0.01 ms | 1442 of 1446 | 2.68 W | 10.35 |
| cadence-29.97 | mpv alone | 170 of 1732 | 11 | 14.4 ms | 0.01 / 0.01 ms | 0 of 1733 | 3.04 W | 9.36 |
| cadence-29.97 | aerith, below | 520 of 1654 | 141 | 19.8 ms | 0.02 / 0.02 ms | 0 of 1655 | 4.53 W | 14.22 |
| cadence-29.97 | aerith, above | 76 of 1731 | 18 | 23.9 ms | 0.03 / 0.03 ms | 1730 of 1732 | 2.5 W | 5.43 |
| cadence-50 | mpv alone | 171 of 2890 | 55 | 15.6 ms | 0.01 / 0.01 ms | 0 of 2891 | 5.25 W | 4.91 |
| cadence-50 | aerith, below | 284 of 2870 | 76 | 15.6 ms | 0.01 / 0.01 ms | 0 of 2871 | 6.45 W | 7.81 |
| cadence-50 | aerith, above | 380 of 2887 | 81 | 16.4 ms | 0.01 / 0.01 ms | 2588 of 2888 | 5.96 W | 5.33 |
| cadence-59.94 | mpv alone | 234 of 3464 | 29 | 39.6 ms | 0.01 / 0.01 ms | 0 of 3465 | 6.0 W | 10.53 |
| cadence-59.94 | aerith, below | 289 of 3435 | 54 | 9.3 ms | 0.01 / 0.01 ms | 0 of 3436 | 6.96 W | 6.04 |
| cadence-59.94 | aerith, above | 160 of 3467 | 21 | 16.7 ms | 0.01 / 0.01 ms | 3462 of 3468 | 6.19 W | 5.4 |
| cadence-60 | mpv alone | 102 of 3467 | 44 | 29.2 ms | 0.02 / 0.02 ms | 0 of 3468 | 7.09 W | 3.13 |
| cadence-60 | aerith, below | 15 of 3473 | 6 | 20.7 ms | 0.01 / 0.01 ms | 0 of 3474 | 7.41 W | 2.07 |
| cadence-60 | aerith, above | 138 of 3464 | 41 | 14.8 ms | 0.01 / 0.01 ms | 3452 of 3465 | 6.1 W | 3.74 |
| sdr | mpv alone | 21 of 1267 | 5 | 10.6 ms | 0.01 / 0.01 ms | 0 of 1268 | 4.21 W | 2.53 |
| sdr | aerith, below | 138 of 1238 | 46 | 20.0 ms | 0.03 / 0.03 ms | 0 of 1239 | 3.39 W | 5.07 |
| sdr | aerith, above | 114 of 1252 | 54 | 42.9 ms | 0.01 / 0.01 ms | 402 of 1253 | 4.69 W | 5.14 |

**Under display sync** (`video-sync=display-resample`). mpv presents at every refresh, so the
spans say nothing of the picture's cadence; mpv's own counters do:

| Clip | Player | Presents a second | Delayed / mistimed (mpv) | Sync, max (mpv's estimate) | Scanned out | GPU power over idle | Other programs, threads |
|---|---|---|---|---|---|---|---|
| cadence-23.976 | mpv alone | 241.188 | 238.0 / 47.0 | 25.73 ms | 0 of 13657 | 11.67 W | 5.64 |
| cadence-23.976 | aerith, below | 252.438 | 572.0 / 140.0 | 28.45 ms | 0 of 13648 | 14.19 W | 5.79 |
| cadence-23.976 | aerith, above | 253.982 | 681.0 / 162.0 | 28.45 ms | 13701 of 13746 | 10.44 W | 6.68 |
| cadence-25 | mpv alone | 246.541 | 415.0 / 78.0 | 22.96 ms | 0 of 13560 | 13.85 W | 7.0 |
| cadence-25 | aerith, below | 247.564 | 423.0 / 94.0 | 24.07 ms | 0 of 13742 | 14.79 W | 5.25 |
| cadence-25 | aerith, above | 243.939 | 284.0 / 45.0 | 25.03 ms | 12220 of 13608 | 11.48 W | 8.7 |
| cadence-59.94 | mpv alone | 240.958 | 497.0 / 46.0 | 18.65 ms | 0 of 13038 | 14.79 W | 14.45 |
| cadence-59.94 | aerith, below | 239.604 | 204.0 / 49.0 | 19.85 ms | 0 of 13546 | 17.1 W | 5.19 |
| cadence-59.94 | aerith, above | 240.745 | 181.0 / 30.0 | 17.5 ms | 13665 of 13714 | 12.97 W | 6.68 |

- **Under audio sync, VRR keeps the cadence on a quiet machine, but not always.** On `cadence-24`,
  with other programs at 0.1 to 0.2 threads, 2 spans of about 1,390 lay more than half a refresh
  off, for mpv alone and for Aerith in the order below alike. On `cadence-25`, as quiet for mpv
  alone (0.26 threads), 41 of 1,444. Where other sessions kept 9 to 15 threads busy, every player
  lost 10 % to 30 % of its spans: load disturbs these figures most.
- **Display sync defeats VRR.** mpv's presents at every refresh hold the display at its top rate:
  94 % to 100 % of intervals whole, against 1 % to 17 % under audio sync. mpv counted 181 to 681
  frames delayed and 30 to 162 mistimed a minute, against 0, its sync estimate reached 17.5 to
  28.5 ms, past the ±20 ms floor in 6 of 9 runs, and the GPU drew 6.6 to 12.3 W more.
- Under audio sync, mpv's sync estimate never passed 0.09 ms in any series of this milestone.
- Umbriel scanned the order above out under VRR too, but not always: 402 of 1,253 frames of the
  4K clip.

## M1.4c — Display sync by the display's rate

### The detection, calibrated on M1.4b's logs

Read on 2026-11-14 from the logs M1.4b's runs kept (`mpv-wayland.log`, every presentation mpv
received), outside the repository: Umbriel at 240 Hz (`window-20261112T014314Z`,
`window-finish240-20261112T041825Z`), KWin at 240 Hz (`window-20261112T051424Z`,
`window-kwin-20261112T054906Z`), KWin under VRR (`window-20261112T062420Z`) and Umbriel under VRR
(`window-20261112T074630Z`). An interval is the time between two frames presented one after the
other; it is whole within a tolerance of a whole number of the refresh period the compositor gave.
Runs under display sync have a column of their own: they cannot show VRR.

| | Fixed rate, audio sync | VRR, audio sync | VRR, display sync |
|---|---|---|---|
| Intervals within 0.05 refresh (208 µs), per run | 100 % | 2 % to 12 % | 98 % to 100 % |
| Intervals within 0.01 refresh (42 µs), per run | 100 % | 0 % to 2 % | 98 % to 100 % |
| Farthest from a whole number of refreshes | 6.1 µs | – | – |

The most whole intervals in any window of consecutive intervals, under VRR and audio sync, against
the fewest at a fixed rate (always every one):

| Window | Within 50 µs | Within 100 µs | Within 200 µs |
|---|---|---|---|
| 16 intervals | 25 % | 31 % | 44 % |
| 24 intervals | 17 % | 25 % | 38 % |

- Every presentation on both compositors carried `vsync`, `hw_clock` and `hw_completion`, and a
  refresh period of 4,166,388 ns, at a fixed rate and under VRR alike.
- No clip's rate made VRR look fixed: at 24 and 60 fps, whose periods are whole numbers of 240 Hz
  refreshes, VRR's intervals scatter as much as at 25 fps.
- From these, the relay's probe reads 24 intervals within 50 µs, half of them whole for a fixed
  rate ([ADR-0022](adr/0022-the-relay-tells-the-display-s-rate-when-the-player-asks.md)).

### Without a GPU, in `agent-headless`

On 2026-11-14, `aerith-testbed` with mpv on lavapipe, a 24 fps clip of mpv's own encoder at
1280 × 720, the order above, other sessions building beside it:

| Compositor | Presentations | Probe's verdict | Its time, from the first frame | mpv |
|---|---|---|---|---|
| sway, headless | no flag, no refresh period | none after 174 frames | – | audio sync throughout |
| KWin 6.8.2, virtual | `vsync`, `hw_clock`, 60 Hz | fixed rate | 1,307 ms | `video-sync=display-resample` at 2.6 s, then 60 presents a second |

`aerith-bench run aerith` under KWin's virtual backend, `cadence-24-1080p-h264-aac`, 10 s: the
probe told a fixed rate in 1,358 ms, and mpv's `display-sync-active` read true in every reading
from the second on (`display_sync.active_share` 1.0 over the 8 readings counted).

### The rule on the desktop at 240 Hz

Measured on 2026-11-14 from 06:25 to 06:54 CET on the owner's desktop under Umbriel, through
`owner-desktop`, by `BENCH_SECONDS=60 scripts/bench-window.sh <corpus> "<display>" rule`
(`1785788`): DP-3 at 240.016 Hz, a fixed rate (a 10 s run of mpv alone first: 100 % of its
intervals whole), scale 1.5. Aerith in the order above, under its rule; mpv alone under audio
sync and under display sync. 60 s a run, the first 2 s left out. The machine was quiet at the
start; other sessions' builds and tests then kept 0.2 to 12.5 hardware threads busy (last column):
**only the 23.976 fps runs, and the 24 fps runs but their last seconds, are quiet figures.**
`window-20261114T052527Z`, kept outside the repository.

Under display sync, the refresh gaps say nothing of the picture's cadence; mpv's delayed and
mistimed frames do. Every Aerith run told a fixed rate, and mpv's `display-sync-active` read true
in every reading counted (100 %).

| Clip | Player | Delayed / mistimed (mpv) | Sync, max | Scanned out | GPU power over idle | Other programs, threads |
|---|---|---|---|---|---|---|
| cadence-23.976 | mpv alone, audio sync | 0 / 0, 118 of 1,386 gaps off cadence | 0.01 ms | 0 of 1,387 | 1.84 W | 0.16 |
| cadence-23.976 | mpv alone, display sync | 9 / 1 | 4.52 ms | 0 of 13,892 | 9.64 W | 0.79 |
| cadence-23.976 | Aerith | 0 / 1 | 2.85 ms | 12,759 of 13,900 | 8.71 W | 0.39 |
| cadence-24 | mpv alone, display sync | 5 / 1 | 11.23 ms | 0 of 13,911 | 10.70 W | 0.52 |
| cadence-24 | Aerith | 12 / 2 | 22.37 ms | 10,895 of 13,890 | 9.54 W | 1.37 |
| cadence-25 | mpv alone, display sync | 65 / 9 | 17.49 ms | 0 of 13,823 | 12.24 W | 2.27 |
| cadence-25 | Aerith | 110 / 16 | 20.55 ms | 12,316 of 13,721 | 9.14 W | 4.13 |
| cadence-29.97 | mpv alone, display sync | 8 / 1 | 7.95 ms | 0 of 13,895 | 11.26 W | 2.59 |
| cadence-29.97 | Aerith | 71 / 14 | 22.05 ms | 9,526 of 13,835 | 12.27 W | 3.50 |
| cadence-50 | mpv alone, display sync | 9 / 6 | 13.18 ms | 0 of 13,876 | 14.21 W | 2.39 |
| cadence-50 | Aerith | 40 / 4 | 15.89 ms | 13,814 of 13,862 | 11.52 W | 4.76 |
| cadence-59.94 | mpv alone, display sync | 222 / 25 | 25.62 ms | 0 of 13,351 | 14.36 W | 6.40 |
| cadence-59.94 | Aerith | 298 / 55 | 20.25 ms | 13,282 of 13,332 | 12.10 W | 8.69 |
| cadence-60 | mpv alone, display sync | 44 / 7 | 19.43 ms | 0 of 13,852 | 14.07 W | 2.26 |
| cadence-60 | Aerith | 8 / 1 | 16.17 ms | 13,850 of 13,903 | 11.74 W | 2.42 |
| sdr-2160p | mpv alone, display sync | 115 / 13 | 19.60 ms | 0 of 12,463 | 12.26 W | 4.60 |
| sdr-2160p | Aerith | 37 / 10 | 19.36 ms | 12,602 of 12,642 | 10.07 W | 2.81 |

mpv alone under audio sync, the rule's alternative at a fixed rate, had 0 frames delayed and its
sync under 0.7 ms, but, from the series' results (one row above), 1 % (25 and 50 fps) to 37 %
(59.94 fps) of its refresh gaps off cadence, 13 % at 24 fps and 9 % at 23.976 fps on a quiet
machine, as at M1.4b.

- **The rule holds at a fixed rate.** Aerith told a fixed rate in 523 to 1,303 ms from the first
  frame, 0.52 s at 59.94 and 60 fps and about 1.3 s at 23.976 to 25 fps, and played under display
  sync from then on.
- **On a quiet machine, the floor holds**: on `cadence-23.976`, Aerith delayed no frame and its
  sync stayed within 2.85 ms, better than mpv alone under display sync (9 delayed, 4.52 ms).
- **Under load, both miss it alike.** The frames delayed fell almost all in seconds where other
  programs took 2 to 15 threads; mpv's sync estimate then reached up to 26 ms, past the ±20 ms
  floor in some runs of either player. The 24 fps run of Aerith delayed its 12 frames in its last
  seconds, as another session took up to 15 threads.
- Aerith's plane was scanned out for 69 % to 100 % of its frames; mpv alone, never. Aerith drew
  0.9 to 3.1 W less than mpv alone under display sync on every clip but 29.97 fps.
- The relay's thread woke 853 to 1,083 times a second, the window's thread 277 to 359.

### The rule on the desktop under VRR

Measured on 2026-11-14 from 06:59 to 07:28 CET, the same way, after the owner turned VRR on: DP-3
with VRR up to 240.016 Hz (a 10 s run of mpv alone first: 9 % of its intervals whole, the refresh
period given 4.1664 ms). A first attempt played on DP-2, where the focus was; it was stopped and
left out. Other sessions kept 0.8 to 15.5 hardware threads busy, by the owner's choice to measure
all the same: **only the runs with under about 2 threads (24 to 50 fps) are quiet figures**.
`window-20261114T055854Z`, kept outside the repository.

Under audio sync, the spans tell the cadence: the time between two presented frames against
their frame periods, more than half a refresh off ([`presentation.rs`](../crates/aerith-bench/src/presentation.rs)).
Every Aerith run told VRR, and mpv's `display-sync-active` read false in every reading (0 %).

| Clip | Player | Spans off by more than half a refresh | Delayed / mistimed (mpv) | Sync, max | Scanned out | GPU power over idle | Other programs, threads |
|---|---|---|---|---|---|---|---|
| cadence-23.976 | mpv alone, audio sync | 244 of 1,371 | 0 / 0 | 0.04 ms | 0 of 1,372 | 3.25 W | 7.33 |
| cadence-23.976 | mpv alone, display sync | – | 480 / 69 | 26.91 ms | 0 of 13,121 | 12.58 W | 8.89 |
| cadence-23.976 | Aerith | 223 of 1,361 | 0 / 0 | 0.03 ms | 1,357 of 1,362 | 3.00 W | 11.96 |
| cadence-24 | mpv alone, audio sync | 440 of 1,365 | 0 / 0 | 0.02 ms | 0 of 1,366 | 2.66 W | 15.48 |
| cadence-24 | mpv alone, display sync | – | 583 / 58 | 74.66 ms | 0 of 12,498 | 12.60 W | 13.80 |
| cadence-24 | Aerith | 29 of 1,387 | 0 / 0 | 0.01 ms | 1,229 of 1,388 | 2.95 W | 0.85 |
| cadence-25 | mpv alone, audio sync | 3 of 1,445 | 0 / 0 | 0.02 ms | 0 of 1,446 | 2.23 W | 0.79 |
| cadence-25 | mpv alone, display sync | – | 3 / 1 | 6.56 ms | 0 of 13,913 | 12.65 W | 1.44 |
| cadence-25 | Aerith | 34 of 1,438 | 0 / 0 | 0.02 ms | 1,383 of 1,439 | 2.45 W | 2.22 |
| cadence-29.97 | mpv alone, audio sync | 10 of 1,733 | 0 / 0 | 0.01 ms | 0 of 1,734 | 3.07 W | 1.74 |
| cadence-29.97 | mpv alone, display sync | – | 0 / 1 | 7.21 ms | 0 of 13,913 | 11.90 W | 1.55 |
| cadence-29.97 | Aerith | 4 of 1,735 | 0 / 0 | 0.01 ms | 1,730 of 1,736 | 3.41 W | 1.52 |
| cadence-50 | mpv alone, audio sync | 0 of 2,896 | 0 / 0 | 0.01 ms | 0 of 2,897 | 5.53 W | 1.62 |
| cadence-50 | mpv alone, display sync | – | 15 / 1 | 8.02 ms | 0 of 13,897 | 12.65 W | 1.67 |
| cadence-50 | Aerith | 20 of 2,893 | 0 / 0 | 0.01 ms | 2,884 of 2,894 | 5.24 W | 1.57 |
| cadence-59.94 | mpv alone, audio sync | 110 of 3,466 | 0 / 0 | 0.01 ms | 0 of 3,467 | 5.95 W | 4.24 |
| cadence-59.94 | mpv alone, display sync | – | 26 / 3 | 16.58 ms | 0 of 13,905 | 13.58 W | 3.56 |
| cadence-59.94 | Aerith | 732 of 3,285 | 0 / 0 | 0.02 ms | 154 of 3,286 | 6.75 W | 9.85 |
| cadence-60 | mpv alone, audio sync | 306 of 3,462 | 0 / 0 | 0.02 ms | 0 of 3,463 | 6.49 W | 5.93 |
| cadence-60 | mpv alone, display sync | – | 237 / 22 | 20.91 ms | 0 of 13,567 | 13.89 W | 5.84 |
| cadence-60 | Aerith | 323 of 3,456 | 0 / 0 | 0.02 ms | 3,444 of 3,457 | 5.50 W | 6.67 |
| sdr-2160p | mpv alone, audio sync | 219 of 1,260 | 0 / 0 | 0.02 ms | 0 of 1,261 | 3.56 W | 7.13 |
| sdr-2160p | mpv alone, display sync | – | 384 / 35 | 26.42 ms | 0 of 11,738 | 11.71 W | 6.33 |
| sdr-2160p | Aerith | 121 of 1,263 | 0 / 0 | 0.02 ms | 1,264 of 1,264 | 3.34 W | 10.74 |

- **The rule holds under VRR.** Aerith told VRR in 521 to 1,346 ms and stayed under audio sync,
  which cost it 7 to 10 W less than display sync and kept its sync within 0.03 ms.
- **On the quiet runs, the cadence holds**: 0.2 % to 2.4 % of spans off for Aerith (24 to 50
  fps), against 0 % to 0.6 % for mpv alone on its quiet runs (25 to 50 fps): Aerith is a little
  behind mpv alone at 25 and 50 fps (34 and 20 spans against 3 and 0), ahead at 29.97 fps. Each figure is one run.
- **Under load, the spans grow for every player under audio sync** (3 % to 32 %), and display
  sync under VRR is worse still: up to 583 frames delayed a minute and sync up to 75 ms where
  other programs took 4 to 14 threads. That is what the rule spares Aerith.
- Aerith's plane was scanned out for 88 % to 100 % of its frames but at 59.94 fps (5 %), a loaded run
  (9.85 threads).
- The relay's thread woke 114 to 334 times a second, the window's 57 to 167.
