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
