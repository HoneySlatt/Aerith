# Measurements

The figures each milestone measured, with the machine, the versions and the date. The budgets they
answer to are in the [quality bar](ROADMAP.md#budgets).

## M1.1 — The workspace and its checks

Measured on 2026-11-11 on the high tier ([ROADMAP](ROADMAP.md#hardware)), Rust 1.99.1, cargo-deny
0.20.2, lychee 0.24.2. The workspace holds one crate, documentation only, and no dependency.

| What | Figure | How |
|---|---|---|
| `scripts/check.sh`, cold | 3.0 s | `time nix develop -c scripts/check.sh` after `rm -rf target`; the shell already built, the advisory database already fetched once |
| `scripts/check.sh`, warm | 1.4 to 1.6 s | the same, three runs in a row |
| CI, Linux job | see below | GitHub's own time for the `check` job, from a fresh runner: Nix installed, toolchain downloaded |
| CI, Windows job | see below | GitHub's own time for the `windows` job, toolchain installed by rustup |

Budget: each CI job under 20 minutes (a floor). Each job's timeout is set to it.
