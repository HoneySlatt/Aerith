#!/usr/bin/env bash
# Every automated check of this repository, which CI runs unchanged inside the development shell
# (flake.nix). No code exists yet: it checks the links between Markdown files, anchors included.
# External links are not checked: they fail for reasons unrelated to the change. M1.1 adds the
# Rust workspace's checks.
set -euo pipefail
cd "$(dirname "$0")/.."

mapfile -t markdown < <(git ls-files --cached --others --exclude-standard '*.md')
lychee --offline --include-fragments --no-progress "${markdown[@]}"
