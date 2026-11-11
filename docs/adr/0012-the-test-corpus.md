# 0012. The test corpus: generated, with a few clips fetched

- Status: Accepted
- Date: 2026-11-11

## Context

The bench plays a corpus of clips to measure the rivals and, later, Aerith
([ADR-0008](0008-the-quality-bar.md)). The corpus must cover what Stage 1 checks: SDR, HDR10, HLG,
HDR10+, Dolby Vision profiles 5, 7 and 8; 23.976 to 60 frames a second; E-AC3, TrueHD, DTS and
DTS-HD for passthrough. Each clip needs a source and a licence. Nothing of the owner's library is
ever played.

What free tools make, checked on 2026-11-11 with FFmpeg 9.0.2, x265 4.2, dovi_tool 2.3.4 and
hdr10plus_tool 1.7.2:

- SDR, HDR10, HLG and HDR10+ pictures, at any rate and length;
- Dolby Vision profiles 5, 8.1 and 8.4 on one layer. Profile 5's metadata goes on a BT.2020
  picture, not on true IPT-PQ-c2: the clip checks what a player detects, not how it looks;
- E-AC3, TrueHD and DTS core sound (the last two through FFmpeg's experimental encoders).

What they do not make: Dolby Vision profile 7 (two layers), DTS-HD Master Audio, DTS:X, E-AC3 with
Atmos (JOC), TrueHD with Atmos. No open encoder exists for them
([survey](../survey.md#test-clips)).

Where those exist:

- FFmpeg's test suite (FATE) holds tiny samples of each but E-AC3 JOC, from a few KB to 6.3 MB. It
  states no terms: they are test fixtures, fetched, not published again.
- Vendor demonstrations (Dolby, DTS) cover everything, Atmos included, under "fair use" claims and
  behind links that disappear.
- Netflix Open Content and the Blender films are real footage under CC BY.

## Decision

**The corpus is made by a script, `scripts/corpus.sh`, and never committed.** It writes every clip
under a temporary name and keeps it once whole. The repository holds the script and the list of
clips in it.

**Every clip that free tools can make is generated.** The clips are reproducible, carry no licence
question, and their content is made for measuring: a moving pattern, the frame number and time
drawn in, a white band for one frame at each beat and a 20 ms beep at the same instant on every
channel. A beat lasts a whole number of frames, so the flash and the beep stay aligned for the
whole clip:

- seven 30-minute cadence clips, from 23.976 to 60 frames a second, in 1080p H.264 with AAC;
- seven 60-second 4K HEVC Main 10 colour clips with E-AC3 5.1: SDR, HDR10, HLG, HDR10+, and Dolby
  Vision 5, 8.1 and 8.4;
- a five-minute 4K HDR10 clip at a remux's bitrate (about 80 Mbit/s, with grain) and TrueHD, for
  memory and power;
- three passthrough clips: E-AC3, TrueHD and DTS, each 5.1.

**What cannot be generated is fetched and pinned by SHA-256.**

- Dolby Vision profile 7, at 1080p and 4K, DTS-HD Master Audio 7.1, DTS:X and TrueHD Atmos come
  from FFmpeg's test suite, about 8 MB in all. They are under a second long, except the DTS-HD MA
  sample (12.5 s): enough to check what a player detects and what it sends, not to listen. The
  sound samples are muxed, intact, under a generated picture.
- Three minutes of Netflix's *Cosmos Laundromat* (CC BY 4.0) give real footage, to compare the
  image. Netflix publishes it as 8-bit H.264, its pixels PQ in P3, but with no colour tags. The
  script converts it to HDR10: BT.2020, PQ, 10-bit, the mastering display P3. Its 8-bit origin may
  band in smooth gradients.

**The script checks every clip** against what its name promises, as FFmpeg reads it: codec, depth,
transfer, primaries, Dolby Vision profile and layers, HDR10+ metadata, sound codec, profile and
channels. A clip that fails stops the script.

## Alternatives considered

- **Everything generated, nothing fetched**: no download at all, but no profile 7, no DTS-HD and no
  Atmos. The milestone asks for profile 7 and DTS-HD, and DTS-HD passthrough could not be checked.
- **Generated, plus the fetched samples, without real footage**: lighter by 730 MB, but tone mapping
  and scaling are judged badly on test patterns.
- **Vendor demonstrations**: complete, Atmos included, but under "fair use" only, without a right to
  publish them again, and behind links that disappear: the bench would not be reproducible.
- **Jellyfin's test videos** (CC BY-SA 4.0): real HDR10 and Dolby Vision 5 and 8, but nothing the
  generator cannot make, and no profile 7, HLG or HD sound.

## Consequences

- The full corpus takes about 1.5 hours to make on the high tier and 11 GB of disk. `--quick`
  makes a few seconds of each clip, to try the bench; those clips are not for measuring.
- E-AC3 with Atmos (JOC) has no source: Atmos detection is not checked in Stage 1. Atmos on Apple
  TV is Stage 7's.
- The fetched samples' terms are unstated. They are used on the owner's machines only, never
  published. If FFmpeg removes them, the pins fail and the corpus needs another source.
- The generated pictures are synthetic: they show cadence, sync and the colour description, not
  how a real picture looks. That is the real clip's job.
- The beat makes a capture-based check of sync possible later (a camera or an audio loopback);
  Stage 1 reads the engines' own figures ([ADR-0013](0013-the-bench.md)).
