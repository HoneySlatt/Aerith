#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
# the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Makes the test corpus that the bench plays (ADR-0012): it fetches the few clips no free tool can
# make, checks them against their pinned SHA-256, generates the rest, then checks that every clip
# carries what its name promises. Run it inside the bench's shell:
#
#   nix develop .#bench --command scripts/corpus.sh [--quick] <directory>
#
# --quick makes every generated clip a few seconds long, to try the bench end to end; its clips are
# not for measuring. The corpus is never committed. Each clip is written under a temporary name and
# renamed once whole, so an interrupted run leaves no half clip; a clip already present is kept.
set -euo pipefail

usage() { echo "usage: scripts/corpus.sh [--quick] <directory>" >&2; exit 2; }
quick=false
if [ "${1:-}" = --quick ]; then quick=true; shift; fi
[ $# -eq 1 ] || usage
corpus=$1

for tool in ffmpeg ffprobe dovi_tool hdr10plus_tool mkvmerge jq curl sha256sum awk; do
    command -v "$tool" >/dev/null || {
        echo "$tool is missing; run this inside the bench's shell: nix develop .#bench" >&2
        exit 1
    }
done
[ -n "${AERITH_BENCH_FONT:-}" ] || { echo "AERITH_BENCH_FONT is unset: use nix develop .#bench" >&2; exit 1; }

mkdir -p "$corpus/sources"
# A quick corpus and a full one never mix: their clips share names, not lengths.
kind=full
if $quick; then kind=quick; fi
if [ -f "$corpus/.kind" ] && [ "$(cat "$corpus/.kind")" != "$kind" ]; then
    echo "$corpus holds a $(cat "$corpus/.kind") corpus; make the $kind one in another folder" >&2
    exit 1
fi
echo "$kind" >"$corpus/.kind"
work=$(mktemp -d "$corpus/.work.XXXXXX")
trap 'rm -rf "$work"' EXIT

# --- Fetched sources -------------------------------------------------------------------------------
# What no free tool can make, each pinned by its SHA-256 (ADR-0012 gives their sources and terms).
# They stay on this machine: the corpus is never published.
fate=https://fate-suite.ffmpeg.org
netflix=https://s3.amazonaws.com/download.opencontent.netflix.com
sources=(
    "dovi-p7.ts $fate/mpegts/dovi-p7.ts 79f741541ec23818018382caf5d10fa71e5a9b07d8552bdaa2a87c16787d0548"
    "dovi-p7-hvce.mkv $fate/mkv/dovi-p7-hvce.mkv 3287bac8deade11e1c57bd65b9c7c8750a1e38e286666cbb4fa39eac264875df"
    "master_audio_7.1_24bit.dts $fate/dts/master_audio_7.1_24bit.dts 227f1008f9529b2bdf937ce856d2562d0128162ac930e2579ef4b8fd7474ee18"
    "dtsx_7.1.4_24bit.dts $fate/dts/dtsx_7.1.4_24bit.dts f3b5ac8a758fbf341805d00878a91507c4c663f9466a1bb56a5a3a93813da517"
    "atmos.thd $fate/truehd/atmos.thd 020c9b9e67f80de105663af191740f8b60f550107c9f765b632f4cd5efb1c6ab"
    "CosmosLaundromat_2k24p_HDR_P3PQ.mp4 $netflix/CosmosLaundromat/CosmosLaundromat_2k24p_HDR_P3PQ.mp4 4762d0d77ce6c09371032e0c6b4dbda8ad781237b5264829857eefeed7a3b40a"
)
for entry in "${sources[@]}"; do
    read -r name url sum <<<"$entry"
    target="$corpus/sources/$name"
    if [ ! -f "$target" ]; then
        echo "fetching $name"
        curl --fail --silent --show-error --location --max-time 3600 -o "$work/$name" "$url"
        # Checked before it takes its place: a wrong file never enters the corpus.
        echo "$sum  $work/$name" | sha256sum --check --quiet || {
            echo "$url does not match its pinned SHA-256; the source changed" >&2
            exit 1
        }
        mv "$work/$name" "$target"
    fi
    echo "$sum  $target" | sha256sum --check --quiet || {
        echo "$target does not match its pinned SHA-256; delete it and run again" >&2
        exit 1
    }
done

# --- Generated clips -------------------------------------------------------------------------------
# Every generated clip shows a moving pattern with its frame number and time, a white band for one
# frame at each beat, and a 20 ms beep at the same instant on every channel, so that an eye, an ear
# or a capture can check the cadence and the sync. A beat lasts round(fps) frames: 1 s at an integer
# rate, 1.001 s at 23.976, so that the flash and the beep stay aligned for the whole clip.

# frames <rate> <seconds>: the whole number of frames that covers the duration.
frames() { awk -v r="$1" -v s="$2" 'BEGIN { split(r, f, "/"); d = (f[2] == "") ? 1 : f[2]; n = s * f[1] / d; print (n == int(n)) ? n : int(n) + 1 }'; }
# beat <rate>: frames per beat.
beat() { awk -v r="$1" 'BEGIN { split(r, f, "/"); d = (f[2] == "") ? 1 : f[2]; printf "%d", f[1] / d + 0.5 }'; }
# seconds <rate> <frames>: the exact duration of that many frames.
seconds() { awk -v r="$1" -v n="$2" 'BEGIN { split(r, f, "/"); d = (f[2] == "") ? 1 : f[2]; printf "%.6f", n * d / f[1] }'; }

# pattern <size> <rate>: the lavfi graph of the pattern, in 8-bit BT.709.
pattern() {
    local beat_frames
    beat_frames=$(beat "$2")
    printf '%s' "testsrc2=size=$1:rate=$2,format=yuv420p,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv,"
    printf '%s' "drawtext=fontfile=$AERITH_BENCH_FONT:fontsize=h/12:fontcolor=white:box=1:boxcolor=black:x=w/20:y=h/5:"
    printf '%s' "text='%{frame_num}  %{pts\\:hms}',"
    printf '%s' "drawbox=x=0:y=0:w=iw:h=ih/8:color=white:t=fill:enable=eq(mod(n\\,$beat_frames)\\,0)"
}

# beep_source <rate> <channel layout> <channels> <seconds>: the lavfi source of the beeps.
beep_source() {
    local period expr exprs
    period=$(awk -v r="$1" -v b="$(beat "$1")" 'BEGIN { split(r, f, "/"); d = (f[2] == "") ? 1 : f[2]; printf "%.9f", b * d / f[1] }')
    expr="if(lt(mod(t\\,$period)\\,0.02)\\,0.5*sin(2*PI*1000*t)\\,0)"
    exprs=$expr
    for ((c = 1; c < $3; c++)); do exprs="$exprs|$expr"; done
    printf '%s' "aevalsrc=exprs=$exprs:channel_layout=$2:sample_rate=48000:duration=$4"
}

# encode_audio <codec> <rate> <seconds> <output>: the beeps in one of the corpus's audio codecs.
encode_audio() {
    local args layout channels
    case "$1" in
        aac) args=(-c:a aac -b:a 192k); layout=stereo; channels=2 ;;
        eac3) args=(-c:a eac3 -b:a 640k); layout='5.1(side)'; channels=6 ;;
        truehd) args=(-strict experimental -c:a truehd); layout='5.1(side)'; channels=6 ;;
        dts) args=(-strict experimental -c:a dca -b:a 1509k); layout='5.1(side)'; channels=6 ;;
        *) echo "unknown audio codec $1" >&2; exit 1 ;;
    esac
    ffmpeg -nostdin -hide_banner -loglevel error -y -f lavfi -i "$(beep_source "$2" "$layout" "$channels" "$3")" \
        "${args[@]}" "$4"
}

