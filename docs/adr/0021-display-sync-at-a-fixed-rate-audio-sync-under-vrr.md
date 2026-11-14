# 0021. Display sync at a fixed refresh rate, audio sync under VRR

- Status: Accepted; amended by 0025 (`display-vdrop` under passthrough)
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
  whole fraction of the display's: 12 % to 31 % of gaps off at 23.976 and 59.94 fps. On Umbriel,
  without `wp_fifo_v1`, mpv alone as Aerith; on KWin, with fifo, Aerith (mpv alone's cadence
  cannot be read there: under fifo, mpv asks for feedback on one frame in two). A frame due just
  at a refresh's edge falls on either side of it. With fifo, a 25 fps film keeps its cadence.
- **At a fixed rate, display sync holds it**, with mpv's own counters of delayed and mistimed
  frames as the measure: 2 to 5 delayed a minute where the compositor scanned the plane out (the
  order above), 6 to 168 elsewhere (mpv alone, the order below). mpv presents at every refresh:
  4.5 to 11.7 W more on the GPU than under audio sync, and the relay's thread wakes 926 to 1,422
  times a second, 4 to 6 times a refresh. mpv's sync estimate reached 7.8 to 13.4 ms in the order
  above, up to 38 ms for mpv alone; under audio sync, never 0.1 ms.
- **Under VRR, audio sync holds it on a quiet machine, but not always**: on a 24 fps film, 2 spans
  of about 1,390 more than half a refresh off, for mpv alone and Aerith alike; on a 25 fps film,
  41 of 1,444 for mpv alone. Load from other programs disturbs it most.
- **Under VRR, display sync defeats VRR**: mpv's presents at every refresh hold the display at its
  top rate; mpv counted 181 to 681 frames delayed a minute, against 0, its sync estimate passed
  the ±20 ms floor in 6 of 9 runs, and the GPU drew 6.6 to 12.3 W more.
- **No Wayland protocol tells a client whether VRR is on.** The compositor's presentation
  feedback shows it under audio sync: at a fixed rate, two frames are always a whole number of
  refreshes apart (100 % of intervals); under VRR, most are not (1 % to 17 % whole). Under display
  sync it does not: a VRR display held at its top rate looks fixed (94 % to 100 % whole). The
  relay receives that feedback for every frame of mpv's.
- mpv can change `video-sync` while it plays.
- 60 Hz is not measured yet: it is measured at M1.5, on the lowest laptop.

## Decision

The owner's decision of 2026-11-12; its detection corrected by the owner the same day, after the
milestone's closing review found that display sync hides VRR.

**The player uses display sync while the display runs at a fixed rate, and audio sync while it
runs under VRR.** The relay tells which from the presentation feedback of mpv's frames, under
audio sync only; the player changes mpv's `video-sync` when the answer changes, and the clock
follows mpv's rate as it does already.

**The player plays under audio sync to tell**, mpv's default, which costs least and shows no wrong
frame for long: at the start of a film, and again a few seconds each time the window or its
display changes (fullscreen, a new size, another output), as a compositor's automatic VRR turns on
or off with them. Under display sync, the display's rate cannot be told.

## Alternatives considered

- **Display sync always**: simple, and holds the cadence at a fixed rate, but defeats VRR, delays
  frames there, and costs the most power.
- **Audio sync always**, mpv's default: costs least and holds the cadence under VRR, but misses
  the floor at a fixed rate on the commonest films.
- **A setting left to the person**: no detection to build, but a choice most people cannot make
  well, and a wrong default for half the displays.

## Consequences

- To build, at M1.4c: the relay's detection of VRR from presentation times under audio sync, its
  thresholds and how long it watches; the player probing again under audio sync when the window or
  its display changes, and switching mpv's `video-sync`; tests against a fake compositor that
  presents at whole refreshes and between them, and that turns VRR on when a window goes
  fullscreen. The bench's `whole_intervals` figure is the measure the thresholds start from.
- A VRR turned on without any change of the window or its display goes unseen until the next
  change: the film keeps display sync, at its cost under VRR.
- Each probe, and each switch, is a few seconds of mpv's timing settling, to be measured.
- The 60 Hz figures of M1.5 confirm the fixed-rate half of the rule, or reopen it.
- At a fixed rate, the relay's thread wakes 4 to 6 times a refresh during a film, and the window's
  thread more than once a refresh, as they share the window's connection
  ([ADR-0018](0018-what-building-the-relay-changed.md)).
