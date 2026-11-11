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
([below](#against-m12)).

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
  is not in the repository; `time_to_the_first_frame` keeps the comparison of the whole sets. Why, and
  whether it holds with a real video output, is not known yet
  ([PLAN](PLAN.md#m13--mpv-driven-through-aerith-player)).
- With its defaults, mpv alone gets faster run after run (243 to 80 ms), and with the player's
  options it does not. The first run of each series is the slowest.

### Against M1.2

M1.2's figures for mpv alone, on the same clip: 617 ms from a file and 690 ms as a stream (medians,
under a load test), 390 to 445 ms on other clips at rest. They are not comparable with the figures
above: M1.2's mpv drew fullscreen through gpu-next with hardware decoding, and its first frame
includes the video output's start and the compilation of its shaders, which a discarded picture
skips. The comparison on the desktop, through `--window`, waits for the owner's go, and is taken
again once M1.2 is complete ([PLAN](PLAN.md#m13--mpv-driven-through-aerith-player)).

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