# The colour of each kind of clip: the conversion from the BT.709 pattern, and x265's signalling.
# The pattern's brightest code read back at 637 of 1023, about 350 cd/m² in PQ, well below the
# mastering display's peak.
mastering='master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50):max-cll=1000,400'
to_pq='zscale=tin=bt709:pin=bt709:min=bt709:rin=tv:t=smpte2084:p=bt2020:m=bt2020nc:r=tv:npl=203'
to_hlg='zscale=tin=bt709:pin=bt709:min=bt709:rin=tv:t=arib-std-b67:p=bt2020:m=bt2020nc:r=tv:npl=203'
x265_sdr='colorprim=bt709:transfer=bt709:colormatrix=bt709'
x265_pq="colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:$mastering:hdr10=1:hdr10-opt=0"
x265_hlg='colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc'
# Profile 5 carries IPT-PQ-c2, which no BT.* tag describes: its stream says "unspecified".
x265_dv5='colorprim=unknown:transfer=unknown:colormatrix=unknown:range=full'

# encode_video <kind> <size> <rate> <frames> <output .h264/.hevc> [extra filter] [x265 rate control]
encode_video() {
    local graph codec params
    graph=$(pattern "$2" "$3")${6:+,$6}
    case "$1" in
        h264) codec=(-c:v libx264 -preset veryfast -crf 26 -x264-params 'colorprim=bt709:transfer=bt709:colormatrix=bt709') ;;
        sdr10) graph="$graph,format=yuv420p10le"; params=$x265_sdr ;;
        pq) graph="$graph,$to_pq,format=yuv420p10le"; params=$x265_pq ;;
        hlg) graph="$graph,$to_hlg,format=yuv420p10le"; params=$x265_hlg ;;
        dv5) graph="$graph,$to_pq,format=yuv420p10le"; params=$x265_dv5 ;;
        *) echo "unknown video kind $1" >&2; exit 1 ;;
    esac
    if [ "$1" != h264 ]; then
        codec=(-c:v libx265 -preset ultrafast -x265-params "$params:${7:-crf=22}:repeat-headers=1:aud=1:log-level=error")
    fi
    ffmpeg -nostdin -hide_banner -loglevel error -y -f lavfi -i "$graph" -frames:v "$4" "${codec[@]}" "$5"
}

