# 0003. On the desktop, mpv in its own process, its image inside the window

- Status: Accepted
- Date: 2026-11-11

## Context

Aerith wants mpv's image (gpu-next on libplacebo, HDR, Dolby Vision profiles 5, 7 and 8, display
sync, ASS subtitles, passthrough) inside its own window, under its own interface. What exists
([survey](../survey.md#video-on-the-desktop)):

- libmpv's render API draws only through the old `vo_gpu`, in OpenGL, software or D3D11: no
  gpu-next, no HDR output. A Vulkan render API was proposed and not merged; a maintainer described
  how one could be accepted (port gpu-next to mpv's render primitives, validate, then expose it).
- `--wid` embeds mpv's own window on X11, Windows and Android, not on Wayland.
- Wayland's colour-management protocol describes each surface on its own, so a video surface can be
  HDR under an SDR interface. KWin, Mutter, Hyprland and wlroots 0.20 (the owner's compositor,
  Umbriel) implement it.
- Writing a player of our own (FFmpeg and libplacebo in the interface's GPU device) means rewriting
  what mpv carries: buffering, seeking, the audio clock, display sync and cadence, passthrough and
  hardware decoding on each system, subtitles. libplacebo has no Direct3D 12 backend, which wgpu
  uses on Windows.
- uwui requires decoders written in C to run in a process of their own (SlimeEngine ADR-0190).
- Jellyfin Desktop, Swiftfin, Streamyfin, Findroid and Stremio's desktop shells all play through
  mpv.

## Decision

**On Linux and Windows, Aerith plays through mpv, unmodified, in a process of its own.** A crash or a
hostile file in the decoders reaches neither the interface nor the tokens Aerith holds.

**Its image is placed inside Aerith's window:**

- **Linux:** Aerith acts as a small Wayland compositor for mpv, and places mpv's buffers in a
  subsurface below the interface's surface, with their colour description. The same buffers can be
  imported into the interface's GPU device, so glass over the video can blur it.
- **Windows:** mpv draws into a child window under an interface composed through DirectComposition,
  as Stremio's Windows shell does.

**The player is reached through one interface, `aerith-player`**, that the rest of Aerith uses on
every platform ([ADR-0004](0004-a-player-engine-per-platform.md)).

**Stage 1 proves it by measurement** on Linux and Windows before anything is built on it: no frame
dropped in 30 minutes at 60 Hz, 240 Hz and with VRR; sound and image within ±20 ms; HDR checked on
the owner's monitor; memory and power against mpv alone on the lowest laptop
([PLAN](../PLAN.md)). A criterion that fails reopens this decision.

**Later, Aerith may contribute to mpv** the Vulkan render API its maintainers described, which would
make the video a texture of the interface. A player of Aerith's own is written only if mpv fails a
named, measured criterion.

## Alternatives considered

- **A player of our own on FFmpeg and libplacebo**: the video a true element of the interface
  everywhere, but years of work mpv already did, no Direct3D 12 in libplacebo, the decoders in the
  interface's process, and a low-level link to wgpu, which ships several major versions a year.
- **Contributing a Vulkan render API to mpv first**: the best of both if accepted, but nothing would
  work until it is.
- **mpv in a window of its own**, as the prototype did: the reason the owner restarted.

## Consequences

- Glass over the video blurs it on Linux; on Windows (a child window) and Android (a SurfaceView) the
  interface cannot read the video's pixels. The look is designed for that strictest case.
- Aerith's Wayland compositor for mpv is new work, its risk measured first.
- During a morph between a poster and the playing video, Windows shows a still frame.
- mpv's GPL build ships beside Aerith ([ADR-0002](0002-mpl-2-0.md)).
