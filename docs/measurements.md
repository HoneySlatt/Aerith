# Measurements

The figures each milestone measured, with the machine, the versions and the date. The budgets they
answer to are in the [quality bar](ROADMAP.md#budgets).

## M1.1 — The workspace and its checks

Measured on 2026-11-11 on the high tier ([ROADMAP](ROADMAP.md#hardware)), Rust 1.99.1, cargo-deny
0.20.2, lychee 0.24.2. The workspace holds one crate, documentation only, and no dependency.

| What | Figure | How |
|---|---|---|
| `scripts/check.sh`, cold | 8.7 s | `time nix develop -c scripts/check.sh` after `rm -rf target`; the shell already built, the advisory database already fetched once |
| `scripts/check.sh`, warm | 7.9 to 8.8 s | the same, three runs in a row |
| Its last step, `scripts/test-check.sh` | about 7 s of the above | 19 scratch copies, one per case |
| The other steps, warm | 1.1 to 1.5 s | the same, with `AERITH_CHECK_INNER=1`, which skips the last step |
| `cargo deny --locked check` alone | 0.8 to 1.0 s | mostly the advisory database's fetch |
| CI, Linux job | 42 s | GitHub's own time for the `check` job of run 42938126550, from a fresh runner: Nix installed, toolchain downloaded |
| CI, Windows job | 35 s | GitHub's own time for the `windows` job of the same run, toolchain installed by rustup |

Budget: each CI job under 20 minutes (a floor). Each job's timeout is set to it. The local check has
no budget of its own yet.