# rpu <profile> <frames> <output>: Dolby Vision metadata from a generic description (dovi_tool).
rpu() {
    jq -n --arg p "$1" --argjson n "$2" '{
        cm_version: "V40", profile: $p, length: $n,
        level6: { max_display_mastering_luminance: 1000, min_display_mastering_luminance: 50,
                  max_content_light_level: 1000, max_frame_average_light_level: 400 } }' >"$work/rpu.json"
    dovi_tool generate -j "$work/rpu.json" -o "$3" >/dev/null
}

# hdr10plus <frames> <output>: one scene of HDR10+ dynamic metadata, one entry per frame.
hdr10plus() {
    jq -n --argjson n "$1" '{
        JSONInfo: { HDR10plusProfile: "B", Version: "1.0" },
        SceneInfo: [ range($n) as $i | {
            BezierCurveData: { Anchors: [143, 298, 447, 592, 731, 864, 891, 917, 938],
                               KneePointX: 164, KneePointY: 240 },
            LuminanceParameters: {
                AverageRGB: 263,
                LuminanceDistributions: { DistributionIndex: [1, 5, 10, 25, 50, 75, 90, 95, 99],
                                          DistributionValues: [0, 6080, 92, 1, 4, 107, 726, 1784, 5843] },
                MaxScl: [7768, 6589, 6912] },
            NumberOfWindows: 1, TargetedSystemDisplayMaximumLuminance: 400,
            SceneFrameIndex: $i, SceneId: 0, SequenceFrameIndex: $i } ],
        SceneInfoSummary: { SceneFirstFrameIndex: [0], SceneFrameNumbers: [$n] },
        ToolInfo: { Tool: "hdr10plus_tool", Version: "1.7.2" } }' >"$2"
}

# mux <output .mkv> <rate> <video> [audio]: the final clip, written whole or not at all.
mux() {
    local partial="$work/$(basename "$1")"
    mkvmerge --quiet -o "$partial" --default-duration "0:${2}fps" "$3" ${4:+"$4"}
    mv "$partial" "$1"
}

