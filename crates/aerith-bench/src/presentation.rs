// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What mpv's frames did on screen, read from mpv's own Wayland connection, the same way for mpv
//! alone and for mpv inside Aerith's window.
//!
//! With `WAYLAND_DEBUG=client`, libwayland prints every message of a connection on standard error.
//! mpv asks the compositor for presentation feedback on each frame it commits; the compositor
//! answers each with `presented` (when, at which refresh, and how: scanned out or composited) or
//! `discarded`. Through Aerith's relay, these answers are the real compositor's, passed back.
//!
//! The figures:
//!
//! - **refresh gaps**: how many refreshes each frame stayed on screen, from the refresh counter
//!   (`msc`). A film at 23.976 frames a second on a 240.016 Hz display shows each frame 10
//!   refreshes, and one 11 about every 3.9 s. A frame is **off cadence** when its gap is neither
//!   of the two whole numbers around the run's mean gap: shown too long or too short for the
//!   content's rate. This holds at any fixed refresh rate, with or without display sync.
//! - **intervals**: the time between two frames presented, for variable refresh rates, where the
//!   refresh counter says nothing of the content's cadence.
//! - **zero copy**: the frames the compositor scanned out straight from mpv's buffer.
//!
//! Several feedback objects may ask about one commit (mpv's own, and the Vulkan driver's for its
//! present timing): their answers carry the same time, so the frames are counted once by time.

use serde_json::{Value, json};

use crate::figures;

/// `wp_presentation_feedback.kind.zero_copy`.
const ZERO_COPY: u32 = 0x8;

/// One frame the compositor presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presented {
    /// When it showed, in nanoseconds on the compositor's presentation clock.
    pub at_ns: u128,
    /// The display's refresh period then, in nanoseconds; 0 when the compositor does not say.
    pub refresh_ns: u32,
    /// The refresh counter; 0 when the compositor has none.
    pub msc: u64,
    /// The feedback's kind flags.
    pub flags: u32,
}

/// What a log holds, in order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Feedback {
    pub presented: Vec<Presented>,
    pub discarded: u64,
}

/// Reads the presentation feedback in a libwayland debug log. Lines of any other kind, and lines
/// the log mixes in from the program's own output, are skipped; requests (`->`) are not answers.
pub fn parse(log: &str) -> Feedback {
    let mut feedback = Feedback::default();
    for line in log.lines() {
        if line.contains(" -> ") {
            continue;
        }
        let Some((_, message)) = line.split_once("wp_presentation_feedback") else {
            continue;
        };
        if message.ends_with(".discarded()") {
            feedback.discarded += 1;
        } else if let Some(arguments) = message
            .split_once(".presented(")
            .and_then(|(_, rest)| rest.strip_suffix(')'))
            && let Some(presented) = presented(arguments)
        {
            // Another feedback object's answer for the same commit.
            if feedback.presented.last().map(|last| last.at_ns) != Some(presented.at_ns) {
                feedback.presented.push(presented);
            }
        }
    }
    feedback
}

