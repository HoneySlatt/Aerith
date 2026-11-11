#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Every automated check of the definition of done (AGENTS.md), which CI runs unchanged inside the
# development shell (flake.nix): `nix develop --command scripts/check.sh`. CONTRIBUTING.md says what
# each step checks and how to run one alone.
set -euo pipefail
cd "$(dirname "$0")/.."

# Outside the shell, cargo deny or lychee may be missing, and cargo may be a compiler other than the
# one rust-toolchain.toml pins.
for tool in git cargo rustc cargo-deny lychee; do
    command -v "$tool" >/dev/null || {
        echo "$tool is missing; run this script inside the development shell:" \
            "nix develop --command scripts/check.sh" >&2
        exit 1
    }
done
pinned=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
if [ "$(rustc --version | cut -d' ' -f2)" != "$pinned" ]; then
    echo "rustc is $(rustc --version), not the $pinned that rust-toolchain.toml pins;" \
        "run this script inside the development shell: nix develop --command scripts/check.sh" >&2
    exit 1
fi

step() { printf '\n==> %s\n' "$*"; }

# The files git knows, tracked or added, that still exist, one per line, names unquoted. A new file
# is checked once it is added: `git add` it first.
mapfile -t repository_files < <(
    git -c core.quotePath=false ls-files -z --cached |
        while IFS= read -r -d '' file; do if [ -e "$file" ]; then printf '%s\n' "$file"; fi; done
)

step "links"
# Markdown, and Rust's documentation, whose links into this repository are written as GitHub URLs:
# those are checked against the checkout. Other external links are not checked: they fail for
# reasons unrelated to a change.
linked=()
for file in "${repository_files[@]}"; do
    case "$file" in *.md | *.rs) linked+=("$file") ;; esac
done
lychee --offline --include-fragments --no-progress \
    --remap "https://github.com/HoneySlatt/Aerith/blob/main/(.*) file://$PWD/\$1" "${linked[@]}"

step "AGENTS.md length"
words=$(wc -w < AGENTS.md)
if [ "$words" -gt 1500 ]; then
    echo "AGENTS.md has $words words; keep it under 1,500 (the project method)." >&2
    exit 1
fi

# A shell script is a `.sh` file or a file whose shebang names sh or bash.
step "scripts"
shell_shebang='^#!.*[/ ](ba)?sh$'
for file in "${repository_files[@]}"; do
    first_line=
    read -r first_line < "$file" || true
    if [[ "$file" == *.sh || "$first_line" =~ $shell_shebang ]]; then
        bash -n "$file"
    fi
done

# AGENTS.md: every file starts with the MPL-2.0 header, after a shebang if it has one. The files
# that cannot carry it are exempted by name; any other file, whatever its kind, must.
step "MPL-2.0 headers"
header_first='This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of'
header_second='the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.'
has_header() {
    local first second
    { read -r first; [[ "$first" == '#!'* ]] && read -r first; read -r second; } < "$1" || true
    [[ "$first" == "// $header_first" && "$second" == "// $header_second" ]] ||
        [[ "$first" == "# $header_first" && "$second" == "# $header_second" ]]
}
missing=0
for file in "${repository_files[@]}"; do
    case "$file" in
        # Documents, the licence itself, the logo (ADR-0010) and generated lock files.
        *.md | LICENSE | logo/* | *.lock) continue ;;
    esac
    if ! has_header "$file"; then
        echo "$file: does not start with the MPL-2.0 header (CONTRIBUTING.md, A new file)" >&2
        missing=1
    fi
done
[ "$missing" -eq 0 ]

step "formatting"
cargo fmt --all --check

step "lints"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "tests"
cargo test --workspace --locked

step "documentation"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked

step "dependencies (licences, advisories, sources)"
cargo deny --locked check

# Inside the scratch copies that scripts/test-check.sh makes, this step would start them again.
if [ -z "${AERITH_CHECK_INNER:-}" ]; then
    step "each check fails when it should"
    scripts/test-check.sh
fi

step "all checks passed"