# clip <name> <video kind> <size> <rate> <seconds> <audio codec> [extra]: one generated clip.
# extra is "hdr10plus", "dv8.1" or "dv8.4" (metadata added to the video), or "heavy" (grain and a
# remux's bitrate).
clip() {
    local name=$1 kind=$2 size=$3 rate=$4 seconds=$5 audio=$6 extra=${7:-} n video ext filter="" rc=""
    local output="$corpus/$name.mkv"
    [ -f "$output" ] && return
    if $quick; then seconds=4; fi
    n=$(frames "$rate" "$seconds")
    echo "generating $name ($n frames)"
    ext=hevc
    [ "$kind" = h264 ] && ext=h264
    video="$work/video.$ext"
    if [ "$extra" = heavy ]; then
        filter='noise=alls=12:allf=t'
        rc='bitrate=80000:vbv-maxrate=100000:vbv-bufsize=160000'
    fi
    encode_video "$kind" "$size" "$rate" "$n" "$video" "$filter" "$rc"
    case "$extra" in
        hdr10plus)
            hdr10plus "$n" "$work/hdr10plus.json"
            hdr10plus_tool inject -i "$video" -j "$work/hdr10plus.json" -o "$work/meta.hevc" >/dev/null
            video="$work/meta.hevc" ;;
        dv8.1 | dv8.4 | dv5)
            rpu "${extra#dv}" "$n" "$work/rpu.bin"
            dovi_tool inject-rpu -i "$video" --rpu-in "$work/rpu.bin" -o "$work/meta.hevc" >/dev/null
            video="$work/meta.hevc" ;;
    esac
    local audio_file=""
    if [ "$audio" != none ]; then
        audio_file="$work/audio.mka"
        encode_audio "$audio" "$rate" "$(seconds "$rate" "$n")" "$audio_file"
    fi
    mux "$output" "$rate" "$video" "$audio_file"
    rm -f "$work"/video.* "$work"/meta.hevc "$work"/audio.mka
}

# The cadence clips: 30 minutes at each common rate, 1080p H.264, as most streams are.
# expect_rate <clip> <rate>: its video's rate, within 0.01 frame a second. Matroska stores a frame's
# duration in nanoseconds, so 59.94 reads back as 19001/317.
expect_rate() {
    local found
    found=$(ffprobe -v error -select_streams v:0 -show_entries stream=r_frame_rate -of csv=p=0 "$corpus/$1" 2>&1) || true
    if ! awk -v f="$found" -v r="$2" 'BEGIN {
            split(f, a, "/"); split(r, b, "/"); if (a[2] == "") a[2] = 1; if (b[2] == "") b[2] = 1
            d = a[1] / a[2] - b[1] / b[2]; exit !(a[2] > 0 && d < 0.01 && d > -0.01) }'; then
        echo "$1: expected $2 frames a second, found: $found" >&2
        failed=1
    fi
}
for rate in 24000/1001 24 25 30000/1001 50 60000/1001 60; do
    label=$(awk -v r="$rate" 'BEGIN { split(r, f, "/"); d = (f[2] == "") ? 1 : f[2]; printf "%g", int(f[1] / d * 1000 + 0.5) / 1000 }')
    clip "cadence-$label-1080p-h264-aac" h264 1920x1080 "$rate" 1800 aac
done

# The colour clips: 4K HEVC Main 10 at 23.976, one per range and kind of metadata, E-AC3 5.1.
clip sdr-2160p-hevc10-eac3 sdr10 3840x2160 24000/1001 60 eac3
clip hdr10-2160p-hevc10-eac3 pq 3840x2160 24000/1001 60 eac3
clip hlg-2160p-hevc10-eac3 hlg 3840x2160 24000/1001 60 eac3
clip hdr10plus-2160p-hevc10-eac3 pq 3840x2160 24000/1001 60 eac3 hdr10plus
clip dv5-2160p-hevc10-eac3 dv5 3840x2160 24000/1001 60 eac3 dv5
clip dv8.1-2160p-hevc10-eac3 pq 3840x2160 24000/1001 60 eac3 dv8.1
clip dv8.4-2160p-hevc10-eac3 hlg 3840x2160 24000/1001 60 eac3 dv8.4

# The heavy clip: a 4K HDR10 remux's bitrate (about 80 Mbit/s, grain that x265 cannot smooth away)
# and TrueHD, for memory and power on the lowest laptop.
clip heavy-hdr10-2160p-hevc10-truehd pq 3840x2160 24000/1001 300 truehd heavy

# The passthrough clips: each codec ffmpeg can encode, under a light 1080p picture.
clip audio-eac3-5.1 h264 1920x1080 24000/1001 60 eac3
clip audio-truehd-5.1 h264 1920x1080 24000/1001 60 truehd
clip audio-dts-5.1 h264 1920x1080 24000/1001 60 dts

# --- Clips made from the fetched sources -----------------------------------------------------------

