// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The bench of Stage 1 ([ADR-0013]): it plays a clip of the test corpus ([ADR-0012]) in one of
//! the rivals, or in Aerith itself through its test window, and measures what the quality bar asks: the time to the first frame, the frames
//! dropped, delayed or mistimed, the audio-video difference, memory, CPU and the GPU's power.
//!
//! ```text
//! aerith-bench run <rival> <clip> [--stream] [--seconds N] [--idle N] [--out DIR]
//!                  [--display TEXT] [--mpv-option NAME=VALUE]...
//!                  [--order below|above] [--toggle-controls N] [--frames T,T...]
//! aerith-bench table <result.json>...
//! aerith-bench presentation <Wayland log> [frames a second]
//! ```
//!
//! `CONTRIBUTING.md` says how to run it, on which display and through which wrapper. It runs on
//! Linux only for now; Windows joins at milestone M1.6.
//!
//! [ADR-0012]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0012-the-test-corpus.md
//! [ADR-0013]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0013-the-bench.md

#[cfg(target_os = "linux")]
mod audio;
#[cfg(target_os = "linux")]
mod colour;
#[cfg(target_os = "linux")]
mod engine;
#[cfg(target_os = "linux")]
mod figures;
#[cfg(target_os = "linux")]
mod json_stream;
#[cfg(target_os = "linux")]
mod presentation;
#[cfg(target_os = "linux")]
mod process;
#[cfg(target_os = "linux")]
mod rivals;
#[cfg(target_os = "linux")]
mod run;
#[cfg(target_os = "linux")]
mod sampler;
#[cfg(target_os = "linux")]
mod sandbox;
#[cfg(target_os = "linux")]
mod serve;

use std::process::ExitCode;

const USAGE: &str = "usage:
  aerith-bench run <mpv|kodi|stremio|jellyfin|aerith> <clip> [--stream] [--seconds N]
                   [--idle N] [--out DIR] [--display TEXT] [--mpv-option NAME=VALUE]...
                   [--order below|above] [--toggle-controls N] [--frames T,T...]
  aerith-bench table <result.json>...
  aerith-bench presentation <Wayland log> [frames a second]";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("run") => run_command(&arguments[1..]),
        Some("table") => table_command(&arguments[1..]),
        Some("presentation") => presentation_command(&arguments[1..]),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(target_os = "linux")]
