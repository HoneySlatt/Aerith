# Aerith

Aerith is an open-source video client for films and series. It will read Stremio addons, your own
files on a disk or a NAS, and Jellyfin servers, and sync with Nuvio, Trakt and its own sync. It
aims at the image and sound of mpv and Kodi, with Dolby Vision, HDR and audio passthrough, on Linux,
Windows, macOS, iPhone, Apple TV, Android phones and Android TV, at a desk and on a couch.

## State

Designed, not built. No code exists yet; nothing can be run. The first stage, the player on the
desktop, comes next.

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

It checks the links between the Markdown files, anchors included, with lychee.

## Licence

Aerith is under the Mozilla Public License 2.0: [`LICENSE`](LICENSE).
