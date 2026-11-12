# 0021. Display sync at a fixed refresh rate, audio sync under VRR

- Status: Accepted
- Date: 2026-11-12

## Context

mpv times a film's frames in one of two ways. Under **audio sync**, its default, it shows each
frame when the audio clock says it is due. Under **display sync** (`video-sync=display-resample`),
it counts the display's refreshes, gives each frame a whole number of them, and resamples the
sound by a fraction of a percent to keep the two together. The quality bar's cadence floor asks
that no frame be dropped or repeated without a cause shown
([ROADMAP](../ROADMAP.md#budgets)). The clock already carries mpv's rate for either
([ADR-0016](0016-the-clock-carries-the-engines-rate.md)).

Measured on 2026-11-12 on the owner's desktop, mpv alone and inside Aerith
([figures](../measurements.md#m14b--inside-the-window-measured)):

- **At a fixed rate of 240 Hz, audio sync misses the cadence** of films whose rate is close to a
  whole fraction of the display's: 12 % to 31 % of gaps off at 23.976 and 59.94 fps, in mpv alone
  as in Aerith, with or without `wp_fifo_v1` (Umbriel lacks it, KWin has it). A frame due just at
  a refresh's edge falls on either side of it. With fifo, a 25 fps film keeps its cadence.
- **At a fixed rate, display sync holds it**: a handful of corrections a minute. But mpv presents
  at every refresh: 5 to 9 W more on the GPU, and the relay's thread wakes about 1,000 times a
  second.
- **Under VRR, audio sync holds it** on a quiet machine: on a 24 fps film, 2 to 8 spans of about
  1,400 more than half a refresh off. The display follows the frames.
- **Under VRR, display sync defeats VRR**: mpv's presents at every refresh hold the display at its
  top rate; mpv counted 181 to 681 frames delayed a minute, against 0, and the GPU drew 7 to 11 W
  more.
- **No Wayland protocol tells a client whether VRR is on.** The compositor's presentation
  feedback shows it: at a fixed rate, two frames are always a whole number of refreshes apart;
  under VRR, most are not (2 % to 12 % whole, against 100 %). The relay receives that feedback for
  every frame of mpv's.
- mpv can change `video-sync` while it plays.
- 60 Hz is not measured yet: it is measured at M1.5, on the lowest laptop.

## Decision

The owner's decision of 2026-11-12.

**The player uses display sync while the display runs at a fixed rate, and audio sync while it
runs under VRR.** The relay tells which from the presentation feedback of mpv's frames; the player
changes mpv's `video-sync` when the answer changes, and the clock follows mpv's rate as it does
already.

**Until the answer is known, at the start of a film, the player plays under audio sync**, mpv's
default, which costs least and shows no wrong frame for long.

## Alternatives considered

- **Display sync always**: simple, and holds the cadence at a fixed rate, but defeats VRR, delays
  frames there, and costs the most power.
- **Audio sync always**, mpv's default: costs least and holds the cadence under VRR, but misses
  the floor at a fixed rate on the commonest films.
- **A setting left to the person**: no detection to build, but a choice most people cannot make
  well, and a wrong default for half the displays.

## Consequences

- To build: the relay's detection of VRR from presentation times, its thresholds and how long it
  watches; the player switching mpv's `video-sync`; tests against a fake compositor that presents
  at whole refreshes and between them. It is planned as its own milestone.
- A display that switches between a fixed rate and VRR during a film (a compositor's "automatic"
  mode entering or leaving fullscreen) switches the sync too; each switch is a short settling of
  mpv's timing, to be measured.
- The 60 Hz figures of M1.5 confirm the fixed-rate half of the rule, or reopen it.
- At a fixed rate, the relay's thread wakes about once a refresh during a film; the window's
  thread too, as they share the window's connection
  ([ADR-0018](0018-what-building-the-relay-changed.md)).
