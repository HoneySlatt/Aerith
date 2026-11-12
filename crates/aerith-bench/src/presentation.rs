// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What mpv's frames did on screen, read from mpv's own Wayland connection, the same way for mpv
//! alone and for mpv inside Aerith's window.
//!
//! With `WAYLAND_DEBUG=client`, libwayland prints every message of a connection on standard error,
//! in order. Each frame of mpv's is a commit of its surface with a buffer attached, which the
//! Vulkan driver makes. mpv asks the compositor for presentation feedback when its frame callback
//! comes, for the next commit; the compositor answers with `presented` (when, at which refresh,
//! and how: scanned out or composited) or `discarded`. Through Aerith's relay, these answers are
//! the real compositor's, passed back.
//!
//! **Not every frame has feedback.** When the driver commits two frames between two of mpv's frame
//! callbacks, the second has none. The figures therefore say how many frames had it, and measure a
//! gap only between two frames committed one after the other that both had it. Aerith's relay asks
//! for feedback on every frame, so its own count (`aerith-testbed --stats`) has every gap.
//!
//! The figures:
//!
//! - **refresh gaps**: how many refreshes a frame stayed on screen before the next, from the
//!   refresh counter (`msc`), or from the time between them over the refresh period when the
//!   compositor keeps no counter (KWin 6.8). The ideal gap is the display's rate over the
//!   frames' rate, `r`: a film at 23.976 frames a second on a 240.016 Hz display has r = 10.0107, so each frame shows
//!   10 refreshes and one in 93 shows 11. The gaps **off cadence** are those the ideal pattern
//!   does not hold, counted on their histogram: of n gaps, the pattern has (1 − f) n of
//!   ⌊r⌋ and f n of ⌈r⌉, f being r's fraction; every gap past those, and every gap of another
//!   length, is off cadence. A frame shown 9 refreshes and the next 11, at r = 10.0107, are two
//!   gaps off cadence. This holds at any fixed refresh rate under audio sync. Under display sync,
//!   mpv presents the same picture at every refresh: every gap is one refresh and says nothing of
//!   the picture's cadence, which mpv's own counters of delayed and mistimed frames tell instead.
//! - **whole intervals**: the share of intervals between two frames presented one after the
//!   other that lie within a twentieth of a refresh of a whole number of refreshes. 100 % at a
//!   fixed rate; under VRR most are not, but display sync holds a VRR display at its top rate and
//!   brings it back to 100 %.
//! - **intervals**: the time between two frames presented one after the other.
//! - **timing**: for variable refresh rates, where refreshes say nothing of the content's cadence,
//!   the time between two frames presented, `k` frames apart, against `k` frame periods. Frames
//!   without feedback lie between, so most spans are of two frames under fifo. A span off by more
//!   than half a refresh of the display's fastest rate showed a frame too early or too late.
//! - **zero copy**: the frames the compositor scanned out straight from mpv's buffer.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use crate::figures;

/// `wp_presentation_feedback.kind.zero_copy`.
const ZERO_COPY: u32 = 0x8;

/// How the compositor presented a frame.
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

/// What became of one frame mpv committed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fate {
    /// No feedback was asked for it, or none came before the log ends.
    #[default]
    Unknown,
    Presented(Presented),
    Discarded,
}

