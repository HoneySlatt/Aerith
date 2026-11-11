# Contributing

How to set up the environment, build Aerith and run its checks. The rules of the code are in
[`AGENTS.md`](AGENTS.md); its parts in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## The environment

Aerith builds on Linux, x86_64, inside a Nix development shell (`flake.nix`) that holds every tool
the checks use, pinned by `flake.lock`. CI uses the same shell.

```sh
nix develop
```

It holds:

- the stable Rust toolchain pinned in `rust-toolchain.toml`, which builds Aerith
  ([ADR-0011](docs/adr/0011-a-pinned-stable-toolchain.md));
- `cargo deny`, which enforces the dependency policy of `deny.toml`;
- `lychee`, which checks the links between Markdown files.

Without Nix, rustup reads `rust-toolchain.toml` and installs the same toolchain; `cargo deny` and
`lychee` are then yours to install. CI's Windows job works this way.

## Building and testing

There is nothing to run yet: `aerith-player` holds only its documentation until M1.3
([`docs/PLAN.md`](docs/PLAN.md)). The commands that build and test the workspace:

```sh
cargo build --workspace
cargo test --workspace
```

## The checks

```sh
scripts/check.sh
```

runs every automated check of the definition of done, in this order, and stops at the first that
fails. It refuses to start outside the development shell. CI runs it unchanged on each push to
`main`, each pull request and once a week.

| Step | What it checks | Alone |
|---|---|---|
| links | Links between Markdown files, anchors included, offline | `lychee --offline --include-fragments $(git ls-files --cached --others --exclude-standard '*.md')` |
| AGENTS.md length | `AGENTS.md` under 1,500 words | `wc -w AGENTS.md` |
| scripts | The scripts' syntax | `for script in scripts/*.sh; do bash -n "$script"; done` |
| MPL-2.0 headers | Each `.rs`, `.sh`, `.nix`, `.toml` and `.yml` file names the MPL-2.0 in its first three lines | the step in `scripts/check.sh` |
| formatting | rustfmt's format | `cargo fmt --all --check` |
| lints | Clippy and the compiler's lints of `Cargo.toml`, warnings as errors | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| tests | Every test | `cargo test --workspace --locked` |
| documentation | rustdoc, warnings as errors | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` |
| dependencies | Licences, RustSec advisories and sources, by `deny.toml`; fetches the advisories | `cargo deny --locked check` |

External links are not checked: they fail for reasons unrelated to a change.

CI also runs the lints and the tests on Windows, with rustup, since Windows is a platform of Stage
1.

How long a full check takes is in
[`docs/measurements.md`](docs/measurements.md#m11--the-workspace-and-its-checks).

## A new file

Each file of code or configuration starts with the MPL-2.0 header, after a shebang if it has one:

```rust
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
```

with `#` in place of `//` in shell, Nix, TOML and YAML files. Nix sees a new file only once it is
added to git: `git add` it before `nix develop`.

## A new dependency

Ask the owner first, with its reason, the alternative weighed and its licence
([`AGENTS.md`](AGENTS.md#code-rules)). `deny.toml` lists the licences that may enter Aerith's
binaries ([ADR-0002](docs/adr/0002-mpl-2-0.md)): a crate under another licence fails the check.

## Bumping the toolchain

Change `channel` in `rust-toolchain.toml` and `rust-version` in `Cargo.toml` together, in a commit of
their own, then run the checks ([ADR-0011](docs/adr/0011-a-pinned-stable-toolchain.md)).
