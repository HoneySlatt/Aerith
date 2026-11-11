# 0008. The quality bar

- Status: Accepted
- Date: 2026-11-11

## Context

The owner's projects aim at the top of their field and measure it. Aerith has no code yet, so no
figure of its own exists; its rivals can be measured now.

## Decision

**The bar has three parts**, held in the [ROADMAP's quality bar](../ROADMAP.md#quality-bar):

- **references**, the rivals to beat, each for what it does best;
- **hardware tiers**, each a named machine, from the lowest supported to the high tier;
- **budgets**, each with how it is measured and where it is checked.

**Floors and leads, as in the owner's other projects.** A floor is an absolute figure and gates the
milestone that measures it. A lead is a margin over the best rival, measured; a lead missed is debt,
never a gate. On every figure measured, Aerith does at least as well as the best rival.

**The figures are set by measuring first.** Stage 1's second milestone (M1.2) measures the rivals (mpv
alone, Kodi, Stremio, Jellyfin Desktop) on the owner's desktop and the lowest laptop; the budgets'
figures are then written into the ROADMAP and shown to the owner. Until then, the ROADMAP gives
their starting points.

## Alternatives considered

- **Figures chosen now, without measurement**: quicker, but promises nobody measured.

## Consequences

- Each budget has a benchmark kept in the repository; the figures live in `docs/measurements.md`
  once the first is taken.
- A tier without a device stays "unmeasured" until the owner has one.