/// mpv's frames, in the order it committed them, as the log tells them.
pub fn parse(log: &str) -> Vec<Fate> {
    let mut surface: Option<String> = None;
    let mut frames: Vec<Fate> = Vec::new();
    // Feedback asked since the surface's last commit, and the frame each one asked about since.
    let mut pending: Vec<String> = Vec::new();
    let mut asked: HashMap<String, Option<usize>> = HashMap::new();
    let mut attached = false;
    for line in log.lines() {
        if let Some((_, request)) = line.split_once(" -> ") {
            if let Some(rest) = request.strip_prefix("wp_presentation") {
                // `wp_presentation#17.feedback(wl_surface#8, new id wp_presentation_feedback#73)`
                let Some((target, id)) = rest
                    .split_once(".feedback(")
                    .and_then(|(_, arguments)| arguments.strip_suffix(')'))
                    .and_then(|arguments| arguments.split_once(", new id "))
                else {
                    continue;
                };
                if surface.is_none() {
                    surface = Some(target.to_owned());
                }
                if surface.as_deref() == Some(target) {
                    pending.push(id.to_owned());
                }
            } else if let Some(target) = &surface {
                if request.starts_with(&format!("{target}.attach(")) {
                    attached = !request.contains("(nil");
                } else if request == format!("{target}.commit()") {
                    // The feedback asked goes with this commit, a frame only if it brought a buffer.
                    let frame = attached.then(|| {
                        frames.push(Fate::Unknown);
                        frames.len() - 1
                    });
                    for id in pending.drain(..) {
                        asked.insert(id, frame);
                    }
                    attached = false;
                }
            }
            continue;
        }
        // `{Default Queue} wp_presentation_feedback#73.presented(0, 44231, 767818000, ...)`
        let Some(start) = line.find("wp_presentation_feedback#") else {
            continue;
        };
        let message = &line[start..];
        let Some((id, event)) = message.split_once('.') else {
            continue;
        };
        // `sync_output` comes first and says nothing of the frame's fate.
        if !(event == "discarded()" || event.starts_with("presented(")) {
            continue;
        }
        let Some(Some(frame)) = asked.remove(id) else {
            continue;
        };
        let fate = if event == "discarded()" {
            Some(Fate::Discarded)
        } else {
            event
                .strip_prefix("presented(")
                .and_then(|arguments| arguments.strip_suffix(')'))
                .and_then(presented)
                .map(Fate::Presented)
        };
        if let (Some(fate), Some(slot)) = (fate, frames.get_mut(frame)) {
            *slot = fate;
        }
    }
    frames
}

