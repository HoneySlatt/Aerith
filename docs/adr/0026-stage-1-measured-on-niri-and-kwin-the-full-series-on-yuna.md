# 0026. Stage 1 measured on niri and KWin, KWin the HDR reference, the full series on Yuna

- Status: Accepted
- Date: 2026-11-17

## Context

The quality bar's high tier is the owner's desktop ([ADR-0008](0008-the-quality-bar.md),
[ROADMAP](../ROADMAP.md#hardware)). Every figure of Stage 1, from M1.2 to M1.5a, was taken under
Umbriel (wlroots 0.20), with some series under KWin 6.8. On 2026-11-16 the owner's desktop moved
from Umbriel to niri. What bears on it, read on 2026-11-17:

- **niri offers no `wp_color_manager_v1`, and runs no output in HDR** (niri 26.04's protocol list;
  its release notes). An HDR film cannot reach the owner's monitor under niri.
- **niri is built on Smithay**, not wlroots. The relay's checks follow wlroots 0.20
  ([ADR-0018](0018-what-building-the-relay-changed.md)); colour management also follows Umbriel,
  KWin and Yuna ([ADR-0027](0027-the-relay-passes-mpvs-colour-description-checked-under-kwin.md)).
  Smithay may refuse what wlroots accepts. One known case is untested: a compositor that never
  sends a subsurface's preferred fractional scale leaves mpv never told its outputs
  ([PLAN](../PLAN.md#known-debt)).
- **niri's configuration turns VRR on for both of the owner's outputs**, and names the 4K 240 Hz
  monitor `DP-2`.
- **KWin 6.8 runs HDR outputs**, offers colour management with a reference luminance of its own,
  and is the field's reference for HDR on Linux. Its Plasma session is on the owner's machine.
- **The owner moves to Yuna, the owner's own compositor, as soon as it can be used daily** (Yuna's
  Stage 2). Yuna draws HDR from its Stage 5.

## Decision

The owner's decisions of 2026-11-17.

**Stage 1's floors hold on niri and on KWin.**

- **KWin is the HDR reference** for M1.5a: the HDR and Dolby Vision clips are compared with mpv
  alone, measured and looked at under KWin with HDR on.
- **niri gets a compatibility pass, not a full series**: the relay never closes the window, mpv is
  told its outputs, and the relay's probe tells a fixed rate from VRR right. Figures taken under
  niri are marked provisional.
- **The full series of the high tier is taken on Yuna** once the owner uses it daily.

Figures taken under Umbriel stay in [`measurements.md`](../measurements.md) as what they are: the
same hardware under another compositor.

## Alternatives considered

- **The full series again under niri**: the owner's compositor today, but one the owner is about to
  leave, without HDR; hours of the owner's screen for figures soon replaced.
- **Keeping Umbriel for the measures**: the figures would compare with M1.2 to M1.4c's, but on a
  compositor the owner no longer runs, and one that Aerith's people will rarely use.
- **Waiting for Yuna before any HDR check**: one compositor fewer, but M1.5a and Stage 1 would wait
  on another project's Stage 5.

## Consequences

- M1.5a's HDR sessions and its measure of `set_luminances` move from Umbriel to KWin.
- A milestone of robustness, before M1.5a's sessions, holds niri's compatibility pass.
- Stage 1's audit compares figures across three compositors; each figure names its compositor.
- Yuna's checks join the relay's union of refusals when Aerith is measured on it.

## Validation and reconsideration

Measured: niri's protocol list (no colour manager, no fifo), read in niri 26.04's source. Assumed:
that Yuna becomes the owner's daily compositor within Stage 1 or soon after. If it does not by
Stage 1's audit, the owner is asked whether the full series is taken under niri or KWin instead.
