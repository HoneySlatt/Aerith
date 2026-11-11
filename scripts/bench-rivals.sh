#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Runs the bench's set of measures on every rival (ADR-0013) and prints the table that compares
# them. Run it inside the bench's shell, on the machine's own desktop session, with nothing else on
# the screen: each rival takes the screen, fullscreen, for the length of its runs. No sound plays.
#
#   nix develop .#bench --command scripts/bench-rivals.sh <set> <corpus> <display> [rival...]
#
# <corpus> is the folder scripts/corpus.sh made; <display> says what the screen is, for the
# results ("DP-3 3840x2160@240"), as the bench cannot read it. Without rivals named, it runs all
# four. The sets, for all four rivals:
#   desktop  the full set on the high tier: about two and a half hours;
#   short    the same measures, shorter: about fifty minutes;
#   laptop   time to the first frame, memory, power and cadence at 23.976 on the lowest laptop:
#            about one hour.
# Each result says how many cores other programs used during the run (`others_cores`): a run
# beside a build is to be taken again.
# BENCH_SECONDS shortens every run to that many seconds, to try the script itself.
# BENCH_MPV_OPTIONS adds options to mpv alone's runs, as `name=value` separated by spaces: in
# agent-headless, without a GPU, mpv needs `gpu-sw=yes`.
# Results go to target/bench/<set>-<date>/, one folder per run, and table.md beside them.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { echo "usage: scripts/bench-rivals.sh <desktop|short|laptop> <corpus> <display> [rival...]" >&2; exit 2; }
[ $# -ge 3 ] || usage
set_name=$1 corpus=$2 display=$3
shift 3
case "$set_name" in
    desktop) heavy=300 cadence=600 film_only=false report=20 ;;
    short) heavy=120 cadence=120 film_only=false report=10 ;;
    laptop) heavy=300 cadence=600 film_only=true report=0 ;;
    *) usage ;;
esac
rivals=("$@")
[ ${#rivals[@]} -gt 0 ] || rivals=(mpv kodi stremio jellyfin)
[ -d "$corpus" ] || { echo "$corpus: no such corpus; make it with scripts/corpus.sh" >&2; exit 1; }

cargo build --release --locked -p aerith-bench
bench=${CARGO_TARGET_DIR:-target}/release/aerith-bench
out="target/bench/$set_name-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$out"
failures=0

# measure <rival> <clip> <seconds> [bench options]: one run; a failed run is reported and counted,
# and the set goes on.
measure() {
    local rival=$1 clip=$2 seconds=${BENCH_SECONDS:-$3}
    shift 3
    if [ ! -e "$corpus/$clip" ]; then
        echo "missing clip $corpus/$clip" >&2
        failures=$((failures + 1))
        return
    fi
    local extra=()
    if [ "$rival" = mpv ]; then
        for option in ${BENCH_MPV_OPTIONS:-}; do extra+=(--mpv-option "$option"); done
    fi
    echo "== $rival, $clip, $seconds s $*"
    local status=0
    "$bench" run "$rival" "$corpus/$clip" --seconds "$seconds" --out "$out" --display "$display" \
        "${extra[@]}" "$@" || status=$?
    if [ "$status" -eq 3 ]; then
        # A process of the run opened a sound device: stop everything before anything else plays.
        echo "STOPPED: a process of the run opened a sound device; the set ends here" >&2
        exit 3
    elif [ "$status" -ne 0 ]; then
        echo "FAILED: $rival $clip $*" >&2
        failures=$((failures + 1))
    fi
}

for rival in "${rivals[@]}"; do
    # The time to the first frame, three times each from a file and from a stream, without the
    # idle power reading that the figure does not need.
    for _ in 1 2 3; do
        measure "$rival" hdr10-2160p-hevc10-eac3.mkv 10 --idle 0
        measure "$rival" hdr10-2160p-hevc10-eac3.mkv 10 --idle 0 --stream
    done
    # Memory and power: a 4K HDR10 remux's bitrate.
    measure "$rival" heavy-hdr10-2160p-hevc10-truehd.mkv "$heavy"
    # Cadence and sync, at film rate, then at television rates.
    measure "$rival" cadence-23.976-1080p-h264-aac.mkv "$cadence"
    if ! $film_only; then
        measure "$rival" cadence-59.94-1080p-h264-aac.mkv "$cadence"
        measure "$rival" cadence-25-1080p-h264-aac.mkv "$((cadence / 2))"
        # What each engine reports of every colour clip and every sound codec.
        for clip in sdr-2160p-hevc10-eac3.mkv hdr10plus-2160p-hevc10-eac3.mkv hlg-2160p-hevc10-eac3.mkv \
            dv5-2160p-hevc10-eac3.mkv dv8.1-2160p-hevc10-eac3.mkv dv8.4-2160p-hevc10-eac3.mkv \
            real-cosmos-laundromat-hdr10.mkv audio-eac3-5.1.mkv audio-truehd-5.1.mkv audio-dts-5.1.mkv \
            audio-dtshd-ma-7.1.mkv; do
            measure "$rival" "$clip" "$report" --idle 0
        done
        # The clips under a second long: Dolby Vision profile 7, DTS:X, TrueHD Atmos.
        for clip in dv7-1080p.ts dv7-2160p.mkv audio-dtsx-7.1.4.mkv audio-truehd-atmos-7.1.mkv; do
            measure "$rival" "$clip" 5 --idle 0
        done
    fi
done

results=("$out"/*/result.json)
if [ -e "${results[0]}" ]; then
    "$bench" table "${results[@]}" | tee "$out/table.md"
else
    echo "no run gave a result" >&2
fi
echo "results in $out; $failures run(s) failed"
[ "$failures" -eq 0 ]
