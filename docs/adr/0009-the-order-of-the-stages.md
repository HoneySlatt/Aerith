# 0009. The order of the stages

- Status: Accepted
- Date: 2026-11-11

## Context

The player and the core need no interface; the interface waits for uwui
([ADR-0005](0005-the-interface-on-uwui-a-later-client.md)). Store releases should follow the lawful
main uses ([ADR-0007](0007-a-neutral-client-in-two-variants.md)). The owner asked for Jellyfin after
the Android phone and before Android TV.

## Decision

**The stages, in order**, whose goals and exits the [ROADMAP](../ROADMAP.md#4-stages) holds:

1. the player on the desktop, Linux and Windows;
2. the core, without an interface; its milestones may alternate with those of Stage 1;
3. the interface on uwui on the desktop, at a desk and on a couch: the first usable version;
4. the Android phone;
5. the local library and Jellyfin;
6. Android TV;
7. Apple: macOS, iPhone, Apple TV;
8. Trakt and Aerith's own sync;
9. the full variant: torrents, JavaScript plugins, offline downloads;
10. watching together.

**Store releases come after Stage 5.** The first release, after Stage 3, is a desktop download.

## Alternatives considered

- **The library and Jellyfin in the first usable version**: lawful content sooner, but a longer wait
  for anything usable.
- **Apple before Android**: the owner's Apple TV sooner, but the hardware, the developer programme
  and uwui's Apple hosts are further away.

## Consequences

- The core's storage is designed for sync from the start (Nuvio sync is in the first usable version;
  Aerith's own sync comes at Stage 8).
- The order after each stage is checked again at its audit; any change is an ADR.
