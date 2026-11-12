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
- `lychee`, which checks links;
- `bwrap` (bubblewrap), the bench's sandbox, which its tests and the player's run. It needs
  unprivileged user namespaces; CI's Ubuntu runner turns them on first;
- mpv, which the player's tests drive, named by `AERITH_TEST_MPV`: the same build as the bench's
  mpv, without its wrapper;
- `pkg-config` and libwayland, with which the player's relay links `libwayland-client`;
- on `LD_LIBRARY_PATH`, the libraries the test window, `aerith-testbed`, loads as it runs:
  libwayland-client, libxkbcommon and the Vulkan loader. winit loads libwayland-client itself, and
  in a build of the whole workspace the relay then does too.

The first entry downloads the toolchain, about 150 MB, and mpv with what it needs: 752 MiB once
unpacked, shared in part with the rest.

The bench has a shell of its own, `nix develop .#bench`: the default shell's tools, plus what makes
the test corpus and the rivals the bench measures ([below](#the-bench)). CI does not load it.

The player's fuzz target has one too, `nix develop .#fuzz`: a nightly Rust pinned by date
(2026-11-10), which cargo-fuzz needs and which builds nothing that users run
([ADR-0011](docs/adr/0011-a-pinned-stable-toolchain.md)), and cargo-fuzz
([below](#the-relays-fuzz-target)). CI does not load it.

Without Nix, rustup reads `rust-toolchain.toml` and installs the same toolchain; `cargo deny`,
`lychee`, `bwrap` and mpv are then yours to install, and `AERITH_TEST_MPV` yours to set. CI's
Windows job works this way; there the player has no engine yet (M1.6): its tests against mpv build
empty, and only its tests of sources and clocks run.

## Building and testing

The workspace holds `aerith-player`, Aerith's player, `aerith-testbed`, the test window
([below](#the-test-window)), and `aerith-bench`, the bench ([below](#the-bench)). The commands that
build and test the workspace:

```sh
cargo build --workspace
cargo test --workspace
```

### The player's tests

`cargo test -p aerith-player` runs four kinds of test
([ADR-0014](docs/adr/0014-the-player-interface-and-mpvs-connection.md)):

- in `src/`, against a fake mpv: every event, error and limit, on Linux. The relay's
  (`src/relay/tests.rs`) run between a fake compositor, `tests/common/host.rs`, which the
  interface's own connection reaches, and a fake mpv on the relay's socket: the plane's surface,
  mpv's frames reaching it, place, hide and show, close, and each check and ceiling ending mpv's
  connection while the interface's still answers. The fake compositor raises the protocol errors
  wlroots 0.20 raises on what the relay sends, at each request and at each commit, so that a
  request of the relay's that the compositor would refuse fails the test;
- in `tests/mpv.rs` and `tests/at_rest.rs`, against the real mpv that `AERITH_TEST_MPV` names, on
  clips mpv makes itself. Each mpv runs in bubblewrap with a home of the test's own; the sound
  devices and the session's runtime folders (its compositor, sound server and bus) are hidden; the
  sound is discarded. `tests/mpv.rs` discards the picture. `at_rest` shows it in a plane of the
  fake compositor's window, and checks that a paused or stopped player wakes none of Aerith's
  threads, the relay's included. There mpv draws with lavapipe, Mesa's Vulkan driver on the CPU,
  when the machine has it (below); elsewhere, as in CI, mpv's video output fails and the relay
  serves no frame;
- in `tests/plane.rs`, the real mpv drawing into the plane with lavapipe: its frames reach the
  fake compositor; mpv draws no frame while the plane is hidden, as the relay counts what mpv
  commits; mpv quits once the plane is closed; and a crash of mpv leaves the window's connection
  whole. The development shell does not hold Mesa, whose closure is about 1.1 GB: these tests are
  ignored unless `AERITH_TEST_WAYLAND=1` is set when they are built.
  `AERITH_TEST_LAVAPIPE` names lavapipe's JSON file; NixOS's own,
  `/run/opengl-driver/share/vulkan/icd.d/lvp_icd.x86_64.json`, is used without it:

```sh
AERITH_TEST_WAYLAND=1 cargo test -p aerith-player --test plane
```

- in `tests/corpus.rs`, on the whole corpus ([below](#the-corpus)), ignored unless asked for:

```sh
AERITH_CORPUS=~/.cache/aerith-bench/corpus cargo test --release -p aerith-player --test corpus -- \
    --ignored --test-threads=1 --nocapture
```

It plays every clip, then the bench's clip as a stream, and prints the time to each first frame
and what mpv reported. Then it times the first frame of the bench's clip, from a file and as a
stream, through the player and through mpv alone, with mpv's defaults (as the bench runs it) and
with the player's options; after one run of each to warm the caches, the three alternate, seven
runs each. The options are written again in the test: keep them in step with `src/mpv.rs`. Run it at each milestone that touches playback, on an idle machine; its figures go to
[`docs/measurements.md`](docs/measurements.md).

### Playing a file

```sh
cargo run --release -p aerith-player --example play -- <file or URL> [--sound] \
    [--seconds N] [--header "Name: value"]...
```

plays a file or a stream through the player and prints each event, with the time since it was
opened, a second after the player started. The picture is discarded; the test window
([below](#the-test-window)) shows it. `--sound` plays the sound aloud. It runs the `mpv` of `PATH`,
or the program `--mpv` names, with your own home: mpv writes nothing there itself. Play test clips
only.

### The test window

`aerith-testbed` ([ADR-0017](docs/adr/0017-a-test-interface-on-winit-and-wgpu.md)) opens a window
on Wayland, on winit and wgpu, and plays a file or stream through the player, mpv's picture in the
player's plane, under test controls and a translucent panel. Its crate documentation, in
`crates/aerith-testbed/src/main.rs`, says what each option does and gives the format of
`--stats`.

```sh
aerith-testbed <file or URL> [--order below|above] [--fullscreen] [--letterbox] [--seconds N]
    [--pause-after N] [--toggle-controls N] [--stats] [--mpv PROGRAM] [--cache FOLDER] [--sound]
```

- `--order below`, the default, puts the video under the window's own surface, transparent;
  `--order above` puts it on a black pixel, under a surface of the testbed's own.
- It opens the source a second after the player starts, prints the time to the first frame on
  stderr, and quits `--seconds` after it (10 by default).
- `--stats` prints, each second, the plane's frames and the wake-ups of the window's thread and of
  the relay's, as JSON lines on stdout.
- Keys: space plays or pauses, the arrows seek 10 s, `f` toggles fullscreen, `h` the controls,
  `q` or Escape quits. The controls show on a key or a move of the pointer, for 3 s.
- The sound is discarded unless `--sound` is given. mpv's caches and the GPU driver's go to a
  folder under the system's temporary folder, removed at the end; `--cache FOLDER` keeps them
  from one run to the next, as Aerith's own cache does.

**In `agent-headless`.** The services that run agents have no display: `agent-headless --sway`
gives the testbed a headless sway of its own. That sway offers no `linux-dmabuf`, so both mpv and
wgpu draw with lavapipe, Mesa's Vulkan driver on the CPU: `VK_DRIVER_FILES` shows wgpu lavapipe
alone, and a wrapper gives mpv the options the player's tests give it. Inside `nix develop`:

```sh
cargo build --release -p aerith-testbed
lavapipe=/run/opengl-driver/share/vulkan/icd.d/lvp_icd.x86_64.json
mkdir -p /tmp/testbed
# A clip of 5 s, made by mpv's own encoder, as the player's tests make theirs.
HOME=/tmp/testbed "$AERITH_TEST_MPV" --no-config --of=matroska --ovc=ffv1 \
    --o=/tmp/testbed/clip.mkv "av://lavfi:testsrc2=size=1280x720:rate=24:duration=5"
# mpv on lavapipe, with the options the player's tests add after the player's own.
cat > /tmp/testbed/mpv <<EOF
#!/bin/sh
export VK_DRIVER_FILES=$lavapipe
exec "\$AERITH_TEST_MPV" "\$@" --gpu-sw=yes --gpu-context=waylandvk
EOF
chmod +x /tmp/testbed/mpv
# The headless output, made smaller so that its software compositor keeps up.
agent-headless --sway -- sh -c "swaymsg output HEADLESS-1 resolution 1280x720 && \
    VK_DRIVER_FILES=$lavapipe target/release/aerith-testbed /tmp/testbed/clip.mkv \
    --seconds 5 --stats --toggle-controls 2 --mpv /tmp/testbed/mpv"
```

`--order above` plays the other order; `--pause-after 2 --seconds 8` shows the window's thread and
the relay's asleep while paused. The headless output is 3840 × 2160, drawn by pixman: it shows a
few frames a second unless made smaller, as above. Figures there are not tier figures.

**On a desktop**, inside `nix develop`, with the pinned mpv and a clip of the corpus:

```sh
cargo run --release -p aerith-testbed -- ~/.cache/aerith-bench/corpus/<clip> \
    --mpv "$AERITH_TEST_MPV" --fullscreen --stats
```

An agent runs it on the owner's display only through `owner-desktop --`, with the owner's go.
The window shows as long as `--seconds` says; `q` ends it sooner. The "above" order needs a
compositor that offers fractional scaling and viewports, as the headless sway does; the testbed
says so and stops on one that does not.

### The relay's fuzz target

`crates/aerith-player/fuzz/` holds the fuzz target of the relay's side facing mpv, for cargo-fuzz
and libFuzzer, in a workspace of its own. Each input is a session: the fake compositor of the
player's tests and an interface on it, the relay on the interface's window, and a fake mpv that
makes what mpv makes at its start, then sends the input as Wayland messages to any object, with
any arguments and file descriptors, between the interface's own changes of the plane. A panic in
any thread, a session that never ends, or an error of the fake compositor's on the interface's
connection is a crash. From `crates/aerith-player`:

```sh
nix develop ../..#fuzz
cargo fuzz run relay -- -max_total_time=600 -timeout=10
```

It writes what it finds under `fuzz/corpus` and `fuzz/artifacts`, which git ignores. A crash is
replayed with `cargo fuzz run relay <file>`. Neither `scripts/check.sh` nor CI runs it yet.

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

## The bench

The bench measures the rivals, and later Aerith, against the quality bar
([`docs/ROADMAP.md`](docs/ROADMAP.md#budgets)). It plays the test corpus
([ADR-0012](docs/adr/0012-the-test-corpus.md)) in mpv alone, Kodi, Stremio or Jellyfin Desktop,
and measures the time to the first frame, the frames dropped, delayed or mistimed, the sync, the
memory, the CPU and the GPU's power ([ADR-0013](docs/adr/0013-the-bench.md)). Every command below
runs inside `nix develop .#bench`.

It plays no sound: the rivals play into the bench's own silent PipeWire and cannot see the sound
devices. It plays nothing of yours: only the corpus.

### The corpus

```sh
scripts/corpus.sh ~/.cache/aerith-bench/corpus
```

makes the corpus ([ADR-0012](docs/adr/0012-the-test-corpus.md#consequences) gives its time and
size). It fetches about 740 MB first,
each file checked against its pinned SHA-256: FFmpeg's test samples and three minutes of *Cosmos
Laundromat*. Run again, it keeps the clips already made and checks them all. `--quick` makes a few
seconds of each clip in a few minutes, to try the bench, not to measure; a folder holds one kind
or the other, never both. It checks each clip's codec, colour, rate, size and length. On a slow
machine, copy a corpus made elsewhere instead.

### One run

```sh
cargo run --release -p aerith-bench -- run mpv ~/.cache/aerith-bench/corpus/hdr10-2160p-hevc10-eac3.mkv \
    --seconds 60 --display "DP-3 3840x2160@240"
```

plays the clip for 60 seconds at most, fullscreen, and prints the path of its `result.json`, under
`target/bench/`, beside the player's logs. `--stream` serves the clip over HTTP on 127.0.0.1 and
plays it as a stream. `--idle N` reads the GPU's idle power for N seconds first (10 by default, 0
to skip). `--mpv-option name=value` adds an option to mpv alone. `--display` is what you say of the
screen: the bench cannot read it. `aerith-bench table <result.json>...` prints the table that
compares runs.

### The set of measures

```sh
scripts/bench-rivals.sh <desktop|short|laptop> ~/.cache/aerith-bench/corpus "<display>" [rival...]
```

runs every measure on every rival, or on those named, and writes `table.md` beside the runs:

- `desktop`: the full set on the high tier, about 2.5 hours;
- `short`: the same, with shorter runs, about 50 minutes;
- `laptop`: the time to the first frame, memory, power, and cadence at 23.976, about one hour.

Each rival takes the screen, fullscreen, for each of its runs. Nothing else may run meanwhile: other
programs change the CPU, the cadence and the GPU's power. Each result says how many hardware
threads they kept busy (`others_cores`, the table's "Other programs"); take again a run where it
is not near zero. If a process of a run ever opens a sound device, the run exits with code 3 and the
script stops at once. Each rival starts with a fresh profile and empty shader caches.
`BENCH_SECONDS=2` shortens every run, to try the script.

The bench needs a desktop session. In an agent's service, `agent-headless --` runs it without a
GPU, which checks that it works but gives no figure: mpv alone needs `--mpv-option gpu-sw=yes`
there, or `BENCH_MPV_OPTIONS=gpu-sw=yes` for the script. On the owner's desktop, an agent runs it through `owner-desktop --`, with the owner's go.

### Aerith inside its window

`aerith` is a player of the bench too: `aerith-bench run aerith <clip>` runs `aerith-testbed` from
`PATH`, fullscreen, which opens the clip itself and plays it through `aerith-player`. The bench
gives the testbed a wrapper as its mpv: the same mpv as mpv alone, with the bench's IPC socket
added, through which the bench reads mpv's counters without driving it. `--order below|above`
chooses the order of the surfaces, `--toggle-controls N` shows and hides the controls N times,
and `--mpv-option` adds an option to that mpv. For mpv alone and Aerith, the bench reads the
compositor's presentation feedback from mpv's Wayland messages (`WAYLAND_DEBUG=client`): the
frames presented, those scanned out (`zero_copy`), and the refresh gaps between them, a frame
off cadence being one whose gap is neither of the two whole numbers around the run's mean.
`--frames 5,20,40` captures the frames at those times instead of measuring the playback, as mpv
draws them in its window.

```sh
scripts/bench-window.sh <corpus> "<display>" [cadence|toggles|sync|frames]...
```

runs M1.4b's measures at the screen's current rate: every SDR cadence clip in mpv alone and in
Aerith in each order, 60 toggles of the controls, display sync, and the frames compared with mpv
alone's (PSNR, `inf` for the same picture). `BENCH_SECONDS` sets each run's length (120 by
default; about 80 minutes for every part); the script's header gives each part's time. It
builds the testbed and puts it on `PATH` itself.

### On the lowest laptop

The owner runs this on the HP Laptop 17-ca2xxx, plugged in, nothing else open:

1. Clone the repository and copy the corpus made on the desktop to `~/.cache/aerith-bench/corpus`
   (making it there would take many hours).
2. Close every other program. Set the screen's brightness as usual; it does not enter the figures.
3. `nix develop .#bench --command scripts/bench-rivals.sh laptop ~/.cache/aerith-bench/corpus "<screen>"`,
   with `<screen>` the laptop's screen as its display settings name it: output, size and rate.
4. Leave the machine alone for about an hour. A rival that fails is reported, and the set goes on.
5. Bring back the folder the script prints at the end (`target/bench/laptop-<date>/`), whole: its
   `table.md` and every run's `result.json` and logs.

The laptop's GPU power comes from the APU's sensor, when it has one; without it, the CPU time
stands for power.

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
