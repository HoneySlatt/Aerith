<p align="center">
  <img src="logo/aerith.svg" alt="Aerith's logo: a five-petal flower made of a single folded ribbon, from violet to cyan" width="256">
</p>

# Aerith

Aerith is an open-source video client for films and series. It will read Stremio addons, your own
files on a disk or a NAS, and Jellyfin servers, and sync with Nuvio, Trakt and its own sync. It
aims at the image and sound of mpv and Kodi, with Dolby Vision, HDR and audio passthrough, on Linux,
Windows, macOS, iPhone, Apple TV, Android phones and Android TV, at a desk and on a couch.

## State

Designed, being built. The Rust workspace and its checks exist, and so does the bench that
measures the rivals Aerith is held to (mpv, Kodi, Stremio, Jellyfin Desktop). Aerith's player
opens, plays, pauses, seeks and stops a file or a stream through mpv on Linux, and shows mpv's
picture inside a window, under a test window that ships to no one; there is no interface yet. The
first stage, the player on the desktop, is under way.

Aerith installs no content source of its own. What you play through it is your responsibility.

## Where to read next

- [`docs/ROADMAP.md`](docs/ROADMAP.md): what Aerith is, its quality bar, its decisions and its
  stages.
- [`docs/PLAN.md`](docs/PLAN.md): the work in progress.
- [`docs/adr/README.md`](docs/adr/README.md): the decisions and why.
- [`docs/survey.md`](docs/survey.md): the facts of the field behind them, with sources.
- [`docs/uwui-needs.md`](docs/uwui-needs.md): what Aerith asks of uwui, its interface's framework.

## Checks

```sh
nix develop -c scripts/check.sh
```

It runs every automated check: formatting, lints, tests, documentation, the dependencies' licences
and advisories, the files' licence headers and the links, then proves that each check fails when
it should.
[`CONTRIBUTING.md`](CONTRIBUTING.md) says what each one does and how to build Aerith.

## Licence

Aerith is under the Mozilla Public License 2.0: [`LICENSE`](LICENSE). Its logo is not under it:
[`logo/README.md`](logo/README.md) says how it may be shown.