# fetched_audio <name> <source>: a fetched audio stream, intact, under a pattern as long as it is.
fetched_audio() {
    local output="$corpus/$1.mkv" seconds n
    [ -f "$output" ] && return
    echo "making $1"
    seconds=$(ffmpeg -nostdin -hide_banner -i "$corpus/sources/$2" -f null - 2>&1 |
        sed -n 's/.*time=\([0-9:.]*\).*/\1/p' | tail -1 | awk -F: '{ print $1 * 3600 + $2 * 60 + $3 }')
    n=$(frames 24000/1001 "$seconds")
    [ "$n" -ge 1 ] || n=1
    encode_video h264 1920x1080 24000/1001 "$n" "$work/video.mkv"
    # mkvmerge does not recognise a raw TrueHD stream this short; FFmpeg copies both streams intact.
    ffmpeg -nostdin -hide_banner -loglevel error -y -i "$work/video.mkv" \
        -i "$corpus/sources/$2" -map 0:v -map 1:a -c copy "$work/$1.mkv"
    mv "$work/$1.mkv" "$output"
    rm -f "$work/video.mkv"
}
fetched_audio audio-dtshd-ma-7.1 master_audio_7.1_24bit.dts
fetched_audio audio-dtsx-7.1.4 dtsx_7.1.4_24bit.dts
fetched_audio audio-truehd-atmos-7.1 atmos.thd

# Dolby Vision profile 7, two layers: the fetched files, copied as they are.
for pair in "dv7-1080p.ts dovi-p7.ts" "dv7-2160p.mkv dovi-p7-hvce.mkv"; do
    read -r name source <<<"$pair"
    if [ ! -f "$corpus/$name" ]; then
        cp "$corpus/sources/$source" "$work/$name"
        mv "$work/$name" "$corpus/$name"
    fi
done

# Real footage: three minutes of Cosmos Laundromat. Netflix publishes it as 8-bit H.264 whose pixels
# are PQ in P3 but whose stream carries no colour tags; it becomes HDR10 here: BT.2020, PQ, 10-bit,
# its mastering display P3. Its 8-bit origin may band in smooth gradients.
real="$corpus/real-cosmos-laundromat-hdr10.mkv"
if [ ! -f "$real" ]; then
    echo "making real-cosmos-laundromat-hdr10"
    length=180
    if $quick; then length=4; fi
    ffmpeg -nostdin -hide_banner -loglevel error -y -ss 60 -t "$length" \
        -i "$corpus/sources/CosmosLaundromat_2k24p_HDR_P3PQ.mp4" -an \
        -vf "zscale=pin=smpte432:tin=smpte2084:min=bt709:rin=tv:p=bt2020:t=smpte2084:m=bt2020nc:r=tv,format=yuv420p10le" \
        -c:v libx265 -preset fast -x265-params "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:$mastering:hdr10=1:crf=16:log-level=error" \
        "$work/real.hevc"
    mux "$real" 24000/1001 "$work/real.hevc"
    rm -f "$work/real.hevc"
fi

# --- Checks ----------------------------------------------------------------------------------------
# Each clip carries what its name promises, as FFmpeg reads it. A failure here is a bug of this
# script or of a tool, never something to skip.
failed=0
expect() {
    local file="$corpus/$1" pattern=$2 found
    # `|| true`: a file ffprobe cannot read fails the pattern below, with ffprobe's message.
    found=$(ffprobe -v error -show_entries \
        stream=codec_name,profile,width,height,r_frame_rate,pix_fmt,color_transfer,color_primaries,channels:stream_side_data=dv_profile,el_present_flag:format=duration \
        -of compact=nokey=0 "$file" 2>&1 | tr '\n' ' ') || true
    if ! grep -Eq "$pattern" <<<"$found"; then
        echo "$1: expected /$pattern/, found: $found" >&2
        failed=1
    fi
}
# expect_length <clip> <seconds>: the clip lasts that long, to the second.
expect_length() {
    local seconds=$2 found
    if $quick; then seconds=4; fi
    found=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$corpus/$1" 2>&1) || true
    if ! awk -v f="$found" -v s="$seconds" 'BEGIN { exit !(f + 0 > 0 && f - s < 1 && s - f < 1) }'; then
        echo "$1: expected $seconds s, found: $found" >&2
        failed=1
    fi
}
for rate in 24000/1001 24 25 30000/1001 50 60000/1001 60; do
    label=$(awk -v r="$rate" 'BEGIN { split(r, f, "/"); d = (f[2] == "") ? 1 : f[2]; printf "%g", int(f[1] / d * 1000 + 0.5) / 1000 }')
    clip_name="cadence-$label-1080p-h264-aac.mkv"
    expect "$clip_name" 'codec_name=h264.*width=1920.*height=1080.*pix_fmt=yuv420p.*color_transfer=bt709.*codec_name=aac.*channels=2'
    expect_rate "$clip_name" "$rate"
    expect_length "$clip_name" 1800
