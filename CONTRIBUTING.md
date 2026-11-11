# Contributing

How to set up the environment, build Aerith and run its checks. The rules of the code are in
[`AGENTS.md`](AGENTS.md); its parts in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## The environment

Aerith builds on Linux, x86_64, inside a Nix development shell (`flake.nix`) that holds every tool
the checks use, pinned by `flake.lock`. CI uses the same shell. It needs Nix with its `nix-command`
and `flakes` features turned on.

```sh
nix develop
```

It holds:

- the stable Rust toolchain pinned in `rust-toolchain.toml`, which builds Aerith
  ([ADR-0011](docs/adr/0011-a-pinned-stable-toolchain.md));
- `cargo deny`, which enforces the dependency policy of `deny.toml`;
- `git`, with which the checks list the repository's files and cargo deny fetches the advisories;
- `lychee`, which checks links.

The first entry downloads the toolchain, about 150 MB.

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
fails. It refuses to start when a tool is missing or when `rustc` is not the version that
`rust-toolchain.toml` pins. CI runs it unchanged on each push to `main`, each pull request and once
a week.

The checks read the files git knows: `git add` a new file before checking it. Nix, too, sees a new
file only once it is added.

| Step | What it checks | Alone |
|---|---|---|
| links | Links in Markdown files and in Rust's documentation, anchors included, offline; GitHub URLs into this repository are checked against the checkout | the step in `scripts/check.sh` |
| AGENTS.md length | `AGENTS.md` under 1,500 words | `wc -w AGENTS.md` |
| scripts | The syntax of each shell script: a `.sh` file, or a file whose shebang names `sh` or `bash` | `bash -n <script>` |
| MPL-2.0 headers | Each file starts with the MPL-2.0 header ([below](#a-new-file)), but documents, `LICENSE`, the logo and lock files | the step in `scripts/check.sh` |
| formatting | rustfmt's format | `cargo fmt --all --check` |
| lints | Clippy and the compiler's lints of `Cargo.toml`, warnings as errors | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| tests | Every test | `cargo test --workspace --locked` |
| documentation | rustdoc on public and private items, warnings as errors | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked` |
| dependencies | Licences, RustSec advisories and sources, by `deny.toml`; fetches the advisories | `cargo deny --locked check` |
| each check fails when it should | Breaks a scratch copy of the repository in each way a check must catch, and expects that check to stop it | `scripts/test-check.sh` |

External links are not checked: they fail for reasons unrelated to a change. When you add a check,
or a way a check must fail, add its case to `scripts/test-check.sh`. Each case rebuilds Aerith's own
crates in its copy; when that grows slow, the cases that reach cargo will need a lighter way.

CI also runs the lints, the tests and the documentation on Windows, with rustup, since Windows is a
platform of Stage 1.

How long a full check takes is in
[`docs/measurements.md`](docs/measurements.md#m11--the-workspace-and-its-checks).

## A new file

Each file starts with the MPL-2.0 header, after a shebang if it has one:

```rust
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
```

with `#` in place of `//` where `#` starts a comment: shell, Nix, TOML, YAML. Documents, `LICENSE`,
the logo and lock files are exempt; a new kind of file that cannot carry a comment is exempted by
name in `scripts/check.sh`.

## A new dependency

Ask the owner first, with its reason, the alternative weighed and its licence
([`AGENTS.md`](AGENTS.md#code-rules)). `deny.toml` lists the licences that may enter Aerith's
binaries ([ADR-0002](docs/adr/0002-mpl-2-0.md)): a crate under another licence fails the check. A
crate under the LGPL, even one that a dependency brings, is admitted by name only, after a review
that says how each build of Aerith meets its licence.

## Bumping the toolchain

In one commit of its own ([ADR-0011](docs/adr/0011-a-pinned-stable-toolchain.md)):

1. `nix flake update rust-overlay`, so that the shell knows the new version;
2. change `channel` in `rust-toolchain.toml` and `rust-version` in `Cargo.toml`;
3. run the checks.
