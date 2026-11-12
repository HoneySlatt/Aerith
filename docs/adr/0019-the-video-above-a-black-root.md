# 0019. The video above a black root, the interface in a surface above it

- Status: Accepted
- Date: 2026-11-12

## Context

On Linux, mpv's picture shows in a plane of the interface's window: a subsurface the relay makes
([ADR-0015](0015-a-relay-for-mpvs-wayland-surface.md)). ADR-0015 left open where that plane lies
among the window's surfaces, to be measured at M1.4b. The test window, `aerith-testbed`
([ADR-0017](0017-a-test-interface-on-winit-and-wgpu.md)), builds both orders:

- **below**: the plane under the window's own surface, which the interface draws transparent but
  for its controls;
- **above**: the window's own surface holds one black pixel, scaled to the window; the plane lies
  on it, under a surface of the interface's own, which holds nothing while the controls are
  hidden.

Measured on 2026-11-12 on the owner's desktop at 240 Hz, on Umbriel (wlroots 0.20) and on KWin 6.8
([figures](../measurements.md#m14b--inside-the-window-measured)):

- **Scanout.** Both compositors showed the video straight from mpv's buffer in the order above,
  once the relay passed mpv's opaque region: most frames of most runs, but on Umbriel only 301 of
  1,385 at 24 fps, 1,984 of 3,451 at 59.94 fps and 402 of 1,253 for the 4K clip under VRR, for a
  reason not known. In the order below, neither ever did: the window's transparent surface lies
  over the video, and they scan out a single buffer.
- **Power.** Under mpv's audio sync, the two orders were within a watt or two of each other and of
  mpv alone, in no steady direction. Under display sync, where mpv presents at every refresh, on
  Umbriel, the order above cost 1.6 to 3.9 W less than mpv alone, and 1.6 to 6.6 W less than the
  order below.
- **Cadence.** On Umbriel the orders did about as well as each other. On KWin, the order above
  had fewer gaps off cadence (13 % against 18 % at 23.976 fps, 12 % against 23 % at 59.94 fps).
- **The controls.** Shown and hidden 60 times, they added about 28 frames shown too short in the
  order above, and about as many shown too long (29 of 12 to 15 refreshes, against 1 to 3 in runs
  without the controls), as the compositor goes from scanout to composition and back; none in the
  order below. The
  interface's own frames were never late in either.
- **What the interface must do.** In the order above, it draws into a surface of its own above the
  plane, paced by that surface's frame callbacks, never into the window's root, which holds the
  black pixel. winit 0.30 does not offer that; the testbed builds it beside winit. These are needs
  of uwui's ([`uwui-needs.md`](../uwui-needs.md)).

## Decision

The owner's decision of 2026-11-12, to be confirmed in a uwui session.

**Aerith's player screen puts the video above a black root.** The window's own surface holds an
opaque black buffer, scaled to the window; the video plane lies on it; the interface draws its
controls in a surface of its own above the plane, which holds nothing while the controls are
hidden. The compositor then has the video alone to show, and can scan it out.

**The plane fills the window**, mpv drawing its own black bars, so that the plane covers the root
whole and the compositor culls it.

## Alternatives considered

- **The video below a transparent root**: the interface draws into the window as it does
  anywhere, and its controls cause no hitch. But no compositor measured scans the video out under
  a transparent surface, so each refresh is composited: more power under display sync, and fewer
  frames on cadence on KWin.
- **Switching order with the controls**: below while they show, above while hidden. The best of
  each in theory, but every switch re-stacks surfaces mid-film, a hitch of its own, and doubles the
  paths to test.

## Consequences

- uwui must let an application draw its content into a surface of its own above a plane, paced by
  that surface's frame callbacks, its input passing through to the window, and must leave the
  window's root surface, its buffer and its scaling to the application. Both needs are now
  indispensable for Stage 3 ([`uwui-needs.md`](../uwui-needs.md)).
- Showing the controls costs about one short frame every two toggles, while the compositor goes
  from scanout to composition and back. Interfaces that show their controls rarely during a film
  pay it rarely.
- The order needs a compositor with viewports and fractional scaling; `aerith-testbed` checks for
  them.
- A uwui session may reopen this decision, with what uwui can build.
- The order below stays in the testbed for measures, until the testbed goes.
