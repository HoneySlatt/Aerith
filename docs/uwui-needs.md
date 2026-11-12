# What Aerith needs of uwui

Aerith's interface is written on uwui, as a later client
([ADR-0005](adr/0005-the-interface-on-uwui-a-later-client.md)). This file lists what Aerith asks of
uwui, for the owner to carry into uwui's sessions. Aerith never changes uwui or SlimeEngine from
here. uwui takes a need only if it is general; a need that serves only Aerith stays in Aerith.

Each need is marked:

- **Indispensable** when the interface would lose a function, an approved look or a row of the
  quality bar without it; each names the stage that first needs it.
- **Wished** otherwise.

The last column says what uwui holds today, read against SlimeEngine's ADR-0181 and ADR-0190 and
uwui's ROADMAP on 2026-11-11: *Yes*, *Planned* (and where), *Out of scope*, or *New*; a need added
since, not yet read against them, says so.

## The video plane

The player draws in another process or in a system view
([ADR-0003](adr/0003-mpv-in-its-own-process.md), [ADR-0004](adr/0004-a-player-engine-per-platform.md)).
The interface sits above it and, on Android, Windows and Apple, cannot read its pixels.

| Need | Mark | uwui today |
|---|---|---|
| A region of the window left open over a video plane that the host places below the interface's surface, moved and resized with the interface in the same frame | Indispensable, Stage 3 | New |
| On Wayland, a surface from another process (mpv's buffers through Aerith's relay, [ADR-0015](adr/0015-a-relay-for-mpvs-wayland-surface.md)) composed as a subsurface; filed jointly with Magnolia's plugin windows | Indispensable, Stage 3 | New |
| The same buffers imported as a texture an element shows (dma-buf), so glass over the video can blur it where the platform allows | Wished, Stage 3 | Planned: textures updated at each frame, first stage |
| Glass that degrades to a tint without blur over a plane it cannot read, by rule of the theme | Indispensable, Stage 3 | New |
| A colour description per surface: an SDR interface over an HDR plane on Wayland, Windows, Android and Apple | Indispensable, Stage 3 | Planned: HDR, before 1.0 (ROADMAP) or from Stage 6d (ADR-0190); the documents disagree |
| Presents timed to the content's cadence under VRR (`commit-timing`), for the interface during playback | Wished, Stage 3 | New |
| A morph between an element (a poster) and the video plane's region | Wished, Stage 3 | Planned: motion, first stage |
| Content drawn into a surface of uwui's own above the plane, paced by that surface's frame callbacks, never the window's root, its input passing through to the window: the video covers a black root, so that the compositor scans it out ([ADR-0019](adr/0019-the-video-above-a-black-root.md)) | Indispensable, Stage 3 | New (added at M1.4) |
| The window's *suspended* state given to the plane's host, so that a hidden window stops the engine drawing | Wished, Stage 3 | New (added at M1.4) |
| The window's own surface, its buffer and its scaling, in the interface's hands, for the black root under the video: winit 0.30 puts a viewport of its own on it, so the testbed reuses winit's viewport, and Umbriel offers no single-pixel buffer ([ADR-0017](adr/0017-a-test-interface-on-winit-and-wgpu.md), [ADR-0019](adr/0019-the-video-above-a-black-root.md)) | Indispensable, Stage 3 | New (added at M1.4) |
| An event queue of the application's own woken by the interface's loop: winit 0.30 calls the application only for its own events, so the testbed re-checks its frame callbacks on a deadline | Wished, Stage 3 | New (added at M1.4) |

## The couch and the remote

| Need | Mark | uwui today |
|---|---|---|
| A couch density for a screen across the room | Indispensable, Stage 3 | Yes (ADR-0190) |
| Focus moved by arrows to the nearest element, with a clear focused state, driven by a gamepad | Indispensable, Stage 3 | Planned: gamepads, first stage |
| An on-screen keyboard for the couch | Indispensable, Stage 3 | New |
| Media keys and the system's now-playing (MPRIS on Linux, SMTC on Windows, MediaSession on Android, Now Playing on Apple) | Indispensable, Stage 3 | Planned: integrations, before 1.0 |
| The system kept awake while a video plays | Indispensable, Stage 3 | New |

## Platforms

| Need | Mark | uwui today |
|---|---|---|
| Android TV: a uwui surface inside a Kotlin activity, the remote's D-pad, Select and Back in the focus model, landscape without overscan | Indispensable, Stage 6 | Out of scope (ADR-0181); under review in uwui |
| macOS as a real target, on Apple silicon | Indispensable, Stage 7 | Planned: 1.0, checked in a VM |
| iPhone: a host in a UIKit shell, text input with composition, VoiceOver | Indispensable, Stage 7 | Within reach, without a port (ADR-0181) |
| Apple TV: a host in a UIKit shell, the tvOS focus engine and the Siri Remote, a native text field | Indispensable, Stage 7 | Out of scope (ADR-0181); under review in uwui |

## General

| Need | Mark | uwui today |
|---|---|---|
| Large virtual grids and rows of posters, scrolled sideways and down, images decoded off the interface's thread in a bounded cache | Indispensable, Stage 3 | Planned: virtual lists, first stage |
| Text ready for translation (Fluent), right-to-left | Indispensable, Stage 3 | Planned: before 1.0 |
| Subtitles drawn by the interface where the engine does not (ASS on Android and Apple) | Wished, Stage 4 | New; may stay in Aerith |
| A way to wake the interface's loop from another thread, so that an event the player sends from its own thread ([ADR-0014](adr/0014-the-player-interface-and-mpvs-connection.md)) is shown at once, without polling | Indispensable, Stage 3 | Not read against uwui yet (added at M1.3) |

## The trial Aerith asks for

Before uwui's API is declared stable, a bounded trial: video frames in an element at the display's
rate, an HDR plane under an SDR interface, and a couch screen driven by a remote. The player screen
is proposed as uwui's screen of reference for glass over moving content.

## What Stage 3 needs first

Every need marked *Indispensable, Stage 3* above. Stage 3 starts when uwui holds them
([ROADMAP](ROADMAP.md#stage-3--the-interface-on-the-desktop--01)).