fn run_command(arguments: &[String]) -> ExitCode {
    let options = match parse_run(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run::run(&options) {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("aerith-bench: {error}");
            let sound = error
                .get_ref()
                .is_some_and(|inner| inner.is::<run::SoundDeviceOpened>());
            // scripts/bench-rivals.sh stops the whole set on this code.
            if sound {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

/// mpv options that would undo what keeps a run safe and comparable: its output, its sound, its
/// IPC and its configuration are the bench's.
#[cfg(target_os = "linux")]
const RESERVED_MPV_OPTIONS: [&str; 9] = [
    "vo",
    "gpu-context",
    "gpu-api",
    "ao",
    "audio-device",
    "input-ipc-server",
    "config",
    "config-dir",
    "log-file",
];

#[cfg(not(target_os = "linux"))]
fn run_command(_arguments: &[String]) -> ExitCode {
    eprintln!("aerith-bench runs on Linux only for now; Windows joins at milestone M1.6");
    ExitCode::FAILURE
}

#[cfg(not(target_os = "linux"))]
fn presentation_command(arguments: &[String]) -> ExitCode {
    run_command(arguments)
}

#[cfg(not(target_os = "linux"))]
fn table_command(arguments: &[String]) -> ExitCode {
    run_command(arguments)
}

/// Reads `run`'s arguments.
#[cfg(target_os = "linux")]
fn parse_run(arguments: &[String]) -> Result<run::RunOptions, String> {
    let mut positional = Vec::new();
    let mut seconds = 1800.0;
    let mut out = std::path::PathBuf::from("target/bench");
    let mut display = "unknown".to_owned();
    let mut mpv_options = Vec::new();
    let mut stream = false;
    let mut idle = std::time::Duration::from_secs(10);
    let mut order = None;
    let mut toggle_controls = None;
    let mut frames = Vec::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let mut value = || {
            rest.next()
                .cloned()
                .ok_or(format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--seconds" => {
                seconds = value()?
                    .parse::<f64>()
                    .ok()
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .ok_or("--seconds needs a positive number")?;
            }
            "--out" => out = value()?.into(),
            "--stream" => stream = true,
            "--idle" => {
                idle = value()?
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| *seconds <= 3600)
                    .map(std::time::Duration::from_secs)
                    .ok_or("--idle needs a whole number of seconds, at most 3600")?;
            }
            "--display" => display = value()?,
            "--order" => {
                let value = value()?;
                if value != "below" && value != "above" {
                    return Err(format!("--order takes below or above, not {value}"));
                }
                order = Some(value);
            }
            "--frames" => {
                frames = value()?
                    .split(',')
                    .map(|time| {
                        time.parse::<f64>()
                            .ok()
                            .filter(|t| t.is_finite() && *t >= 0.0)
                    })
                    .collect::<Option<Vec<f64>>>()
                    .ok_or("--frames takes times in seconds, separated by commas")?;
            }
            "--toggle-controls" => {
                toggle_controls = Some(
                    value()?
                        .parse::<u32>()
                        .ok()
                        .filter(|times| (1..=10_000).contains(times))
                        .ok_or("--toggle-controls needs a whole number from 1 to 10000")?,
                );
            }
            "--mpv-option" => {
                let option = value()?;
                let Some((name, _)) = option.split_once('=').filter(|_| !option.starts_with('-'))
                else {
                    return Err(format!("--mpv-option takes NAME=VALUE, not {option}"));
                };
                if RESERVED_MPV_OPTIONS.contains(&name) {
                    return Err(format!("--mpv-option cannot set {name}: the bench sets it"));
                }
                mpv_options.push(option);
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ => positional.push(argument.clone()),
        }
    }
    let [rival, clip] = positional.as_slice() else {
        return Err("run needs a rival and a clip".to_owned());
    };
    let rival = rivals::Rival::from_name(rival).ok_or(format!("unknown rival {rival}"))?;
    let mpv = matches!(rival, rivals::Rival::Mpv | rivals::Rival::Aerith);
    if !mpv_options.is_empty() && !mpv {
        return Err(
            "--mpv-option applies to mpv alone and Aerith's mpv; the others keep their own options"
                .to_owned(),
        );
    }
    if !frames.is_empty() && !mpv {
        return Err("--frames applies to mpv alone and Aerith".to_owned());
    }
    if (order.is_some() || toggle_controls.is_some()) && rival != rivals::Rival::Aerith {
        return Err("--order and --toggle-controls apply to Aerith's testbed only".to_owned());
    }
    Ok(run::RunOptions {
        rival,
        clip: clip.into(),
        seconds,
        out,
        display,
        mpv_options,
        order,
        toggle_controls,
        frames,
        stream,
        idle,
    })
}

/// Prints the presentation figures of a Wayland log that a run kept (`player.err` for mpv alone,
/// `mpv-wayland.log` for Aerith), all of it, the first two seconds left out, against the frames'
/// rate on screen when it is given.
#[cfg(target_os = "linux")]
fn presentation_command(arguments: &[String]) -> ExitCode {
    let (path, frame_rate) = match arguments {
        [path] => (path, None),
        [path, rate] => match rate.parse::<f64>() {
            Ok(rate) if rate > 0.0 => (path, Some(rate)),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        },
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match std::fs::read(path) {
        Ok(bytes) => {
            let frames = presentation::parse(&String::from_utf8_lossy(&bytes));
            println!("{}", presentation::figures(&frames, 2.0, frame_rate));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{path}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the Markdown table that compares the runs whose results are given.
#[cfg(target_os = "linux")]
fn table_command(paths: &[String]) -> ExitCode {
    if paths.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    println!("{}", figures::TABLE_HEADER);
    let mut failed = false;
    for path in paths {
        let result = std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()));
        match result {
            Ok(result) => println!("{}", figures::table_row(&result)),
            Err(error) => {
                eprintln!("{path}: {error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use rivals::Rival;

    fn arguments(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn run_reads_its_options() {
        let options = parse_run(&arguments(
            "mpv clip.mkv --stream --seconds 60 --idle 0 --out /tmp/b --display DP-3 --mpv-option video-sync=display-resample",
        ))
        .unwrap();
        assert_eq!(options.rival, Rival::Mpv);
        assert_eq!(options.clip, std::path::PathBuf::from("clip.mkv"));
        assert_eq!(options.seconds, 60.0);
        assert_eq!(options.display, "DP-3");
        assert_eq!(options.mpv_options, ["video-sync=display-resample"]);
        assert!(options.stream);
        assert!(options.idle.is_zero());
        let aerith = parse_run(&arguments(
            "aerith clip.mkv --order above --toggle-controls 60 --mpv-option video-sync=display-resample --frames 5,20.5",
        ))
        .unwrap();
        assert_eq!(aerith.rival, Rival::Aerith);
        assert_eq!(aerith.order.as_deref(), Some("above"));
        assert_eq!(aerith.toggle_controls, Some(60));
        assert_eq!(aerith.frames, [5.0, 20.5]);
        assert_eq!(aerith.mpv_options, ["video-sync=display-resample"]);
    }

    #[test]
    fn run_refuses_what_it_cannot_do() {
        for wrong in [
            "",
            "mpv",
            "vlc clip.mkv",
            "mpv clip.mkv --seconds -1",
            "mpv clip.mkv --seconds",
            "mpv clip.mkv --idle 1.5",
            "mpv clip.mkv --idle 18446744073709551615",
            "mpv clip.mkv --mpv-option gpu-context=drm",
            "mpv clip.mkv --mpv-option ao=alsa",
            "mpv clip.mkv --frobnicate",
            "mpv clip.mkv --mpv-option novalue",
            "mpv clip.mkv --mpv-option --vo=x",
            "kodi clip.mkv --mpv-option hwdec=no",
            "mpv clip.mkv --order above",
            "aerith clip.mkv --order sideways",
            "aerith clip.mkv --toggle-controls 0",
            "aerith clip.mkv --frames 5,x",
            "aerith clip.mkv --frames -1",
            "kodi clip.mkv --frames 5",
            "aerith clip.mkv --mpv-option input-ipc-server=/tmp/x",
            "mpv a.mkv b.mkv",
        ] {
            assert!(
                parse_run(&arguments(wrong)).is_err(),
                "{wrong:?} should be refused"
            );
        }
    }
}
