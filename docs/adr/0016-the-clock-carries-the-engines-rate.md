# 0016. The clock carries the engine's rate, and is read again every 10 s

- Status: Accepted; amends 0014 (the clock)
- Date: 2026-11-11

## Context

[ADR-0014](0014-the-player-interface-and-mpvs-connection.md) makes the position a clock: a reading,
the instant it was taken, and whether it runs. The interface computes the position at any moment
from it, at real time. ADR-0014 left open whether to read it again while playing, once mpv may
change its speed to match the display.

Measured on 2026-11-11 on the owner's desktop:
- mpv 0.42 with gpu-next, `video-sync=display-resample`, on a 240.016 Hz output;
- 150 s per clip, the position read each second and fitted
  ([figures](../measurements.md#m14a--the-relay-and-the-test-interface)).

The findings:

- mpv's `video-speed-correction` settles within about 10 s and then stays the same. It was 1.001067
  for a 23.976 fps clip and 0.99317 for a 25 fps clip.
- A clock at real time drifts from mpv's position by 0.12 % to 0.67 %, up to 24 s an hour.
- A clock that runs at `speed × video-speed-correction` still drifts by +0.018 % on both clips,
  about 11 ms a minute, 0.65 s an hour. The offset is the same for both clips, so it is a property
  of the display's clock against the system's, not noise.
- mpv's own position moves by whole frames: a reading may lag by up to one frame.

Whether Aerith uses display sync at all is measured at M1.4b and decided then
([PLAN](../PLAN.md)). The clock must hold either way.

## Decision

The owner's decision of 2026-11-11.

**A clock carries its rate**: how many seconds of the source pass in one second of real time. It is
mpv's `speed` times its `video-speed-correction`, read with the position. Under mpv's default audio
sync the rate is the speed, 1.

**While a source plays, the player reads the clock again every 10 s.** With the rate, the error
between readings stays under 2 ms, plus up to one frame. Paused, stopped or waiting for data, the
player reads nothing: the at-rest floor is untouched.

## Alternatives considered

- **The rate alone, no reading again**: no wake-up at all while playing, but 0.65 s an hour of
  drift, until the next pause, seek or source.
- **Reading again every 10 s, without the rate**: up to 67 ms of error between readings when mpv
  slows a 25 fps film to the display.
- **Observing mpv's correction**: no timer, but mpv may change it at each frame. The prototype saw
  it change twice at the start, then never; that is no promise.

## Consequences

- `Clock` gains a field, and `position_at` uses it.
- The engine thread wakes once every 10 s while a source plays; it blocks without a timeout
  otherwise.
- The known debt "the clock is not read again while playing" is paid.
