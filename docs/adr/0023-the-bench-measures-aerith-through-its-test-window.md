# 0023. The bench measures Aerith through its test window, as it measures a rival

- Status: Accepted; amends 0013 (how the bench measures Aerith)
- Date: 2026-11-14

## Context

[ADR-0013](0013-the-bench.md) made the bench a Rust crate that drives each rival's engine, and
said it "will measure Aerith through `aerith-player` later". When M1.4b came to measure Aerith
inside its window, what had to be measured was more than the player:

- the plane in a real window, on the compositor, where the cadence, the scanout and the power
  are decided ([ADR-0019](0019-the-video-above-a-black-root.md));
- the same figures as for mpv alone, read the same way, so that the two compare: mpv's counters
  over its IPC, and the compositor's presentation feedback;
- the interface's own wake-ups and frames, which only a window has.

`aerith-testbed` ([ADR-0017](0017-a-test-interface-on-winit-and-wgpu.md)) already plays a file
through `aerith-player` in a window, and prints its own statistics. M1.4b built the bench's Aerith
on it, and M1.4c reads through it whether mpv runs under display sync and what the relay's probe
told ([ADR-0022](0022-the-relay-tells-the-display-s-rate-when-the-player-asks.md)).

## Decision

The owner's decision of 2026-11-14.

**The bench runs Aerith as a rival: it starts `aerith-testbed`, fullscreen, in its sandbox, and
links no crate of Aerith's.** The testbed opens the clip itself, as Aerith would. The bench gives
it a wrapper as its mpv: the same mpv as mpv alone, with the bench's IPC socket and
`WAYLAND_DEBUG=client` added. Through these, the bench reads Aerith's mpv exactly as it reads mpv
alone: its counters, `display-sync-active`, and the compositor's presentation feedback in its
Wayland log. From the testbed's `--stats` it adds what only Aerith has: the relay's count of every
frame presented, its probes, and the wake-ups of the window's thread and of the relay's.

## Alternatives considered

- **The bench linking `aerith-player`**, as ADR-0013 foresaw: the player's events at hand, but
  no window, so no plane, no scanout and no compositor; or a second test window inside the bench.
- **The testbed measuring itself**: no wrapper, but mpv alone would be measured by other code,
  and the two figures would no longer compare.

## Consequences

- The bench measures what a person would see: the testbed's window, the relay, the compositor.
- The bench and the testbed share a format, the testbed's `--stats` lines, documented in the
  testbed's crate; a change there is a change of the bench.
- When Stage 3's interface replaces the testbed ([ADR-0017](0017-a-test-interface-on-winit-and-wgpu.md)),
  the bench runs that interface the same way, or this decision is reopened.
- Every figure read from the Wayland log includes `WAYLAND_DEBUG=client`'s cost, for mpv alone
  and Aerith alike, not measured.
