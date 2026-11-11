# 0001. What Aerith is

- Status: Accepted
- Date: 2026-11-11

## Context

The owner built a prototype media client, Magnolia-media, from 2026-10-28 to 2026-11-04: Rust and
egui, Stremio addons, a Nuvio account, and playback in a separate mpv window with its own controls
in Lua. It had no roadmap, no decisions written down, no measurements and no licence, and stopped
with its last screen unfinished. The name Magnolia now belongs to the owner's DAW.

The owner restarts the project as Aerith, for four reasons: the look egui allowed, the player in a
window of its own, the code grown without the project method, and a wider scope. The prototype's
lessons are kept in [`../survey.md`](../survey.md#the-prototype-magnolia-media); its code is not
ported.

## Decision

**Aerith is a video client for films and series**, open source and public, at the level of the best
of its field combined: the reach of Stremio and Nuvio, the image and sound of mpv and Kodi, the
polish of Infuse and the Apple TV app. It does not play music.

**It reads, over time:** Stremio addons (catalogues, metadata, streams, subtitles); the owner's own
files on a disk or a NAS; Jellyfin servers. Not Plex.

**It syncs** with a Nuvio account, with Trakt, and with a sync of Aerith's own between a person's
devices.

**Its image and sound are not negotiable:** HDR10, HDR10+ and Dolby Vision shown correctly;
TrueHD/Atmos, DTS-HD/DTS:X and E-AC3 passed through intact; rendering at mpv's level (libplacebo);
complete subtitles (ASS, PGS, online search, a sync offset). Aerith states what each platform can
deliver, and never claims what it did not observe.

**It runs on** Linux, Windows, macOS, iPhone, Apple TV, Android phones and Android TV
([ADR-0006](0006-seven-platforms.md)), used at a desk with a mouse and on a couch with a remote or
a gamepad, both equally well.

**Later, it adds** built-in torrents, offline downloads, Nuvio's JavaScript plugins and watching
together ([ADR-0007](0007-a-neutral-client-in-two-variants.md)).

**Its interface** has an identity of its own, built on uwui's language (glass, shapes that morph,
springs). It is in English first, every text ready for translation.

## Alternatives considered

- **Continuing Magnolia-media**: its protocols and tests worked, but its interface, its player
  window and its missing method were the reasons to restart.
- **A Plex client too**: Plex charges third-party clients for remote streaming and offers no Rust
  client; the owner chose Jellyfin only.
- **Music as well**: Opium, the owner's music player, covers it.

## Consequences

- The scope is that of several mature products; the stages of the
  [ROADMAP](../ROADMAP.md#4-stages) deliver it in order, each stage measured.
- Nothing of Magnolia-media's data is imported: it was a prototype with no users but its author.