/// The refreshes from `before` to `after`: by the refresh counters when the compositor keeps one,
/// else by the time between them over the refresh period, rounded, as KWin 6.8 sends no counter.
/// `None` when neither says, or when the counter or the time went back (an output changed).
fn refreshes_between(before: &Presented, after: &Presented) -> Option<u64> {
    if before.msc != 0 && after.msc != 0 {
        return after.msc.checked_sub(before.msc).filter(|gap| *gap > 0);
    }
    if after.refresh_ns == 0 || after.at_ns <= before.at_ns {
        return None;
    }
    let refresh = u128::from(after.refresh_ns);
    let gap = (after.at_ns - before.at_ns + refresh / 2) / refresh;
    u64::try_from(gap).ok().filter(|gap| *gap > 0)
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

/// The figures of the frames from `settle` seconds after the first one presented: the first
/// seconds after a start settle the clock, as for mpv's own figures. `frame_rate` is the frames'
/// rate on screen, the clip's rate times mpv's speed correction, for the ideal gap; without it,
/// the mean gap stands for the ideal.
pub fn figures(frames: &[Fate], settle: f64, frame_rate: Option<f64>) -> Value {
    let presented_at = |fate: &Fate| match fate {
        Fate::Presented(presented) => Some(presented.at_ns),
        _ => None,
    };
    let Some(first) = frames.iter().find_map(presented_at) else {
        return json!({"frames": frames.len(), "presented": 0});
    };
    let settled = first + (settle * 1e9) as u128;
    let Some(start) = frames
        .iter()
        .position(|fate| presented_at(fate).is_some_and(|at| at >= settled))
    else {
        return json!({"frames": frames.len(), "presented": 0});
    };
    let frames = &frames[start..];
    let shown: Vec<Presented> = frames
        .iter()
        .filter_map(|fate| match fate {
            Fate::Presented(presented) => Some(*presented),
            _ => None,
        })
        .collect();
    let discarded = frames
        .iter()
        .filter(|fate| **fate == Fate::Discarded)
        .count();
    // Pairs of frames committed one after the other, both presented.
    let pairs: Vec<(Presented, Presented)> = frames
        .windows(2)
        .filter_map(|pair| match pair {
            [Fate::Presented(before), Fate::Presented(after)] => Some((*before, *after)),
            _ => None,
        })
        .collect();
    let mut gaps = BTreeMap::new();
    for (before, after) in &pairs {
        if let Some(gap) = refreshes_between(before, after) {
            *gaps.entry(gap).or_insert(0_u64) += 1;
        }
    }
    let intervals: Vec<f64> = pairs
        .iter()
        .map(|(before, after)| after.at_ns.saturating_sub(before.at_ns) as f64 / 1e6)
        .collect();
    let refreshes: Vec<f64> = shown
        .iter()
        .filter(|frame| frame.refresh_ns > 0)
        .map(|frame| f64::from(frame.refresh_ns) / 1e6)
        .collect();
    let refresh_ms = figures::quantile(refreshes, 0.5);
    let ideal = refresh_ms
        .zip(frame_rate)
        .filter(|(refresh, rate)| *refresh > 0.0 && *rate > 0.0)
        .map(|(refresh, rate)| 1000.0 / refresh / rate);
    let span = match (shown.first(), shown.last()) {
        // Times that go back, in a spliced or malformed log, give a span of zero.
        (Some(first), Some(last)) => last.at_ns.saturating_sub(first.at_ns) as f64 / 1e9,
        _ => 0.0,
    };
    // The frames committed from the first shown to the last, for the rate.
    let last_shown = frames.iter().rposition(|fate| presented_at(fate).is_some());
    let committed_over_span = last_shown.unwrap_or(0);
    json!({
        "frames": frames.len(),
        "presented": shown.len(),
        "discarded": discarded,
        "without_feedback": frames.len() - shown.len() - discarded,
        "seconds": figures::round(span, 1),
        "frames_per_second": (span > 0.0).then(|| figures::round(committed_over_span as f64 / span, 3)),
        "zero_copy": shown.iter().filter(|frame| frame.flags & ZERO_COPY != 0).count(),
        "refresh_ms": refresh_ms.map(|ms| figures::round(ms, 4)),
        "refresh_gaps": gap_figures(&gaps, ideal),
        "whole_intervals": refresh_ms.and_then(|refresh| whole_share(&intervals, refresh)),
        "intervals_ms": interval_figures(intervals),
        "timing": frame_rate.zip(refresh_ms).map_or(Value::Null, |(rate, refresh)| {
            timing_figures(frames, 1000.0 / rate, refresh)
        }),
    })
}

/// The figures of refresh gaps counted by length: their counts, their mean, the ideal gap
/// (`ideal`, or the mean without it) and the gaps off cadence (the module's documentation says
/// how they are counted). `null` without any gap.
pub fn gap_figures(counts: &BTreeMap<u64, u64>, ideal: Option<f64>) -> Value {
    let total: u64 = counts.values().sum();
    if total == 0 {
        return Value::Null;
    }
    let mean = counts.iter().map(|(gap, count)| gap * count).sum::<u64>() as f64 / total as f64;
    let ideal = ideal.unwrap_or(mean);
    let (low, high) = (ideal.floor() as u64, ideal.ceil() as u64);
    let fraction = ideal - ideal.floor();
    let count_of = |gap: u64| counts.get(&gap).copied().unwrap_or(0) as f64;
    let n = total as f64;
    let on_cadence = if low == high {
        count_of(low)
    } else {
        count_of(low).min((1.0 - fraction) * n) + count_of(high).min(fraction * n)
    };
    json!({
        "gaps": total,
        "mean": figures::round(mean, 4),
        "ideal": figures::round(ideal, 4),
        "counts": counts.iter().map(|(gap, count)| (gap.to_string(), json!(count))).collect::<serde_json::Map<_, _>>(),
        "off_cadence": (n - on_cadence).round() as u64,
    })
}

/// How far the time between presented frames lies from their frame periods (`period_ms`), against
/// half a refresh and a whole one (`refresh_ms`): see the module's documentation.
fn timing_figures(frames: &[Fate], period_ms: f64, refresh_ms: f64) -> Value {
    let presented: Vec<(usize, u128)> = frames
        .iter()
        .enumerate()
        .filter_map(|(index, fate)| match fate {
            Fate::Presented(presented) => Some((index, presented.at_ns)),
            _ => None,
        })
        .collect();
    let errors: Vec<f64> = presented
        .windows(2)
        .map(|pair| {
            let ((first, at), (second, later)) = (pair[0], pair[1]);
            let span = later.saturating_sub(at) as f64 / 1e6;
            (span - (second - first) as f64 * period_ms).abs()
        })
        .collect();
    if errors.is_empty() {
        return Value::Null;
    }
    let beyond = |limit: f64| errors.iter().filter(|error| **error > limit).count();
    json!({
        "spans": errors.len(),
        "off_by_half_a_refresh": beyond(refresh_ms / 2.0),
        "off_by_a_refresh": beyond(refresh_ms),
        "worst_ms": figures::round(errors.iter().copied().fold(0.0, f64::max), 1),
    })
}

/// The share, rounded to a hundredth, of `intervals` within a twentieth of a refresh of a whole
/// number of refreshes of `refresh_ms`; `None` without intervals.
fn whole_share(intervals: &[f64], refresh_ms: f64) -> Option<f64> {
    if intervals.is_empty() || refresh_ms <= 0.0 {
        return None;
    }
    let whole = intervals
        .iter()
        .filter(|interval| {
            let refreshes = *interval / refresh_ms;
            (refreshes - refreshes.round()).abs() < 0.05
        })
        .count();
    Some(figures::round(whole as f64 / intervals.len() as f64, 2))
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

    /// The lines of one frame as mpv and Mesa print them with libwayland 1.24: the feedback mpv
    /// asked, as `feedback`, then the driver's attach and commit.
    fn commit(feedback: Option<u32>) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(id) = feedback {
            lines.push(format!(
                "[01:20:09.409060] {{Default Queue}}  -> wp_presentation#17.feedback(wl_surface#8, new id wp_presentation_feedback#{id})"
            ));
        }
        lines.push(
            "[01:20:09.450636] {mesa vk display queue}  -> wl_surface#8.attach(wl_buffer#83, 0, 0)"
                .to_owned(),
        );
        lines.push("[01:20:09.450644] {mesa vk display queue}  -> wl_surface#8.damage_buffer(0, 0, 2147483647, 2147483647)".to_owned());
        lines
            .push("[01:20:09.450651] {mesa vk display queue}  -> wl_surface#8.commit()".to_owned());
        lines
    }

    /// The compositor's answer to feedback `id`: presented at `ms` milliseconds, refresh `msc`.
    fn presented_line(id: u32, ms: u64, msc: u64, flags: u32) -> String {
        let (seconds, nanoseconds) = (ms / 1000, (ms % 1000) * 1_000_000);
        format!(
            "[01:20:09.451127] {{Default Queue}} wp_presentation_feedback#{id}.presented(0, {seconds}, {nanoseconds}, 4166388, 0, {msc}, {flags})"
        )
    }

    /// A log of frames each with feedback, presented at the refreshes `mscs`, 42 ms apart.
    fn log_of(mscs: &[u64], flags: u32) -> String {
        let mut lines = Vec::new();
        for (index, msc) in mscs.iter().enumerate() {
            let id = 70 + (index as u32 % 2);
            lines.extend(commit(Some(id)));
            lines.push(presented_line(id, 1000 + 42 * index as u64, *msc, flags));
        }
        lines.join("\n")
    }

    #[test]
    fn each_frame_gets_the_answer_to_the_feedback_asked_before_its_commit() {
        let mut lines = commit(Some(73));
        lines.extend(commit(None));
        lines.push("Playing: clip.mkv".to_owned());
        lines.push("[01:20:09.451118] {Default Queue} wp_presentation_feedback#73.sync_output(wl_output#30)".to_owned());
        lines.push(presented_line(73, 1000, 240, 0x7));
        lines.extend(commit(Some(73)));
        lines.push(
            "[01:20:09.451] {Default Queue} wp_presentation_feedback#73.discarded()".to_owned(),
        );
        lines.push("[01:20:09.451] {Default Queue} discarded wl_seat#5.name(\"seat0\")".to_owned());
        let frames = parse(&lines.join("\n"));
        assert_eq!(
            frames,
            [
                Fate::Presented(Presented {
                    at_ns: 1_000_000_000,
                    refresh_ns: 4_166_388,
                    msc: 240,
                    flags: 0x7
                }),
                Fate::Unknown,
                Fate::Discarded,
            ]
        );
    }

    #[test]
    fn a_commit_without_a_buffer_is_no_frame_and_takes_the_feedback_asked() {
        let mut lines = vec![
            "[0] {Default Queue}  -> wp_presentation#17.feedback(wl_surface#8, new id wp_presentation_feedback#73)".to_owned(),
            "[0] {Default Queue}  -> wl_surface#8.commit()".to_owned(),
            presented_line(73, 1000, 240, 0),
        ];
        lines.extend(commit(None));
        assert_eq!(parse(&lines.join("\n")), [Fate::Unknown]);
    }

    #[test]
    fn gaps_off_the_ideal_pattern_are_off_cadence() {
        // 10, 10, 11, 10, 9, 10, 12.
        let mut mscs = vec![1000];
        for gap in [10, 10, 11, 10, 9, 10, 12] {
            mscs.push(mscs.last().unwrap() + gap);
        }
        let figures = figures(&parse(&log_of(&mscs, 0)), 0.0, None);
        let gaps = &figures["refresh_gaps"];
        // The mean, 10.29, stands for the ideal: of 7 gaps, 5 of 10 and 2 of 11.
        assert_eq!(gaps["off_cadence"], 2);
        assert_eq!(gaps["gaps"], 7);
        assert_eq!(gaps["counts"], json!({"9": 1, "10": 4, "11": 1, "12": 1}));
        assert_eq!(figures["presented"], 8);
        // At 240.016 Hz for 23.976 frames a second: 10 is the ideal, 11 once in 93. The 9 and its
        // 11, and the 12, are off cadence.
        let at_film_rate = super::figures(&parse(&log_of(&mscs, 0)), 0.0, Some(240.016 / 10.0107));
        assert_eq!(at_film_rate["refresh_gaps"]["off_cadence"], 3);
    }

    #[test]
    fn a_9_and_an_11_around_a_10_are_two_gaps_off_cadence() {
        let mut counts = BTreeMap::new();
        counts.insert(10, 91);
        counts.insert(9, 4);
        counts.insert(11, 5);
        // The ideal has one 11 in 100: 99 tens and one eleven.
        let figures = gap_figures(&counts, Some(10.01));
        assert_eq!(figures["off_cadence"], 8);
        assert_eq!(gap_figures(&BTreeMap::new(), None), Value::Null);
    }

    #[test]
    fn a_frame_without_feedback_breaks_the_gap_rather_than_doubling_it() {
        let mut lines = commit(Some(70));
        lines.push(presented_line(70, 1000, 1000, 0));
        lines.extend(commit(None));
        lines.extend(commit(Some(71)));
        lines.push(presented_line(71, 1083, 1020, 0));
        lines.extend(commit(Some(70)));
        lines.push(presented_line(70, 1125, 1030, 0));
        let figures = figures(&parse(&lines.join("\n")), 0.0, None);
        assert_eq!(figures["refresh_gaps"]["counts"], json!({"10": 1}));
        assert_eq!(figures["without_feedback"], 1);
        assert_eq!(figures["frames"], 4);
    }

    #[test]
    fn a_whole_mean_leaves_only_that_gap_on_cadence() {
        let mscs: Vec<u64> = (0..5).map(|n| 1000 + n * 10).collect();
        let figures = figures(&parse(&log_of(&mscs, 0)), 0.0, None);
        assert_eq!(figures["refresh_gaps"]["off_cadence"], 0);
        assert_eq!(figures["refresh_gaps"]["mean"], 10.0);
    }

    #[test]
    fn the_first_seconds_settle_and_zero_copy_is_counted() {
        // Frames 42 ms apart from 1 s: those before 2 s after the first are left out.
        let mscs: Vec<u64> = (0..60).map(|n| 1000 + n * 10).collect();
        let figures = figures(&parse(&log_of(&mscs, ZERO_COPY)), 2.0, None);
        assert_eq!(figures["presented"], 12);
        assert_eq!(figures["zero_copy"], 12);
        assert_eq!(figures["intervals_ms"]["p50"], 42.0);
    }

    #[test]
    fn without_a_refresh_counter_gaps_come_from_time() {
        // 42 ms apart at a refresh of 4.166388 ms: 10 refreshes.
        let figures = figures(&parse(&log_of(&[0, 0, 0], 0)), 0.0, None);
        assert_eq!(figures["refresh_gaps"]["counts"], json!({"10": 2}));
        assert_eq!(figures["presented"], 3);
    }

    #[test]
    fn spans_off_their_frame_periods_are_counted() {
        // Frames 41.708 ms apart at 23.976 fps, one of them 6 ms late; the second has no
        // feedback, so the first span is of two frames.
        let mut lines = commit(Some(70));
        lines.push(presented_line(70, 1000, 0, 0));
        lines.extend(commit(None));
        for (index, ms) in [1083, 1131, 1167].into_iter().enumerate() {
            let id = 71 + index as u32;
            lines.extend(commit(Some(id)));
            lines.push(presented_line(id, ms, 0, 0));
        }
        let timing = &figures(&parse(&lines.join("\n")), 0.0, Some(24000.0 / 1001.0))["timing"];
        assert_eq!(timing["spans"], 3);
        // 1083 − 1000 = 83 of 83.4 ms; 1131 − 1083 = 48 of 41.7, 6.3 late; 1167 − 1131 = 36, 5.7 early.
        assert_eq!(timing["off_by_half_a_refresh"], 2);
        assert_eq!(timing["off_by_a_refresh"], 2);
        assert_eq!(timing["worst_ms"], 6.3);
    }

    #[test]
    fn times_that_go_back_give_no_negative_span_and_no_panic() {
        let log = [
            commit(Some(70)).join("\n"),
            presented_line(70, 10_000, 0, 0),
            commit(Some(71)).join("\n"),
            presented_line(71, 13_000, 0, 0),
            commit(Some(72)).join("\n"),
            presented_line(72, 11_000, 0, 0),
        ]
        .join("\n");
        let figures = figures(&parse(&log), 0.0, None);
        assert_eq!(figures["presented"], 3);
        assert_eq!(figures["seconds"], 1.0);
    }

    #[test]
    fn whole_intervals_tell_a_fixed_rate_from_a_variable_one() {
        // At 4.166388 ms a refresh: 42 ms is 10.08 refreshes, whole; 39.6 ms is 9.5, not.
        assert_eq!(
            whole_share(&[41.664, 41.664, 39.6, 37.5], 4.166388),
            Some(0.75)
        );
        assert_eq!(whole_share(&[], 4.166388), None);
    }

    #[test]
    fn an_empty_log_has_no_frames() {
        assert_eq!(figures(&parse(""), 2.0, None)["presented"], 0);
    }
}
