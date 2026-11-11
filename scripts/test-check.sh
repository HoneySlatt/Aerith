#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Proves that scripts/check.sh fails when it should: each case breaks a scratch copy of the
# repository one way, runs the check there, and expects it to stop at a named step, or to pass.
# scripts/check.sh runs it as its last step; it runs alone too, inside the development shell.
set -euo pipefail
cd "$(dirname "$0")/.."

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
copy="$scratch/copy"
# One target directory for every case, so that each one rebuilds only Aerith's own crates.
export CARGO_TARGET_DIR="$scratch/target"
# Stops the copies from running these cases again.
export AERITH_CHECK_INNER=1

failures=0

# expect <name> <step, or "pass"> <commands that break the copy, run inside it>
expect() {
    local name=$1 expected=$2 breakage=$3 output status last
    rm -rf "$copy"
    mkdir -p "$copy"
    # What git knows, as it is in the working tree, and the repository itself.
    git -c core.quotePath=false ls-files -z --cached |
        while IFS= read -r -d '' file; do if [ -e "$file" ]; then printf '%s\0' "$file"; fi; done |
        tar --null -T - -cf - .git | tar -xf - -C "$copy"
    (cd "$copy" && eval "$breakage")
    status=0
    output=$(cd "$copy" && scripts/check.sh 2>&1) || status=$?
    last=$(grep '^==> ' <<<"$output" | tail -n 1)
    if [ "$expected" = pass ] && [ "$status" -eq 0 ]; then
        echo "ok    $name: passes"
    elif [ "$expected" != pass ] && [ "$status" -ne 0 ] && [ "$last" = "==> $expected" ]; then
        echo "ok    $name: fails at '$expected'"
    else
        echo "FAIL  $name: expected '$expected', got exit $status at '$last'" >&2
        failures=$((failures + 1))
    fi
}

header='# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.'
lib=crates/aerith-player/src/lib.rs

expect "the repository as it is" pass ":"
expect "a tracked file deleted, not yet staged" pass "rm .gitignore"
expect "a document whose name is not ASCII" pass \
    "echo '# Glossaire' > docs/glossaire-é.md && git add docs/glossaire-é.md"

expect "a broken link in a document" "links" "echo '[x](nowhere.md)' >> README.md"
expect "a broken link to the repository in Rust's documentation" "links" \
    "sed -i 's|0004-a-player-engine-per-platform.md|0004-no-such-adr.md|' $lib"

expect "AGENTS.md too long" "AGENTS.md length" "printf 'word %.0s' {1..1500} >> AGENTS.md"

expect "a .sh script that does not parse" "scripts" \
    "printf '%s\nif then\n' \"\$header\" > scripts/bad.sh && git add scripts/bad.sh"
expect "a script without extension that does not parse" "scripts" \
    "printf '#!/usr/bin/env bash\n%s\nif then\n' \"\$header\" > scripts/bad && git add scripts/bad"

expect "no header in Rust" "MPL-2.0 headers" "sed -i 1,2d $lib"
expect "no header in a workflow" "MPL-2.0 headers" "sed -i 1,2d .github/workflows/ci.yml"
expect "no header in a new Python script" "MPL-2.0 headers" \
    "echo 'print(1)' > scripts/bench.py && git add scripts/bench.py"
expect "a header that denies the licence" "MPL-2.0 headers" \
    "sed -i '1s|.*|// This file is not under the Mozilla Public License, v. 2.0.|' $lib"

expect "unformatted Rust" "formatting" "echo 'fn  f( ){}' >> $lib"
expect "an undocumented public item" "lints" "echo 'pub fn f() {}' >> $lib"
expect "unsafe without a SAFETY comment" "lints" \
    "printf '/// Doc.\npub fn f() -> u8 {\n    let x = 1u8;\n    unsafe { *std::ptr::addr_of!(x) }\n}\n' >> $lib"
expect "a failing test" "tests" "printf '#[test]\nfn fails() {\n    panic!(\"expected\");\n}\n' >> $lib"
expect "a broken link in a private item's documentation" "documentation" \
    "printf '/// See [Nowhere].\n#[allow(dead_code)]\nfn f() {}\n' >> $lib"

expect "a crate under the GPL" "dependencies (licences, advisories, sources)" \
    "sed -i 's/^license = \"MPL-2.0\"$/license = \"GPL-3.0-only\"/' Cargo.toml"
expect "a crate under the LGPL without its exception" "dependencies (licences, advisories, sources)" \
    "sed -i 's/^license = \"MPL-2.0\"$/license = \"LGPL-3.0-only\"/' Cargo.toml"

if [ "$failures" -gt 0 ]; then
    echo "$failures case(s) of scripts/check.sh behaved wrongly" >&2
    exit 1
fi
