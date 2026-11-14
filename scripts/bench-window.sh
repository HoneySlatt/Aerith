#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Measures Aerith inside its window, through aerith-testbed, against mpv alone (milestones M1.4b
# and M1.4c), at the refresh rate the screen has now. Run it inside the bench's shell, on the machine's own
# desktop session, with nothing else on the screen: each run takes the screen, fullscreen. No sound
# plays: the players play into the bench's own silent PipeWire (ADR-0013).
#
#   nix develop .#bench --command scripts/bench-window.sh <corpus> <display> [part...]
#
# <corpus> is the folder scripts/corpus.sh made; <display> says what the screen is now, for the
# results ("DP-3 3840x2160@240", "DP-3 3840x2160@60", "DP-3 3840x2160 VRR"). The parts, all of
# them unless some are named:
#   cadence  every SDR clip at 23.976 to 60 fps, and the SDR 4K clip: mpv alone, then Aerith in
#            each order of the surfaces;
#   toggles  Aerith in each order, the controls shown and hidden 60 times on the 23.976 clip;
#   sync     mpv alone under display sync (video-sync=display-resample) on the 23.976, 25 and
#            59.94 clips. Aerith chooses its sync by the display's rate (ADR-0021): each of its
#            sources starts under audio sync, whatever mpv's command line says;
#   rule     M1.4c's measure of that rule: every SDR clip at 23.976 to 60 fps, and the SDR 4K
#            clip: mpv alone under audio sync, mpv alone under display sync, then Aerith in the
#            order above (ADR-0019), which picks its own;
#   frames   three frames of the SDR 4K clip and of the 23.976 clip, captured by mpv alone and by
#            Aerith's mpv, and compared (PSNR; "inf" is the same picture). mpv alone's on-screen
#            controller is turned off, as Aerith's is: it shows over the first frame after a load.
# BENCH_SECONDS sets the length of each playing run (120 by default). With it at 120, the parts
# take about 55, 5, 7, 3 and 55 minutes; at 60, about half.
# BENCH_MPV_OPTIONS adds options to every mpv, alone and Aerith's, as `name=value` separated by
# spaces: in agent-headless, without a GPU, mpv needs `gpu-sw=yes`.
# Results go to target/bench/window-<date>/, one folder per run, and table.md beside them.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { echo "usage: scripts/bench-window.sh <corpus> <display> [cadence|toggles|sync|frames|rule]..." >&2; exit 2; }
[ $# -ge 2 ] || usage
corpus=$1 display=$2
shift 2
parts=("$@")
[ ${#parts[@]} -gt 0 ] || parts=(cadence toggles sync frames)
for part in "${parts[@]}"; do
    case "$part" in cadence | toggles | sync | frames | rule) ;; *) usage ;; esac
done
[ -d "$corpus" ] || { echo "$corpus: no such corpus; make it with scripts/corpus.sh" >&2; exit 1; }
seconds=${BENCH_SECONDS:-120}

cargo build --release --locked -p aerith-bench -p aerith-testbed
release=$(realpath "${CARGO_TARGET_DIR:-target}/release")
# The bench starts the testbed by name, inside its sandbox.
export PATH="$release:$PATH"
bench=$release/aerith-bench
out="target/bench/window-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$out"
failures=0

# measure <player> <clip> <seconds> [bench options]: one run; a failed run is reported and
# counted, and the set goes on.
measure() {
    local player=$1 clip=$2 length=$3
    shift 3
    if [ ! -e "$corpus/$clip" ]; then
        echo "missing clip $corpus/$clip" >&2
        failures=$((failures + 1))
        return
    fi
    local extra=()
    for option in ${BENCH_MPV_OPTIONS:-}; do extra+=(--mpv-option "$option"); done
    echo "== $player, $clip, $length s $*"
    local status=0
    "$bench" run "$player" "$corpus/$clip" --seconds "$length" --out "$out" --display "$display" \
        "${extra[@]}" "$@" || status=$?
    if [ "$status" -eq 3 ]; then
        echo "STOPPED: a process of the run opened a sound device; the set ends here" >&2
        exit 3
    elif [ "$status" -ne 0 ]; then
        echo "FAILED: $player $clip $*" >&2
        failures=$((failures + 1))
    fi
}

# each <clip> <seconds> [bench options]: mpv alone, then Aerith in each order.
each() {
    local clip=$1 length=$2
    shift 2
    measure mpv "$clip" "$length" "$@"
    measure aerith "$clip" "$length" --order below "$@"
    measure aerith "$clip" "$length" --order above "$@"
}

for part in "${parts[@]}"; do
    case "$part" in
        cadence)
            for rate in 23.976 24 25 29.97 50 59.94 60; do
                each "cadence-$rate-1080p-h264-aac.mkv" "$seconds"
            done
            each sdr-2160p-hevc10-eac3.mkv "$((seconds < 55 ? seconds : 55))"
            ;;
        toggles)
            # Shown 1 s and hidden 1 s each time, from 1 s after the first frame.
            for order in below above; do
                measure aerith cadence-23.976-1080p-h264-aac.mkv 125 --order "$order" --toggle-controls 60
            done
            ;;
        sync)
            for rate in 23.976 25 59.94; do
                measure mpv "cadence-$rate-1080p-h264-aac.mkv" "$seconds" --mpv-option video-sync=display-resample
            done
            ;;
        rule)
            for clip in cadence-{23.976,24,25,29.97,50,59.94,60}-1080p-h264-aac.mkv sdr-2160p-hevc10-eac3.mkv; do
                length=$seconds
                [ "$clip" != sdr-2160p-hevc10-eac3.mkv ] || length=$((seconds < 55 ? seconds : 55))
                measure mpv "$clip" "$length"
                measure mpv "$clip" "$length" --mpv-option video-sync=display-resample
                measure aerith "$clip" "$length" --order above
            done
            ;;
        frames)
            for clip in sdr-2160p-hevc10-eac3.mkv cadence-23.976-1080p-h264-aac.mkv; do
                # Aerith's testbed quits on its own clock, the run's length after its first frame:
                # long enough for three exact seeks and captures on a slow machine.
                measure mpv "$clip" 60 --idle 0 --frames 5,20,40 --mpv-option osc=no
                measure aerith "$clip" 60 --idle 0 --frames 5,20,40 --mpv-option osc=no
            done
            ;;
    esac
done

results=("$out"/*/result.json)
if [ -e "${results[0]}" ]; then
    "$bench" table "${results[@]}" | tee "$out/table.md"
else
    echo "no run gave a result" >&2
fi

# The frames: each of Aerith's against mpv alone's, the same clip and time.
for aerith in "$out"/*-aerith-*/frame-*.png; do
    [ -e "$aerith" ] || break
    run=$(basename "$(dirname "$aerith")")
    clip=${run#*-aerith-}
    reference=$(ls "$out"/*-mpv-"$clip"/"$(basename "$aerith")" 2>/dev/null | head -n 1)
    if [ -n "$reference" ]; then
        psnr=$(ffmpeg -hide_banner -nostdin -i "$reference" -i "$aerith" -lavfi psnr -f null - 2>&1 |
            grep -o 'average:[^ ]*' || echo "not compared")
        echo "frame $clip $(basename "$aerith"): PSNR $psnr" | tee -a "$out/frames.md"
    fi
done
echo "results in $out; $failures run(s) failed"
[ "$failures" -eq 0 ]
