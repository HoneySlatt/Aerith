#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Every automated check of the definition of done (AGENTS.md), which CI runs unchanged inside the
# development shell (flake.nix): `nix develop --command scripts/check.sh`. CONTRIBUTING.md says what
# each step checks and how to run one alone.
set -euo pipefail
cd "$(dirname "$0")/.."

# Outside the shell, cargo would be whatever toolchain the machine has, and cargo deny and lychee
# would be missing after minutes of other checks.
for tool in cargo cargo-deny lychee; do
    command -v "$tool" >/dev/null || {
        echo "$tool is missing; run this script inside the development shell:" \
            "nix develop --command scripts/check.sh" >&2
        exit 1
    }
done

step() { printf '\n==> %s\n' "$*"; }

# The files of the repository, tracked or new, without the ignored ones.
files() { git ls-files --cached --others --exclude-standard "$@"; }

step "links"
mapfile -t markdown < <(files '*.md')
lychee --offline --include-fragments --no-progress "${markdown[@]}"

step "AGENTS.md length"
words=$(wc -w < AGENTS.md)
if [ "$words" -gt 1500 ]; then
    echo "AGENTS.md has $words words; keep it under 1,500 (the project method)." >&2
    exit 1
fi

step "scripts"
for script in scripts/*.sh; do
    bash -n "$script"
done

# AGENTS.md: every file starts with the MPL-2.0 header. Documents, the licence, the logo and the
# lock files cannot carry it; every file of code or configuration must, after a shebang if any.
step "MPL-2.0 headers"
missing=0
while IFS= read -r file; do
    if ! head -n 3 "$file" | grep -q "Mozilla Public License, v. 2.0"; then
        echo "$file: no MPL-2.0 header in its first lines" >&2
        missing=1
    fi
done < <(files '*.rs' '*.sh' '*.nix' '*.toml' '*.yml' '*.yaml')
[ "$missing" -eq 0 ]

step "formatting"
cargo fmt --all --check

step "lints"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "tests"
cargo test --workspace --locked

step "documentation"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

step "dependencies (licences, advisories, sources)"
cargo deny --locked check

step "all checks passed"
