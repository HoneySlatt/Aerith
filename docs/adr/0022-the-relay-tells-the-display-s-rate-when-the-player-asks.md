# 0022. The relay tells the display's rate when the player asks: 24 intervals, 50 µs

- Status: Accepted
- Date: 2026-11-14

## Context

[ADR-0021](0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md) decided that the player
plays under display sync while the display runs at a fixed rate, and under audio sync while it
runs under VRR. The relay is to tell which from the compositor's presentation feedback for mpv's
frames, under audio sync only. ADR-0021 left to M1.4c the thresholds, how long the relay watches,
and how the relay and the player share the work.

M1.4b's logs hold every presentation mpv received, on the owner's desktop at 240 Hz and under
VRR, on Umbriel and on KWin 6.8, mpv alone and inside Aerith
([figures](../measurements.md#the-detection-calibrated-on-m14bs-logs)):

- **At a fixed rate**, no interval of about 70,000 between two frames presented one after the
  other lay more than 6.1 µs from a whole number of refresh periods.
- **Under VRR, under audio sync**, intervals scatter: 0 % to 12 % lie within 0.05 of a refresh
  (208 µs). The film's rate does not change this, even where its period is a whole number of
  refreshes (24 and 60 fps at 240 Hz). In any window of 24 intervals, at most 4 lie within 50 µs
  (17 %), and at most 6 within 100 µs.
- **Under display sync**, 97 % to 100 % are whole, VRR or not, as ADR-0021 says.
- Every presentation on both compositors carried the `vsync` and `hw_clock` flags and a refresh
  period. wlroots' headless backend, which agents test on, gives neither flag nor period.
- mpv restores an option given with a load when that source ends, even after a change while it
  played (mpv 0.42, checked over its IPC). mpv sends no change of `display-sync-active`.

## Decision

**The player asks; the relay probes, then tells.** The engine thread asks the relay for a probe,
numbered, when mpv is under audio sync: at each source's first frame, and after each change the
relay reports. The relay's probe reads the frames presented after that, and tells the engine
thread its verdict once, with the probe's number; a verdict on an older probe is dropped. The
relay never knows which sync mpv runs under, and the engine thread never sees a single frame.

**A probe lets 8 frames go by, then reads 24 intervals.** The 8 cover the frames mpv timed before
audio sync, still in its swapchain. An interval counts only between two frames that both carry
`vsync` and `hw_clock` and the same refresh period; a frame without them breaks the chain. An
interval is whole within 50 µs of a whole number of refresh periods, one or more. **At least
half whole is a fixed rate; fewer is VRR.** 50 µs is eight times the farthest seen at a fixed rate,
and short of VRR's scatter. Half the intervals, 12, lies 12 from a fixed rate's 24 and 8 from
VRR's worst 4. At 24 fps, a probe takes about 1.4 s; at 60 fps, about 0.55 s.

**What is a change.** A new size of the plane, the plane's surface entering or leaving an output,
and a new refresh period in the presentations. A change during a probe starts the probe again,
untold. A change after a probe is told once, until the next probe.

**Each source starts under audio sync**, an option of its own load: mpv restores it at the source's
end, so that no source inherits the last one's display sync. On a fixed rate, the engine thread
sets `video-sync=display-resample`; on a change under display sync, it sets audio sync back, and
asks for a probe once mpv answered. After each switch it reads the clock again
([ADR-0016](0016-the-clock-carries-the-engines-rate.md)).

**No verdict is audio sync.** A compositor without presentation feedback, without hardware times
or without a refresh period never ends a probe: mpv stays under audio sync, mpv's default.

**What the relay tells is for the bench, not an event.** `Plane::presentation` gives the probes
ended, the last verdict and how long the last probe took. The interface has no use for it yet.

## Alternatives considered

- **The relay deciding alone**, switching mpv through the engine thread on its own verdicts: one
  party less to ask, but the relay would have to know which sync mpv runs under, to know when its
  frames say something.
- **The verdict on every frame, a sliding window**: the display's state known at once at any
  time, but a probe under display sync is meaningless, so the probe must stop anyway.
- **A tolerance of a twentieth of a refresh** (the bench's `whole_intervals`): it scales with the
  refresh, but VRR's scatter and the hardware's precision are times, not fractions. At 60 Hz it
  would be 833 µs, past VRR's scatter.
- **The share of whole intervals at 75 % or 90 %**: a fixed rate scored 100 % in every window;
  half keeps the widest margin on both sides.
- **An event for the interface**, `DisplaySync` or a field of the report: Stage 3 may want one
  in its statistics; nothing reads it now. mpv sends no change of `display-sync-active`, so a
  report of it would have to be read again on a timer.
- **Audio sync set by `set_property` at each source's start**: one command more each source, and
  a race with the source's first frames; the load's option is mpv's own mechanism.

## Consequences

- A compositor that offers presentation times but not from the display's hardware (wlroots'
  headless backend) keeps mpv under audio sync. KWin's virtual backend gives hardware flags, and
  a probe there ends on a fixed rate.
- A switch to display sync and back costs a probe each time the window changes, under VRR too:
  about 1.4 s of audio sync at 24 fps, measured at M1.4c on the desktop.
- The thresholds rest on one desktop at 240 Hz, under two compositors. 60 Hz, at M1.5 on the
  lowest laptop, confirms them or reopens them.
- mpv may refuse display sync where its own conditions fail (the film's rate too far from the
  display's): the bench reads `display-sync-active` to see what mpv did.
