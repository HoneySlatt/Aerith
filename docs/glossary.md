# Glossary

The terms Aerith coins or uses in a sense of its own.

- **`aerith-player`**: the one interface through which the rest of Aerith drives playback, whatever
  the engine ([ADR-0004](adr/0004-a-player-engine-per-platform.md)).
- **Engine**: what decodes and presents the video on a platform: mpv, Media3 or AVFoundation.
- **Video plane**: the surface or system view where the engine presents the video, below the
  interface's surface, which the interface may not be able to read.
- **Bench**: the tool of Stage 1, `aerith-bench`, and its scripts, that play the corpus and measure
  the budgets ([ADR-0013](adr/0013-the-bench.md)).
- **Corpus**: the clips the bench plays, each with its source and licence
  ([ADR-0012](adr/0012-the-test-corpus.md)).
- **Beat**: in a generated clip, the frame that shows a white band and the instant of a 20 ms beep,
  once every round(fps) frames, so that sound and image can be checked against each other.
- **Run**: one rival playing one clip under the bench, which writes one `result.json`.
- **Silent sound server**: the bench's own PipeWire, whose only output is a null sink; the rivals
  play into it and see no sound device ([ADR-0013](adr/0013-the-bench.md)).
- **Floor** and **lead**: an absolute budget that gates a milestone, and a margin over the best rival
  that becomes debt when missed ([ADR-0008](adr/0008-the-quality-bar.md)).
- **Store build** and **full build**: the two variants of Aerith; the first without torrents and
  JavaScript plugins ([ADR-0007](adr/0007-a-neutral-client-in-two-variants.md)).
- **Couch** and **desk**: the interface across a room driven by a remote or a gamepad, and at a desk
  driven by a mouse and a keyboard.
- **Passthrough**: compressed audio (E-AC3, TrueHD, DTS) sent intact to a receiver, which decodes it.
