// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The command line.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "usage: aerith-testbed <file or URL> [--order below|above] [--fullscreen] \
[--letterbox]
                      [--seconds N] [--pause-after N] [--toggle-controls N] [--stats]
                      [--mpv PROGRAM] [--sound]";

/// Where the video plane lies among the window's surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Below the window's own surface, which wgpu draws, transparent over the video.
    Below,
    /// Above the window's own surface, which holds an opaque black pixel, and below a surface of
    /// the testbed's own, which wgpu draws.
    Above,
}

/// What the command line asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// A file's path, or an `http://` or `https://` URL.
    pub source: String,
    pub order: Order,
    pub fullscreen: bool,
    /// The plane covers the video's rectangle only, rather than the whole window.
    pub letterbox: bool,
    /// How long to play after the first frame, then quit.
    pub seconds: Duration,
    /// When to pause, after the first frame.
    pub pause_after: Option<Duration>,
    /// How many times to show and hide the controls.
    pub toggle_controls: u32,
    pub stats: bool,
    /// mpv's program.
    pub mpv: PathBuf,
    /// Play the sound aloud.
    pub sound: bool,
}

/// What to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    Help,
}

/// A command line that asks nothing the testbed can do, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgumentError(String);

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Reads the arguments that follow the program's name.
pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Parsed, ArgumentError> {
    let error = |text: String| Err(ArgumentError(text));
    let mut arguments = arguments.into_iter();
    let mut source = None;
    let mut options = Options {
        source: String::new(),
        order: Order::Below,
        fullscreen: false,
        letterbox: false,
        seconds: Duration::from_secs(10),
        pause_after: None,
        toggle_controls: 0,
        stats: false,
        mpv: PathBuf::from("mpv"),
        sound: false,
    };
    while let Some(argument) = arguments.next() {
        let mut value = |flag: &str| {
            arguments
                .next()
                .ok_or_else(|| ArgumentError(format!("{flag} needs a value")))
        };
        match argument.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--order" => {
                options.order = match value("--order")?.as_str() {
                    "below" => Order::Below,
                    "above" => Order::Above,
                    other => return error(format!("--order is below or above, not {other:?}")),
                }
            }
            "--fullscreen" => options.fullscreen = true,
            "--letterbox" => options.letterbox = true,
            "--seconds" => {
                options.seconds = seconds("--seconds", &value("--seconds")?)?;
                if options.seconds.is_zero() {
                    return error("--seconds is more than 0".to_owned());
                }
            }
            "--pause-after" => {
                options.pause_after = Some(seconds("--pause-after", &value("--pause-after")?)?);
            }
            "--toggle-controls" => {
                let count = value("--toggle-controls")?;
                options.toggle_controls = count.parse().map_err(|_| {
                    ArgumentError(format!("--toggle-controls takes a count, not {count:?}"))
                })?;
            }
            "--stats" => options.stats = true,
            "--mpv" => options.mpv = PathBuf::from(value("--mpv")?),
            "--sound" => options.sound = true,
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return error(format!("unknown option {flag:?}"));
            }
            _ if source.is_none() => source = Some(argument),
            _ => return error(format!("one source only; {argument:?} is a second")),
        }
    }
    let Some(source) = source else {
        return error("no file or URL to play".to_owned());
    };
    options.source = source;
    Ok(Parsed::Run(options))
}

/// A number of seconds, whole or not, never negative.
fn seconds(flag: &str, value: &str) -> Result<Duration, ArgumentError> {
    value
        .parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .ok_or_else(|| ArgumentError(format!("{flag} takes seconds, not {value:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &[&str]) -> Result<Parsed, ArgumentError> {
        parse(line.iter().map(|argument| (*argument).to_owned()))
    }

    fn options(line: &[&str]) -> Options {
        match parsed(line) {
            Ok(Parsed::Run(options)) => options,
            other => panic!("{line:?} gave {other:?}"),
        }
    }

    fn refused(line: &[&str]) -> String {
        match parsed(line) {
            Err(error) => error.to_string(),
            other => panic!("{line:?} was accepted: {other:?}"),
        }
    }

    #[test]
    fn a_source_alone_takes_every_default() {
        assert_eq!(
            options(&["clip.mkv"]),
            Options {
                source: "clip.mkv".to_owned(),
                order: Order::Below,
                fullscreen: false,
                letterbox: false,
                seconds: Duration::from_secs(10),
                pause_after: None,
                toggle_controls: 0,
                stats: false,
                mpv: PathBuf::from("mpv"),
                sound: false,
            }
        );
    }

    #[test]
    fn every_option_is_read_in_any_order() {
        let options = options(&[
            "--order",
            "above",
            "--stats",
            "https://example.org/clip.mkv",
            "--seconds",
            "5",
            "--pause-after",
            "2.5",
            "--toggle-controls",
            "60",
            "--fullscreen",
            "--letterbox",
            "--mpv",
            "/tmp/mpv-lavapipe",
            "--sound",
        ]);
        assert_eq!(options.source, "https://example.org/clip.mkv");
        assert_eq!(options.order, Order::Above);
        assert_eq!(options.seconds, Duration::from_secs(5));
        assert_eq!(options.pause_after, Some(Duration::from_millis(2500)));
        assert_eq!(options.toggle_controls, 60);
        assert_eq!(options.mpv, PathBuf::from("/tmp/mpv-lavapipe"));
        assert!(options.stats && options.fullscreen && options.letterbox && options.sound);
    }

    #[test]
    fn help_is_asked_for_anywhere() {
        assert_eq!(parsed(&["clip.mkv", "--help"]), Ok(Parsed::Help));
        assert_eq!(parsed(&["-h"]), Ok(Parsed::Help));
    }

    #[test]
    fn bad_command_lines_are_refused_with_a_reason() {
        assert_eq!(refused(&[]), "no file or URL to play");
        assert_eq!(
            refused(&["a.mkv", "b.mkv"]),
            "one source only; \"b.mkv\" is a second"
        );
        assert_eq!(
            refused(&["a.mkv", "--colour"]),
            "unknown option \"--colour\""
        );
        assert_eq!(refused(&["a.mkv", "-x"]), "unknown option \"-x\"");
        assert_eq!(refused(&["a.mkv", "--order"]), "--order needs a value");
        assert_eq!(
            refused(&["a.mkv", "--order", "beside"]),
            "--order is below or above, not \"beside\""
        );
        assert_eq!(
            refused(&["a.mkv", "--toggle-controls", "-1"]),
            "--toggle-controls takes a count, not \"-1\""
        );
        assert_eq!(
            refused(&["a.mkv", "--seconds", "0"]),
            "--seconds is more than 0"
        );
        for bad in ["-3", "NaN", "inf", "ten", "1e30"] {
            assert_eq!(
                refused(&["a.mkv", "--seconds", bad]),
                format!("--seconds takes seconds, not {bad:?}")
            );
        }
        assert_eq!(
            refused(&["a.mkv", "--pause-after", "-1"]),
            "--pause-after takes seconds, not \"-1\""
        );
        assert_eq!(refused(&["a.mkv", "--mpv"]), "--mpv needs a value");
    }

    #[test]
    fn a_lone_dash_is_a_source() {
        assert_eq!(options(&["-"]).source, "-");
    }
}
