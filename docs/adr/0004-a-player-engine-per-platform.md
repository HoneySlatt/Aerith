# 0004. A player engine per platform: mpv, Media3, AVFoundation

- Status: Accepted
- Date: 2026-11-11

## Context

Reference image and sound depend on what each system lets a player reach
([survey](../survey.md#video-on-android) and [Apple](../survey.md#apple-platforms)):

- **Android:** HDR needs a SurfaceView (a TextureView converts HDR to SDR from Android 13). Media3
  handles Dolby Vision profiles 8 and 4 through the HEVC base layer and passes E-AC3-JOC, DTS-HD and
  TrueHD through. Kodi, Just Player and Nuvio's TV app rely on Media3 or MediaCodec.
- **Apple:** only AVPlayer obtains from the system true Dolby Vision, Atmos, the TV's switch to the
  content's range and rate, and AirPlay. AVPlayer does not open Matroska; Infuse-class players and
  AetherEngine demux with FFmpeg and remux on the fly to HLS for AVPlayer. mpv on macOS draws only
  in OpenGL and cannot pass TrueHD or DTS-HD through; MPVKit calls itself a learning project.

## Decision

**Three engines behind the one `aerith-player` interface:**

- **mpv** on Linux and Windows ([ADR-0003](0003-mpv-in-its-own-process.md));
- **Media3** on Android phones and Android TV, in a SurfaceView below the interface, through a thin
  Kotlin layer;
- **AVFoundation** on macOS, iPhone and Apple TV: FFmpeg (LGPL build) demuxes and remuxes to HLS for
  AVPlayer; codecs Apple does not decode fall back to a software decoder.

**The interface is the contract**: open, play, pause, seek, tracks, subtitles, speed, the position
and its clock, what the engine actually did (decoder, colour, audio path), and errors. What an
engine cannot do is reported, never faked.

**Whether the Apple engine is Aerith's own or a library** (AetherEngine, under LGPL with an App
Store exception) is decided at the Apple stage, as any dependency.

## Alternatives considered

- **mpv everywhere, through mpv-android and MPVKit**: one engine, but no Dolby Vision sent to the TV
  and no Atmos on Apple, and higher power; below the owner's bar.
- **AVFoundation on iPhone and Apple TV only, mpv on macOS**: libplacebo kept on the Mac, but mpv's
  weakest platform, and two engines to follow on Apple.

## Consequences

- Three engines to write, test and keep in step; their shared tests run against the interface.
- Complex ASS subtitles are drawn by Aerith on Android and Apple, where the engine does not.
- The interface sits above a video plane it cannot read on Android, Windows and Apple: the look is
  designed for it ([ADR-0003](0003-mpv-in-its-own-process.md#consequences)).
