# 0007. A neutral client, in two variants

- Status: Accepted
- Date: 2026-11-11

## Context

Store rules ([survey](../survey.md#stores-and-third-party-rules)): Apple forbids downloaded code that
changes an app's features (guideline 2.5.2) and facilitating illegal file sharing (5.2.3); Google
Play forbids apps that facilitate unauthorised streaming; Trakt refuses apps that promote piracy.
Most of Nuvio's public JavaScript providers scrape unlicensed sites. Stremio ships a limited version
on the iOS App Store and its full version through AltStore in the EU. Stremio addons are declarative
HTTP endpoints, not downloaded code.

## Decision

**Aerith ships no source of content by default.** It installs no addon, plugin or stream source on
its own; what it shows first is lawful: metadata catalogues, the owner's files, Jellyfin.

**Two variants are built from one code base:**

- **store builds** (App Store, Google Play) without the torrent engine and without JavaScript
  plugins;
- **full builds**, downloaded directly (desktop packages, an APK, AltStore in the EU), with both.

**JavaScript plugins run in a sandbox** in every full build: limits on memory and time, network
access only through Aerith's I/O layer and its rules, no file system, no access to Aerith's tokens.

**Addons receive the person's activity only when allowed**: Stremio's `player` and `library` events
are off for each addon until the person turns them on.

## Alternatives considered

- **The same features everywhere**: one build, but rejection by Apple and Google, and so no Apple
  TV.
- **No torrents and no JavaScript plugins**: the simplest and clearest position, but two features the
  owner wants are lost.

## Consequences

- Two variants to build and test; the difference is a build feature, never a runtime switch.
- Apple TV never has torrents or JavaScript plugins.
- Store releases come after the local library and Jellyfin, the lawful main use reviewers look for
  ([ROADMAP](../ROADMAP.md#4-stages)).
