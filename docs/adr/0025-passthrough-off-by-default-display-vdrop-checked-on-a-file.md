# 0025. Passthrough off by default, frames dropped or repeated under it, checked on a file

- Status: Accepted; amends 0021 (the sync under passthrough)
- Date: 2026-11-14

## Context

The quality bar's passthrough floor asks that the bitstream sent be identical to the source's,
for E-AC3, TrueHD, DTS and DTS-HD ([ROADMAP](../ROADMAP.md#budgets)). There is no audio-video
receiver, and the owner's sound card is never touched without the owner's go. Read on 2026-11-14
in the pinned sources (mpv 0.42.0, PipeWire 1.6.11, ALSA lib 1.2.16.1, FFmpeg 9.0.2):

- **mpv wraps the bitstream itself**, in IEC 61937 bursts, with FFmpeg's `spdif` muxer
  (`audio/decode/ad_spdif.c`): E-AC3 and TrueHD at 192 kHz, DTS-HD Master Audio on 8 channels at
  192 kHz, DTS core at 48 kHz. Nothing changes a byte after it: volume, resampling and channel
  options skip non-PCM formats.
- **mpv falls back to PCM without a word** when the audio output refuses the format
  (`player/audio.c`). Only `audio-out-params/format` (`spdif-truehd` and the like) tells what was
  sent.
- **Passthrough turns display sync off.** Under `video-sync=display-resample`, mpv returns before
  timing frames by the display when passthrough is active (`player/video.c`): the bitstream cannot
  be resampled. The player then runs under audio sync, which misses the cadence floor at a fixed
  rate on 23.976 and 59.94 fps films (12 % to 31 % of gaps off,
  [ADR-0021](0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md)).
  `display-vdrop` keeps timing frames by the display under passthrough, dropping or repeating a
  frame when the two clocks drift apart.
- **A Linux desktop passes the bitstream through PipeWire**, as `audio/iec958` with its codec, to
  an ALSA device whose name begins with `hdmi` or `iec958` and whose `iec958.codecs` lists it
  (`spa/plugins/alsa/alsa-pcm.c`). PipeWire's null sink takes raw PCM only.
- **ALSA's `file` plugin over its `null` device** writes every frame it is given to a file,
  unchanged, with no sound card. Its `null` device is not paced: a client writes as fast as it
  decodes.
- **FFmpeg's `spdif` demuxer cannot read TrueHD or DTS-HD back.** And as mpv wraps with FFmpeg's
  own muxer, comparing a capture with FFmpeg's output proves nothing about the wrapping.

## Decision

The owner's decisions of 2026-11-14.

**Passthrough is a setting of the player's, off by default**, as in mpv, Kodi and Jellyfin
Desktop: the codecs to pass through, which only the person knows their receiver takes. The
player's report says what mpv actually sent, from `audio-out-params`, so that a fallback to PCM
shows.

**Under passthrough at a fixed rate, the player uses `display-vdrop`** in place of
`display-resample`, as Kodi does: each frame on a whole number of refreshes, a frame dropped or
repeated when the clocks drift, each counted and shown with its cause. Under VRR, audio sync stays
the rule.

**The check captures the bitstream on a file, two ways, with no sound device:**

- mpv's ALSA output into an ALSA `file` device over `null`, declared in the run's own ALSA
  configuration: the byte check, fast, run without a window;
- mpv's PipeWire output into the bench's own PipeWire, whose sink is an ALSA device named `hdmi`
  that is the same `file` device: the path of a Linux desktop, paced by PipeWire, on which
  `display-vdrop` is measured. A run must first confirm that PipeWire takes it; if it does not,
  the owner is asked again.

**The bench reads the capture itself.** A parser of its own checks each burst's preamble, takes
out the payload of each E-AC3, TrueHD, DTS and DTS-HD burst (TrueHD's from its MAT frames), and
compares it byte for byte with the source's packets, as FFmpeg copies them out of the file.

## Alternatives considered

- **Audio sync under passthrough**, mpv's default: nothing to build, but the cadence floor missed
  at a fixed rate on the commonest films.
- **ALSA alone**: simpler, but PipeWire's path, the one people use, checked by reading only, and
  no pacing to measure sync with passthrough.
- **mpv's file output** (`--ao=pcm`): the simplest, but it skips the audio output's negotiation,
  where passthrough is most often lost.
- **A loopback sound card or a real HDMI output**: closest to the hardware, but root to load a
  module, or the owner's sound card.
- **Comparing with FFmpeg's `spdif` muxer**: one command, but mpv wraps with the same code.

## Consequences

- Under passthrough, a film at a fixed rate shows a dropped or repeated frame now and then, by
  design; the bench counts them apart from frames lost to load.
- The parser of IEC 61937 is untrusted-input code in the bench: bounded and fuzzed like the rest.
- The capture through PipeWire depends on PipeWire treating a device by its name; a PipeWire that
  changes it breaks the check, not Aerith.
- Atmos inside E-AC3 is not checked: the corpus has no source for it
  ([ADR-0012](0012-the-test-corpus.md)).
