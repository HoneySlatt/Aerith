# 0017. A test interface on winit and wgpu, thrown away at Stage 3

- Status: Accepted
- Date: 2026-11-11

## Context

Stage 1 must show mpv's picture inside Aerith's window "under a test interface", measured against
the rivals ([ADR-0003](0003-mpv-in-its-own-process.md), [ROADMAP](../ROADMAP.md)).
[ADR-0005](0005-the-interface-on-uwui-a-later-client.md) puts Aerith's interface on uwui and rules
out a temporary one. The test interface is not that interface: it ships to no one. Its purpose:

- give the relay of [ADR-0015](0015-a-relay-for-mpvs-wayland-surface.md) a real window and
  connection to join;
- draw controls and a translucent panel over the video on the GPU, at the display's rate, so that
  their cost against mpv's is measured;
- place the video plane in each of the two orders M1.4b measures.

uwui's host is winit 0.30.13 with wgpu 30.0.1, as SlimeEngine uses them.

## Decision

The owner's decision of 2026-11-11.

**The test interface is a crate apart, `aerith-testbed`, on winit 0.30.13 and wgpu 30**, the
versions SlimeEngine uses. It is never published and never shipped. `aerith-player` depends on
neither. The bench runs it to measure Aerith inside the window.

**It is removed when Stage 3's interface on uwui plays a film**, recorded as debt with that end.

## Alternatives considered

- **smithay-client-toolkit with wgpu**: a thinner window layer that owns all its surfaces. But it is
  not uwui's host, so winit's constraints (its frame pacing, which surfaces' input it accepts) would
  not be met here first.
- **A panel drawn in shared memory, without wgpu**: very few dependencies, but the GPU's sharing
  between a 240 Hz interface and mpv would go unmeasured.

## Consequences

- winit and wgpu, under MIT or Apache-2.0, and their dependencies enter the workspace's lock file.
  `cargo deny` checks their licences. They lengthen the build and CI's time, measured before and
  after.
- Each wgpu major the testbed skips widens its distance from uwui; it follows SlimeEngine's
  version when it must change.
- What the testbed learns about winit becomes needs in [`uwui-needs.md`](../uwui-needs.md).