/// The arguments of `presented`: `tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, flags`.
fn presented(arguments: &str) -> Option<Presented> {
    let numbers: Vec<u64> = arguments
        .split(',')
        .map(|number| number.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [sec_hi, sec_lo, nsec, refresh, seq_hi, seq_lo, flags] = numbers.as_slice() else {
        return None;
    };
    let seconds = (u128::from(*sec_hi) << 32) | u128::from(*sec_lo);
    Some(Presented {
        at_ns: seconds * 1_000_000_000 + u128::from(*nsec),
        refresh_ns: u32::try_from(*refresh).ok()?,
        msc: (*seq_hi << 32) | *seq_lo,
        flags: u32::try_from(*flags).ok()?,
    })
}

/// The figures of the frames presented from `settle` seconds after the first one: the first
/// seconds after a start settle the clock, as for mpv's own figures.
pub fn figures(feedback: &Feedback, settle: f64) -> Value {
    let Some(first) = feedback.presented.first() else {
        return json!({"presented": 0, "discarded": feedback.discarded});
    };
    let settle_ns = (settle * 1e9) as u128;
    let frames: Vec<Presented> = feedback
        .presented
        .iter()
        .copied()
        .filter(|frame| frame.at_ns >= first.at_ns + settle_ns)
        .collect();
    let (Some(start), Some(end)) = (frames.first(), frames.last()) else {
        return json!({"presented": 0, "discarded": feedback.discarded});
    };
    let span = (end.at_ns - start.at_ns) as f64 / 1e9;
    let intervals: Vec<f64> = frames
        .windows(2)
        .map(|pair| (pair[1].at_ns - pair[0].at_ns) as f64 / 1e6)
        .collect();
    let refreshes: Vec<f64> = frames
        .iter()
        .filter(|frame| frame.refresh_ns > 0)
        .map(|frame| f64::from(frame.refresh_ns) / 1e6)
        .collect();
    let zero_copy = frames
        .iter()
        .filter(|frame| frame.flags & ZERO_COPY != 0)
        .count();
    json!({
        "presented": frames.len(),
        "discarded": feedback.discarded,
        "seconds": figures::round(span, 1),
        "per_second": (span > 0.0).then(|| figures::round((frames.len() - 1) as f64 / span, 3)),
        "zero_copy": zero_copy,
        "refresh_ms": figures::quantile(refreshes, 0.5).map(|ms| figures::round(ms, 4)),
        "refresh_gaps": refresh_gaps(&frames),
        "intervals_ms": interval_figures(intervals),
    })
}

/// The refresh gaps between frames presented one after the other: their counts by length, their
/// mean, and the frames off cadence. `null` when the compositor gives no refresh counter. A
/// counter that goes back (an output changed) starts the gaps again.
fn refresh_gaps(frames: &[Presented]) -> Value {
    if frames.iter().all(|frame| frame.msc == 0) {
        return Value::Null;
    }
    let gaps: Vec<u64> = frames
        .windows(2)
        .filter(|pair| pair[1].msc > pair[0].msc)
        .map(|pair| pair[1].msc - pair[0].msc)
        .collect();
    if gaps.is_empty() {
        return Value::Null;
    }
    let mean = gaps.iter().sum::<u64>() as f64 / gaps.len() as f64;
    let (low, high) = (mean.floor() as u64, mean.ceil() as u64);
    let off_cadence = gaps
        .iter()
        .filter(|&&gap| gap != low && gap != high)
        .count();
    let mut counts = std::collections::BTreeMap::new();
    for gap in &gaps {
        *counts.entry(*gap).or_insert(0_u64) += 1;
    }
    json!({
        "mean": figures::round(mean, 4),
        "counts": counts.iter().map(|(gap, count)| (gap.to_string(), json!(count))).collect::<serde_json::Map<_, _>>(),
        "off_cadence": off_cadence,
    })
}

fn interval_figures(intervals: Vec<f64>) -> Value {
    if intervals.is_empty() {
        return Value::Null;
    }
    let at =
        |fraction| figures::quantile(intervals.clone(), fraction).map(|ms| figures::round(ms, 3));
    json!({"min": at(0.0), "p01": at(0.01), "p50": at(0.5), "p99": at(0.99), "max": at(1.0)})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line as libwayland 1.24 prints it, for a frame presented at `ms` milliseconds.
    fn line(id: u32, ms: u64, msc: u64, flags: u32) -> String {
        let (seconds, nanoseconds) = (ms / 1000, (ms % 1000) * 1_000_000);
        format!(
            "[00:29:28.309227] {{Default Queue}} wp_presentation_feedback#{id}.presented(0, {seconds}, {nanoseconds}, 4166389, 0, {msc}, {flags})"
        )
    }

    #[test]
    fn presented_and_discarded_frames_are_read_and_requests_skipped() {
        let log = [
            "[00:29:28.117932] {Default Queue}  -> wp_presentation#17.feedback(wl_surface#6, new id wp_presentation_feedback#62)".to_owned(),
            line(62, 1000, 240, 0x7),
            "Playing: clip.mkv".to_owned(),
            "[00:29:28.403552] {Default Queue} wp_presentation_feedback#66.discarded()".to_owned(),
            "[00:29:28.403552] {Default Queue} discarded wl_seat#5.name(\"seat0\")".to_owned(),
            line(64, 1042, 250, 0xf),
            "[00:29:28.403552] {Default Queue} wp_presentation_feedback#70.presented(0, 1".to_owned(),
        ]
        .join("\n");
        let feedback = parse(&log);
        assert_eq!(feedback.discarded, 1);
        assert_eq!(
            feedback.presented,
            [
                Presented {
                    at_ns: 1_000_000_000,
                    refresh_ns: 4_166_389,
                    msc: 240,
                    flags: 0x7
                },
                Presented {
                    at_ns: 1_042_000_000,
                    refresh_ns: 4_166_389,
                    msc: 250,
                    flags: 0xf
                },
            ]
        );
    }

    #[test]
    fn two_answers_for_one_commit_count_one_frame() {
        let log = [
            line(62, 1000, 240, 0),
            line(63, 1000, 240, 0),
            line(64, 1042, 250, 0),
        ]
        .join("\n");
        assert_eq!(parse(&log).presented.len(), 2);
    }

    #[test]
    fn gaps_off_the_two_around_the_mean_are_off_cadence() {
        // 10, 10, 11, 10, 9, 10, 12: a mean of 10.29, so 10 and 11 are on cadence; 9 and 12 are not.
        let mut msc = 1000;
        let mut lines = vec![line(1, 0, msc, 0)];
        for (index, gap) in [10, 10, 11, 10, 9, 10, 12].into_iter().enumerate() {
            msc += gap;
            lines.push(line(2, 100 + 42 * index as u64, msc, 0));
        }
        let figures = figures(&parse(&lines.join("\n")), 0.0);
        let gaps = &figures["refresh_gaps"];
        assert_eq!(gaps["off_cadence"], 2);
        assert_eq!(gaps["counts"], json!({"9": 1, "10": 4, "11": 1, "12": 1}));
        assert_eq!(figures["presented"], 8);
    }

    #[test]
    fn a_whole_mean_leaves_only_that_gap_on_cadence() {
        let lines: Vec<String> = (0..5).map(|n| line(1, n * 42, 1000 + n * 10, 0)).collect();
        let figures = figures(&parse(&lines.join("\n")), 0.0);
        assert_eq!(figures["refresh_gaps"]["off_cadence"], 0);
        assert_eq!(figures["refresh_gaps"]["mean"], 10.0);
    }

    #[test]
    fn the_first_seconds_settle_and_zero_copy_is_counted() {
        let lines = [
            line(1, 0, 1, 0),
            line(1, 1000, 241, 0),
            line(1, 2000, 481, ZERO_COPY),
            line(1, 2050, 493, ZERO_COPY | 0x1),
            line(1, 2100, 505, 0),
        ];
        let figures = figures(&parse(&lines.join("\n")), 2.0);
        assert_eq!(figures["presented"], 3);
        assert_eq!(figures["zero_copy"], 2);
        assert_eq!(figures["intervals_ms"]["p50"], 50.0);
        assert_eq!(figures["per_second"], 20.0);
    }

    #[test]
    fn without_a_refresh_counter_there_are_no_gaps() {
        let lines: Vec<String> = (0..3).map(|n| line(1, n * 42, 0, 0)).collect();
        let figures = figures(&parse(&lines.join("\n")), 0.0);
        assert_eq!(figures["refresh_gaps"], Value::Null);
        assert_eq!(figures["presented"], 3);
    }

    #[test]
    fn an_empty_log_has_no_frames() {
        assert_eq!(figures(&parse(""), 2.0)["presented"], 0);
    }
}