done
for clip_name in sdr hdr10 hlg hdr10plus dv5 dv8.1 dv8.4; do
    expect "$clip_name-2160p-hevc10-eac3.mkv" 'codec_name=hevc.*width=3840.*height=2160'
    expect_rate "$clip_name-2160p-hevc10-eac3.mkv" 24000/1001
    expect_length "$clip_name-2160p-hevc10-eac3.mkv" 60
done
expect_length heavy-hdr10-2160p-hevc10-truehd.mkv 300
for clip_name in audio-eac3-5.1 audio-truehd-5.1 audio-dts-5.1; do expect_length "$clip_name.mkv" 60; done
expect_length real-cosmos-laundromat-hdr10.mkv 180
expect sdr-2160p-hevc10-eac3.mkv 'codec_name=hevc.*pix_fmt=yuv420p10le.*color_transfer=bt709.*codec_name=eac3.*channels=6'
expect hdr10-2160p-hevc10-eac3.mkv 'pix_fmt=yuv420p10le.*color_transfer=smpte2084.*color_primaries=bt2020'
expect hlg-2160p-hevc10-eac3.mkv 'pix_fmt=yuv420p10le.*color_transfer=arib-std-b67.*color_primaries=bt2020'
expect dv5-2160p-hevc10-eac3.mkv 'dv_profile=5'
expect dv8.1-2160p-hevc10-eac3.mkv 'color_transfer=smpte2084.*dv_profile=8'
expect dv8.4-2160p-hevc10-eac3.mkv 'color_transfer=arib-std-b67.*dv_profile=8'
expect dv7-1080p.ts 'dv_profile=7.*el_present_flag=1'
expect dv7-2160p.mkv 'dv_profile=7.*el_present_flag=1'
expect heavy-hdr10-2160p-hevc10-truehd.mkv 'color_transfer=smpte2084.*codec_name=truehd.*channels=6'
expect audio-eac3-5.1.mkv 'codec_name=eac3.*channels=6'
expect audio-truehd-5.1.mkv 'codec_name=truehd.*channels=6'
expect audio-dts-5.1.mkv 'codec_name=dts.*profile=DTS[^-].*channels=6'
expect audio-dtshd-ma-7.1.mkv 'codec_name=dts.*profile=DTS-HD MA.*channels=8'
expect audio-dtsx-7.1.4.mkv 'codec_name=dts.*profile=DTS-HD MA \+ DTS:X'
expect audio-truehd-atmos-7.1.mkv 'codec_name=truehd.*profile=Dolby TrueHD \+ Dolby Atmos'
expect real-cosmos-laundromat-hdr10.mkv 'pix_fmt=yuv420p10le.*color_transfer=smpte2084.*color_primaries=bt2020'
# HDR10+ lives in SEI messages that ffprobe's stream fields do not show: hdr10plus_tool reads them.
hdr10plus_check="$work/hdr10plus-check.hevc"
ffmpeg -nostdin -hide_banner -loglevel error -y -i "$corpus/hdr10plus-2160p-hevc10-eac3.mkv" -map 0:v -c copy -frames:v 2 \
    -bsf:v hevc_mp4toannexb -f hevc "$hdr10plus_check"
if ! hdr10plus_tool --verify extract "$hdr10plus_check" 2>&1 | grep -q 'Dynamic HDR10+ metadata detected'; then
    echo "hdr10plus-2160p-hevc10-eac3.mkv: no HDR10+ metadata found" >&2
    failed=1
fi
[ "$failed" -eq 0 ] || exit 1
echo "corpus complete in $corpus"
