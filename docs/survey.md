# Survey

The facts behind Aerith's decisions, with their sources, as read on 2026-11-10 and 2026-11-11.
Facts change: each section gives the date its sources were read. A decision that rests on a fact
here links to its section.

## The prototype, Magnolia-media

The owner's earlier prototype, 32 commits from 2026-10-28 to 2026-11-04, kept outside this
repository. Rust and egui; Stremio addons; a Nuvio account; mpv in a separate process and window,
driven over its JSON IPC, with its own controls in Lua; a couch mode driven by a gamepad.

Kept as lessons:

- Prove the picture and the sound before building the interface, with figures.
- Separate what a file claims, what the player did and what reached the display: never show
  "Dolby Vision" or "Atmos" from a file name; read the decoder and the audio path from the engine.
- Hardware decoding only for the exact codec, profile, depth and chroma; software decoding is a
  stated fallback.
- Tokens only in the system's keyring; the data folder private; stream headers kept out of command
  lines; no disk cache of posters; diagnostics stripped of titles, addresses and file names.
- The interface's thread never waits: network, storage, images and IPC elsewhere, in bounded
  queues; stale results dropped.
- Test protocols against local fakes, and state what was checked against real services.
- A resume identity that does not depend on signed URLs.

Avoided:

- Documentation that drifts from the code (two figures were wrong within days).
- Sources shown before the engine can play them (torrents were listed, then refused).
- Work stopped with a large uncommitted change and nothing written about where it stood.
- Renames that leave migration code behind.

## Video on the desktop

Read 2026-11-10.

