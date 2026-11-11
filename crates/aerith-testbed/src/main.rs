// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The test interface of Stage 1 ([ADR-0017]): one window, on winit and wgpu as uwui's host makes
//! them, that plays a file or stream through `aerith-player`, mpv's picture in the player's plane
//! ([ADR-0015]), under test controls and a translucent panel. It ships to no one, and goes when
//! Stage 3's interface on uwui plays a film.
//!
//! ```text
//! aerith-testbed <file or URL> [--order below|above] [--fullscreen] [--letterbox]
//!                [--seconds N] [--pause-after N] [--toggle-controls N] [--stats]
//!                [--mpv PROGRAM] [--sound]
//! ```
//!
//! `CONTRIBUTING.md` says how to run it, in `agent-headless` and on a desktop.
//!
//! # What it does
//!
//! The player starts with the window, mpv's window ready; the source is opened one second later,
//! so that the time to the first frame, printed on stderr, leaves out mpv's own start. The testbed
//! quits `--seconds` after the first frame (10 by default), when the source ends, or on `q`.
//!
//! - **The order of the surfaces** (`src/args.rs`), which milestone M1.4b compares:
//!   - `below`, the default: the plane lies below the window's own surface, which wgpu draws,
//!     transparent but for the controls;
//!   - `above`: the window's surface holds one black pixel, scaled to the window, and the plane
//!     lies on it, below a surface of the testbed's own that wgpu draws. While the controls are
//!     hidden, that surface holds nothing, so that the compositor sees one picture: the video's.
//!     `src/surfaces.rs` says how.
//! - **The plane** fills the window, mpv drawing its own black bars; with `--letterbox` it covers
//!   only the video's rectangle, and the window shows around it.
//! - **The controls**: a play/pause button and a seek bar on a translucent panel. They show on a
//!   key or a move of the pointer, and hide 3 s after the last (`src/visibility.rs`). Space plays
//!   or pauses, the arrows seek 10 s back or forth, `f` toggles fullscreen, `h` shows or hides the
//!   controls, `q` and Escape quit. A click on the button or the bar does what they show. The
//!   player reports no length yet, so the bar spans `--seconds`.
//! - **At rest, nothing wakes**: the window's loop waits for events, with a deadline only while
//!   the controls show or a frame callback is awaited. The player's events reach it from a thread
//!   blocked on the player's channel; the run's own deadlines (the open, `--pause-after`,
//!   `--toggle-controls`, the end) and the statistics come from a thread apart
//!   (`src/schedule.rs`). A frame is drawn only when what the controls show changes, paced by the
//!   compositor's frame callbacks on the surface drawn (`src/app.rs`). winit's own pacing is never
//!   used: a surface that the video covers gets no frame callbacks (wlroots issue 4133).
//! - `--pause-after N` pauses N seconds after the first frame. `--toggle-controls N` shows and
//!   hides the controls N times from 1 s after the first frame, each time shown 1 s and hidden
//!   1 s, for M1.4b's measures. `--mpv PROGRAM` names mpv (by default the one of `PATH`), so that
//!   a wrapper can add options; `--sound` plays the sound aloud, which is discarded otherwise.
//!
//! mpv's shader cache, and the GPU driver's of both processes, go to a folder of the system's
//! temporary folder, removed at the end, so that each run starts with empty caches; `--cache
//! FOLDER` keeps them in a folder that outlives the run, as Aerith's own cache does.
//!
//! # The statistics
//!
//! With `--stats`, one JSON object a line goes to stdout, every second after the open, and one
//! more at the end with `"summary": true` and the time to the first frame. Every count is a total
//! since the start: subtract two lines for a rate. Human messages go to stderr. The fields:
//!
//! - `since_open`: seconds since the source was opened;
//! - `presentation`: the plane's `Presentation`, as `aerith-player` counts it:
//!   `committed`, `presented`, `discarded`, `zero_copy`, `gaps` (32 counts) and `longer_gaps`;
//! - `main_thread` and `relay_thread`: the context switches of the testbed's main thread, which
//!   runs the window, and of the player's relay thread, `aerith-relay`, from
//!   `/proc/self/task/<tid>/status`: `voluntary` ones are the thread's sleeps, so each is a
//!   wake-up; `involuntary` ones are preemptions. `null` when the thread cannot be read;
//! - `clock`: the player's last clock, `position` in seconds read at that moment, `running` and
//!   `rate`; `null` before the first;
//! - `ui_frames`: the frames the testbed drew; `ui_frames_late`: those whose frame callback had
//!   not come 100 ms later. The next frame waits for it, however late;
//! - in the summary only: `first_frame_ms`, from the open to the player's `Started`, or `null`.
//!
//! ```text
//! {"since_open":1.000,"presentation":{"committed":24,"presented":24,"discarded":0,"zero_copy":0,"gaps":[0,0,...],"longer_gaps":0},"main_thread":{"voluntary":310,"involuntary":4},"relay_thread":{"voluntary":95,"involuntary":0},"clock":{"position":0.412,"running":true,"rate":1.000000},"ui_frames":1,"ui_frames_late":0}
//! ```
//!
//! [ADR-0015]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0015-a-relay-for-mpvs-wayland-surface.md
//! [ADR-0017]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0017-a-test-interface-on-winit-and-wgpu.md

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod args;
#[cfg(target_os = "linux")]
mod draw;
#[cfg(target_os = "linux")]
mod layout;
#[cfg(target_os = "linux")]
mod schedule;
#[cfg(target_os = "linux")]
mod stats;
#[cfg(target_os = "linux")]
mod surfaces;
#[cfg(target_os = "linux")]
mod visibility;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let options = match args::parse(std::env::args().skip(1)) {
        Ok(args::Parsed::Run(options)) => options,
        Ok(args::Parsed::Help) => {
            println!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("aerith-testbed: {error}\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    let kept = options.cache.clone();
    let cache = kept.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("aerith-testbed-{}", std::process::id()))
    });
    // The GPU driver's shader cache of this process goes with mpv's, not to the person's home.
    // SAFETY: no other thread exists yet, so nothing reads the environment while it changes.
    unsafe {
        std::env::set_var("MESA_SHADER_CACHE_DIR", cache.join("driver"));
        std::env::set_var("__GL_SHADER_DISK_CACHE_PATH", cache.join("driver"));
    }
    let outcome = app::run(options, &cache);
    if kept.is_none() {
        let _ = std::fs::remove_dir_all(&cache);
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aerith-testbed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("aerith-testbed shows the player's plane, which exists on Linux only for now");
    ExitCode::FAILURE
}
