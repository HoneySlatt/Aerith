# 0011. A pinned stable toolchain; a nightly only for the checks that need one

- Status: Accepted
- Date: 2026-11-11

## Context

Aerith is written in Rust. Its checks run in a Nix development shell on Linux, and CI also builds
it on Windows, where there is no Nix; contributors without Nix use rustup. Some later checks need a
nightly compiler: `cargo fuzz` for the parsers of untrusted input (Stage 2), Miri for `unsafe`
code. No check of Stage 1's first milestone needs one. Nixpkgs packages stable Rust only, at the
version of the nixpkgs it locks.

The owner's other projects pin their toolchain: Magnolia (its ADR-0013) and SlimeEngine use
`rust-toolchain.toml` and rust-overlay, Rust's own builds by version or by date.

## Decision

**Stable Rust builds Aerith**, at a version pinned in `rust-toolchain.toml` (1.99.1 on 2026-11-11).
The development shell reads that file through rust-overlay, pinned by `flake.lock`; rustup reads
the same file, on CI's Windows job and for contributors without Nix. `rust-version` in `Cargo.toml`
follows it. Edition 2024.

**A nightly pinned by date runs only the checks that need one**, and never builds what users run.
It enters `flake.nix` with the first such check, not before: none needs it yet.

**The toolchains are bumped on purpose**, at each stage's end with the dependencies, or when a check
needs a newer one, each bump in its own commit.

## Alternatives considered

- **A nightly in the shell from the start**, as Magnolia has: ready for fuzzing and Miri, but a
  second toolchain, about 150 MB downloaded and 1 GB on disk, in each new shell and each CI run,
  for a tool nothing uses before Stage 2.
- **Rust from nixpkgs, unpinned**: one flake input fewer, but the compiler would change with each
  nixpkgs update, no nightly would be available, and rustup on Windows would have no version to read.
- **fenix in place of rust-overlay**: the same toolchains, but a second way beside the family's.
- **rustup outside Nix on Linux too**: no Nix input, but the shell would no longer hold every tool
  the checks use.

## Consequences

- One file says which compiler builds Aerith, on every system.
- The first check that needs a nightly adds it to `flake.nix`, with its date, and says so in
  `CONTRIBUTING.md`.
- A new stable Rust is not taken until someone bumps the pin.