- mpv 0.42.0 (2026-11-05) needs libplacebo 7.387.0. libmpv's render API draws through the old
  `vo_gpu`, in OpenGL, software or D3D11: "There is currently no support for the libmpv / render
  API in `vo_gpu_next`"
  ([mpv wiki](https://github.com/mpv-player/mpv/wiki/GPU-Next-vs-GPU)). The HDR output hint and
  Dolby Vision's profile 7 enhancement layer are gpu-next's only.
- A Vulkan render API for libmpv was proposed
  ([PR 18258](https://github.com/mpv-player/mpv/pull/18258), closed 2026-07-13 over authorship);
  a maintainer gave the route for one to be accepted: port gpu-next to mpv's render primitives,
  validate, then expose it. The issue asking for one swapchain shared with an interface
  ([18343](https://github.com/mpv-player/mpv/issues/18343)) was closed by its own author, unanswered.
- `--wid` embeds mpv's window on X11, Windows and Android; Wayland embedding would need a new
  subsurface API, which a maintainer called not planned (2024).
- libplacebo can import an application's Vulkan device and wrap its images; it handles HDR10,
  HDR10+ and Dolby Vision metadata; it has no Direct3D 12 backend
  ([libplacebo](https://github.com/haasn/libplacebo)).
- wgpu 30 can wrap a raw Vulkan device; HDR surface colour spaces arrived in 30.0.0, populated on
  Windows, macOS and the web, driver-dependent on Vulkan
  ([changelog](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md)).
- Wayland's colour-management protocol describes each surface on its own; KWin 6.7, Mutter 51,
  Hyprland 0.52 and wlroots 0.20 implement it
  ([wayland.app](https://wayland.app/protocols/color-management-v1)). The owner's compositor,
  Umbriel, on wlroots 0.20.2, exposes `wp_color_manager_v1` (read from its binary on 2026-11-11).
- Jellyfin Desktop, Swiftfin, Streamyfin, Findroid and Stremio's desktop shells play through mpv.
  Stremio's Windows shell draws mpv into the window under a WebView2 interface, which it calls
  2–5 times more efficient than its Qt shell.
- Kodi draws its own player and interface together on the desktop; on Android it places MediaCodec's
  video view under its interface.
- Passthrough: mpv's PipeWire output maps TrueHD, DTS-HD and E-AC3 to IEC 958; WASAPI exclusive
  mode is needed for non-PCM on Windows; mpv on macOS cannot pass TrueHD or DTS-HD through.
- Writing a player means its cache sized in bytes and seconds (30 s of a 150 Mbit/s remux is
  562.5 MB), precise seeking, an audio clock that counts the device's latency, display sync
  (25 fps on 240 Hz alternates 9 and 10 refreshes), passthrough and hardware decoding on each
  system, and subtitles.

## Video on Android

Read 2026-11-10.

- HDR needs a SurfaceView: a TextureView converts HDR to SDR from Android 13
  ([Android HDR](https://developer.android.com/media/platform/hdr-video)).
- Media3 falls back to the HEVC base layer for Dolby Vision profiles 4 and 8, not 5 or 7, and passes
  E-AC3-JOC, DTS-HD, DTS:X and TrueHD through
  ([Media3 formats](https://developer.android.com/media/media3/exoplayer/supported-formats)).
- Nuvio's TV app and Just Player use Media3; Kodi uses MediaCodec and IEC 61937 itself.

## Apple platforms

Read 2026-11-11.

- Only AVPlayer obtains true Dolby Vision, Atmos (E-AC3 JOC), the TV's switch to the content's
  range and rate (`AVDisplayManager`) and AirPlay. HLS on Apple carries Dolby Vision as profile 5,
  or profile 8 on an HEVC base layer
  ([HLS authoring](https://developer.apple.com/documentation/http-live-streaming/hls-authoring-specification-for-apple-devices)).
  AVPlayer does not open Matroska and does not play TrueHD.
- AetherEngine demuxes with FFmpeg and remuxes to HLS-fMP4 for AVPlayer; it claims Dolby Vision
  5, 7 and 8, HDR10+ and E-AC3 JOC passthrough, under LGPL-3.0 with an App Store exception
  ([AetherEngine](https://github.com/superuser404notfound/AetherEngine)). MPVKit calls itself
  "only suitable for learning" ([MPVKit](https://github.com/mpvkit/MPVKit)).
- VLC was removed from the App Store in 2011 over the GPL and returned in 2013 under MPL-2.0; VLC
  for iOS is GPL-2.0-or-later and MPL-2.0
  ([COPYING](https://code.videolan.org/videolan/vlc-ios/-/raw/master/COPYING),
  [FSF](https://www.fsf.org/blogs/licensing/more-about-the-app-store-gpl-enforcement)). Swiftfin and
  Streamyfin are MPL-2.0 on the App Store.
- Uploads need Xcode 26 since 2026-04-28; Xcode 27 "will only install and run on Apple silicon Macs"
  ([Xcode 27](https://developer.apple.com/documentation/xcode-release-notes/xcode-27-release-notes),
  [requirements](https://developer.apple.com/news/upcoming-requirements/)). macOS 26 is the last
  for Intel Macs.
- macOS's licence allows virtual copies only "on each Apple-branded computer you own or control that
  is already running the Apple Software" ([SLA](https://www.apple.com/legal/sla/docs/macOSTahoe.pdf)).
- GitHub's macOS runners, on Apple silicon, are free for public repositories.
- The EU's alternative distribution covers iOS and iPadOS, not tvOS
  ([Apple and the DMA](https://developer.apple.com/support/dma-and-apps-in-the-eu/)).
- The developer programme costs US$99 a year; a free account's apps expire after 7 days.
- For a framework: wgpu's Metal backend supports HDR colour spaces; winit's UIKit backend lacks
  composed text input and tvOS focus; AccessKit has macOS and iOS adapters, none for tvOS; tvOS
  requires custom interfaces to implement focus; nothing requires matching Liquid Glass.

## Stores and third-party rules

Read 2026-11-10 and 2026-11-11.

- Apple's App Review: no downloaded code that changes the app's features (2.5.2); no facilitating
  illegal file sharing (5.2.3); an Apple TV app must work with the Siri Remote alone (2.4.3)
  ([guidelines](https://developer.apple.com/app-store/review/guidelines/), 2026-10-16).
- Google Play: "Don't create an app that encourages or facilitates the infringement of intellectual
  property rights (e.g. unauthorized streaming)"
  ([policy](https://support.google.com/googleplay/android-developer/answer/9888072)).
- Trakt's API policy refuses apps that "promote copyright infringement or piracy"; production access
  is reviewed
  ([policy](https://github.com/trakt/trakt-api/blob/master/projects/developer/src/lib/guides/api-use-policy.md)).
- Stremio offers a limited iOS version and its full version on AltStore PAL
  ([downloads](https://www.stremio.com/downloads)).
- TMDB: free for non-commercial use with attribution; no cache longer than 6 months
  ([terms](https://www.themoviedb.org/api-terms-of-use)).

## Protocols and services

Read 2026-11-10.

- Stremio addons: HTTP JSON at `/{resource}/{type}/{id}[/{extra}].json`; resources `catalog`,
  `meta`, `stream`, `subtitles`, `addon_catalog`, and the event-only `player` and `library`, which
  reach every installed addon that declares them
  ([protocol](https://github.com/Stremio/stremio-addon-sdk/blob/master/docs/protocol.md),
  [API](https://github.com/Stremio/stremio-addon-sdk/blob/master/docs/api/README.md)).
- stremio-core: Rust, MIT, not on crates.io; its `types` reusable, its `Ctx` tied to Stremio's own
  backend ([stremio-core](https://github.com/Stremio/stremio-core)).
- Nuvio: GPL-3.0, Kotlin Multiplatform and Compose; its TV app on Compose for TV and Media3; its
  backend Supabase at `api.nuvio.tv`, self-hostable; Jellyfin, watch parties and an engine on
  libtorrent already shipped ([NuvioMobile](https://github.com/NuvioMedia/NuvioMobile),
  [NuvioTV](https://github.com/NuvioMedia/NuvioTV/releases)).
- Nuvio's JavaScript providers export `getStreams(tmdbId, mediaType, season, episode)` and need
  `fetch`; their helpers are bundled
  ([documentation](https://github.com/tapframe/nuvio-providers/blob/main/DOCUMENTATION.md)).
- Jellyfin publishes its API as OpenAPI (12.x); playback negotiates direct play, direct stream or
  transcoding; progress is reported to `/Sessions/Playing`
  ([OpenAPI](https://api.jellyfin.org/openapi/jellyfin-openapi-stable.json)).
- Plex charges third-party clients for remote streaming
  ([Ars Technica](https://arstechnica.com/gadgets/2025/11/plexs-crackdown-on-free-remote-streaming-access-starts-this-week/)).
- Trakt: device-code sign-in; scrobbling; 1 write a second and 500 reads per 5 minutes
  ([rate limits](https://github.com/trakt/trakt-api/blob/master/projects/developer/src/lib/guides/rate-limiting.md)).
- Watching together: Syncplay and Jellyfin's SyncPlay relay state through a server; clients estimate
  clock offset and round trip, change speed for small drift and seek for large drift
  ([Syncplay](https://github.com/Syncplay/syncplay/blob/master/syncplay/constants.py),
  [Jellyfin](https://github.com/jellyfin/jellyfin-web/blob/master/src/plugins/syncPlay/core/PlaybackCore.js)).

## Test clips

Read 2026-11-11, for the corpus ([ADR-0012](adr/0012-the-test-corpus.md)).

- dovi_tool 2.3.4 generates Dolby Vision metadata for profiles 5, 8.1 and 8.4 only; x265 4.2 signals
  5, 8.1, 8.2 and 8.4. No open encoder makes profile 7's enhancement layer
  ([dovi_tool](https://github.com/quietvoid/dovi_tool)).
- FFmpeg 9.0.2 encodes E-AC3, and TrueHD and DTS core as experimental encoders; nothing open
  encodes DTS-HD MA, DTS:X or Atmos. Two young projects claim a DTS-HD MA encoder, unverified:
  `callumcomplains/subwoofa` (GPL-3.0) and `B00kerLouis/Swift_DTSHDMAX` (research-only licence).
- FFmpeg's test suite ([fate-suite.ffmpeg.org](https://fate-suite.ffmpeg.org/)) holds Dolby Vision
  profile 7 samples (`mpegts/dovi-p7.ts`, `mkv/dovi-p7-hvce.mkv`), DTS-HD MA 7.1
  (`dts/master_audio_7.1_24bit.dts`), DTS:X (`dts/dtsx_7.1.4_24bit.dts`) and TrueHD Atmos
  (`truehd/atmos.thd`), from 10 KB to 6.3 MB, under no stated terms. No E-AC3 with Atmos.
- Netflix Open Content is under CC BY 4.0
  ([opencontent.netflix.com](https://opencontent.netflix.com/)). *Cosmos Laundromat*'s
  `CosmosLaundromat_2k24p_HDR_P3PQ.mp4` (730 MB) is 8-bit H.264, 2048 × 858, with no colour tags;
  read with ffprobe. A 2018 licence file of *Meridian* says CC BY-NC-ND 4.0, against the page.
- The Blender films are under CC BY 3.0 or 4.0, all SDR
  ([Big Buck Bunny](https://peach.blender.org/about/), [Tears of Steel](https://mango.blender.org/about/)).
- Jellyfin's test videos are under CC BY-SA 4.0: SDR, HDR10, Dolby Vision 5, 8.1 and 8.4, no
  profile 7, no HLG ([repo.jellyfin.org](https://repo.jellyfin.org/test-videos/)).
- Kodi's wiki lists vendor demonstrations with profile 7, DTS-HD MA, DTS:X and Atmos, under "fair
  use is claimed", on third-party hosts.

## The rivals' engines

Read 2026-11-11 in their sources, at the versions `flake.lock` pins: mpv 0.42.0
([mpv](https://github.com/mpv-player/mpv)), Kodi 21.3 ([xbmc](https://github.com/xbmc/xbmc)),
stremio-linux-shell 1.2.2 ([Stremio](https://github.com/Stremio/stremio-linux-shell)), Jellyfin
Desktop 2.0.0-unstable-2026-08-31 ([jellyfin-desktop](https://github.com/jellyfin/jellyfin-desktop))
([ADR-0013](adr/0013-the-bench.md)).

- mpv publishes `frame-drop-count`, `decoder-frame-drop-count`, `vo-delayed-frame-count`,
  `mistimed-frame-count` and `avsync`, and lets `input-ipc-server` change at run time.
- Kodi's JSON-RPC listens on 127.0.0.1:9090 in a fresh profile. Its dropped and skipped frames and
  its audio-video difference reach only its overlay; its debug log has `dropped in output` lines
  and its audio engine's `average error`. `Player.OnAVStart` marks the start of playback.
- Stremio's shell and Jellyfin Desktop embed libmpv through the render API in OpenGL. Neither reads
  an `mpv.conf` nor opens mpv's socket. Stremio's shell lets a page send any mpv command, but
  observe only a fixed list of properties. Jellyfin Desktop's `userCommand` runs an mpv command
  line; it chooses the sound track itself when a file loads.
- Stremio's shell never sets `hwdec`: its mpv decodes in software. Jellyfin Desktop's mpv uses
  `vaapi-copy`, and downmixes to 2.0 by default; so does Kodi (2.0 by default).
- Stremio's streaming server (`server.js`) is not free software.

## Building blocks

Read 2026-11-10; candidates for the stages that need them, each decided then.

- Torrents: librqbit 9.1.0, Apache-2.0, streams with Range requests, one main maintainer, uploads
  unless limited ([rqbit](https://github.com/ikatson/rqbit)).
- JavaScript: rquickjs 0.14, MIT, memory and time limits, no system API unless given
  ([rquickjs](https://github.com/DelSkayn/rquickjs)); boa, pure Rust.
- Sync: a last-writer-wins log in SQLite is enough for lists, positions and settings; Loro and
  Automerge for richer merges; iroh for devices without a server.
- Storage: rusqlite 0.41; the keyring crate split into a core and a store per system.
- Licences: mpv is GPL-2.0-or-later, LGPL with `-Dgpl=false`
  ([Copyright](https://github.com/mpv-player/mpv/blob/master/Copyright)); FFmpeg is LGPL unless
  built with `--enable-gpl` ([legal](https://ffmpeg.org/legal.html)).
