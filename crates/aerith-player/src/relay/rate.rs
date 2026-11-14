// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How the relay tells a display at a fixed refresh rate from one under VRR, from the times the
//! compositor presented mpv's frames ([ADR-0021], [ADR-0022]).
//!
//! No Wayland protocol says whether VRR is on. The presentation feedback shows it while mpv plays
//! under audio sync, which presents each frame when it is due: at a fixed rate, the time between
//! two frames presented is always a whole number of refreshes, as the display shows a frame only
//! at a refresh; under VRR, the display shows it when it comes, and that time is rarely whole.
//! Under display sync, mpv presents at every refresh and holds a VRR display at its top rate, so
//! that it looks fixed: the player probes only under audio sync, and asks for each probe.
//!
//! A **probe** lets [`SETTLE`] frames and [`SETTLE_NS`] go by, then reads [`INTERVALS`] intervals
//! between frames presented one after the other. An interval is whole when it lies within
//! [`TOLERANCE_NS`] of a whole number of the refresh period the compositor gives. The display runs at a fixed rate when
//! at least half the intervals are whole.
//!
//! The figures, set from M1.4b's logs on the owner's desktop at 240 Hz, Umbriel and KWin, by
//! fixed rate and VRR ([measurements]): at a fixed rate, no interval of about 70,000 lay more than
//! 6.1 µs from a whole number of refreshes; under VRR, no window of 24 intervals held more than 4
//! within 50 µs (17 %), whatever the film's rate, its period a whole number of refreshes (24 and
//! 60 fps at 240 Hz) included.
//!
//! [ADR-0021]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md
//! [ADR-0022]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0022-the-relay-tells-the-display-s-rate-when-the-player-asks.md
//! [measurements]: https://github.com/HoneySlatt/Aerith/blob/main/docs/measurements.md#m14c--display-sync-by-the-displays-rate

use crate::plane::DisplayRate;

/// The frames a probe lets go by before it reads intervals: those mpv timed before it was asked
/// for audio sync, in its swapchain (two by default) or on their way, with a margin.
pub(super) const SETTLE: u32 = 8;

/// How long a probe lets frames go by, from the first frame presented after it was asked, on the
/// compositor's clock. Under display sync, mpv presents a frame again at each refresh until its
/// share of refreshes runs out, and a change of `video-sync` leaves the frame on screen to its end
/// (mpv 0.42, `video/out/vo.c`): up to one film frame (41.7 ms at 23.976 fps) of presents one
/// refresh apart, then the swapchain's two, 33 ms at 60 Hz. Those would read as whole intervals,
/// as a VRR display fed at every refresh runs at its top rate. Counted in frames, they grow with
/// the refresh rate (22 at 480 Hz); in time, they stay within 100 ms for sources of about 10 fps
/// and more.
pub(super) const SETTLE_NS: u128 = 100_000_000;

/// The intervals a probe reads: one second of a 24 fps film.
pub(super) const INTERVALS: u32 = 24;

/// How far from a whole number of refreshes an interval may lie and still be whole: eight times
/// the farthest seen at a fixed rate, and short of the scatter of frames under VRR.
pub(super) const TOLERANCE_NS: u128 = 50_000;

/// What the relay tells the player of the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayNews {
    /// The probe the player numbered `probe` ended: the display runs at `rate`.
    Rate { probe: u64, rate: DisplayRate },
    /// The window or its display changed since the last probe ended: the plane's size, the
    /// outputs it is on, or the display's refresh period. The rate may have changed with it, as
    /// a compositor's automatic VRR turns on and off with fullscreen.
    Changed,
}

/// A frame the compositor presented, as a probe reads it: when, and the refresh period then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Timed {
    pub(super) at_ns: u128,
    pub(super) refresh_ns: u32,
}

/// The relay's probe of the display's rate.
#[derive(Debug, Default)]
pub(super) struct Probe {
    phase: Phase,
    /// The last frame presented with a time the display's hardware gave, and a refresh period:
    /// a frame without one leaves it.
    last: Option<Timed>,
}

#[derive(Debug, Default)]
enum Phase {
    /// No probe was asked for since the plane was made.
    #[default]
    Idle,
    /// A probe the player numbered `number` reads the frames.
    Watching {
        number: u64,
        /// Frames still to let go by.
        settle: u32,
        /// Until when frames are let go by, once a frame with a time came: [`SETTLE_NS`] after it.
        settle_until: Option<u128>,
        /// The frame read last since the settle, which the next makes an interval with.
        previous: Option<Timed>,
        intervals: u32,
        whole: u32,
    },
    /// The last probe ended; nothing changed since.
    Ended,
    /// The last probe ended, and a change was reported since: the player is to ask again.
    Reported,
}

impl Probe {
    /// Starts the probe the player numbered `number`, mpv now under audio sync, in place of any
    /// other.
    pub(super) fn watch(&mut self, number: u64) {
        self.phase = Phase::Watching {
            number,
            settle: SETTLE,
            settle_until: None,
            previous: None,
            intervals: 0,
            whole: 0,
        };
    }

    /// The window or its display changed. A probe under way starts again, as frames before the
    /// change say nothing of the display after it. Once a probe ended, the change is news for
    /// the player, told once until it asks again.
    pub(super) fn changed(&mut self) -> Option<DisplayNews> {
        match &mut self.phase {
            Phase::Watching {
                settle,
                settle_until,
                previous,
                intervals,
                whole,
                ..
            } => {
                (*settle, *settle_until, *previous) = (SETTLE, None, None);
                (*intervals, *whole) = (0, 0);
                None
            }
            Phase::Ended => {
                self.phase = Phase::Reported;
                Some(DisplayNews::Changed)
            }
            Phase::Idle | Phase::Reported => None,
        }
    }

    /// Reads a frame the compositor presented: `frame` is `None` when its time is not one the
    /// display's hardware gave (no `vsync` or `hw_clock` flag) or the compositor gave no refresh
    /// period, and then breaks the chain of intervals. Returns the news it makes, if any.
    pub(super) fn presented(&mut self, frame: Option<Timed>) -> Option<DisplayNews> {
        if let Some(frame) = frame
            && self
                .last
                .replace(frame)
                .is_some_and(|last| last.refresh_ns != frame.refresh_ns)
        {
            return self.changed();
        }
        let Phase::Watching {
            number,
            settle,
            settle_until,
            previous,
            intervals,
            whole,
        } = &mut self.phase
        else {
            return None;
        };
        if let Some(frame) = frame {
            settle_until.get_or_insert(frame.at_ns + SETTLE_NS);
        }
        if *settle > 0 {
            *settle -= 1;
            return None;
        }
        let settled = frame
            .zip(*settle_until)
            .is_some_and(|(frame, until)| frame.at_ns >= until);
        // A frame without a time breaks the chain, as does each frame still settling.
        if !settled {
            *previous = None;
            return None;
        }
        let (Some(previous), Some(frame)) = (std::mem::replace(previous, frame), frame) else {
            return None;
        };
        // A time that goes back, or stands still, is no interval: an output changed.
        let interval = frame
            .at_ns
            .checked_sub(previous.at_ns)
            .filter(|ns| *ns > 0)?;
        *intervals += 1;
        if is_whole(interval, u128::from(frame.refresh_ns)) {
            *whole += 1;
        }
        if *intervals < INTERVALS {
            return None;
        }
        let rate = if *whole * 2 >= INTERVALS {
            DisplayRate::Fixed
        } else {
            DisplayRate::Variable
        };
        let probe = *number;
        self.phase = Phase::Ended;
        Some(DisplayNews::Rate { probe, rate })
    }
}

/// Whether `interval` lies within [`TOLERANCE_NS`] of a whole number, one or more, of
/// `refresh`, both in nanoseconds. A refresh of 0, which the protocol allows, is never whole.
fn is_whole(interval: u128, refresh: u128) -> bool {
    if refresh == 0 {
        return false;
    }
    let refreshes = (interval + refresh / 2) / refresh;
    refreshes >= 1 && interval.abs_diff(refreshes * refresh) <= TOLERANCE_NS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 240.016 Hz, as the owner's display gives it.
    const REFRESH: u32 = 4_166_388;

    fn frame(at_ns: u128) -> Option<Timed> {
        Some(Timed {
            at_ns,
            refresh_ns: REFRESH,
        })
    }

    /// Feeds `probe` frames presented `gaps` nanoseconds apart, from `start`, and returns the
    /// news they made, with the time of the last frame.
    fn feed(probe: &mut Probe, start: u128, gaps: &[u128]) -> (Vec<DisplayNews>, u128) {
        let mut at = start;
        let mut news: Vec<DisplayNews> = probe.presented(frame(at)).into_iter().collect();
        for gap in gaps {
            at += gap;
            news.extend(probe.presented(frame(at)));
        }
        (news, at)
    }

    /// The gaps of a 23.976 fps film at a fixed 240.016 Hz: 10 refreshes, and 11 now and then.
    fn fixed_gaps(count: usize) -> Vec<u128> {
        (0..count)
            .map(|index| u128::from(REFRESH) * if index % 9 == 4 { 11 } else { 10 })
            .collect()
    }

    /// The gaps of a 24 fps film under VRR: its period, 10.0007 refreshes, give or take a
    /// scatter of up to 0.6 ms, as M1.4b measured. The scatter is a fixed pattern, not random,
    /// and keeps every gap more than 50 µs from a whole number of refreshes.
    fn variable_gaps(count: usize) -> Vec<u128> {
        let scatter: [i64; 7] = [
            -410_000, 230_000, -90_000, 580_000, -260_000, 120_000, -330_000,
        ];
        (0..count)
            .map(|index| (41_666_667_i64 + scatter[index % scatter.len()]) as u128)
            .collect()
    }

    #[test]
    fn a_fixed_rate_is_told_once_the_settle_and_the_intervals_are_read() {
        let mut probe = Probe::default();
        probe.watch(7);
        // The settle's frames, the frame that starts the chain, then every interval but the last.
        let (news, at) = feed(
            &mut probe,
            1_000_000_000,
            &fixed_gaps((SETTLE + INTERVALS) as usize - 1),
        );
        assert!(news.is_empty(), "{news:?}");
        let news = probe.presented(frame(at + u128::from(REFRESH) * 10));
        assert_eq!(
            news,
            Some(DisplayNews::Rate {
                probe: 7,
                rate: DisplayRate::Fixed
            })
        );
        // A probe tells once.
        let (news, _) = feed(&mut probe, at + u128::from(REFRESH) * 20, &fixed_gaps(40));
        assert!(news.is_empty(), "{news:?}");
    }

    #[test]
    fn frames_scattered_between_refreshes_are_variable() {
        let mut probe = Probe::default();
        probe.watch(1);
        let gaps = variable_gaps((SETTLE + INTERVALS) as usize);
        let (news, _) = feed(&mut probe, 0, &gaps);
        assert_eq!(
            news,
            [DisplayNews::Rate {
                probe: 1,
                rate: DisplayRate::Variable
            }]
        );
    }

    #[test]
    fn half_the_intervals_whole_is_a_fixed_rate_and_fewer_is_not() {
        let half = INTERVALS as usize / 2;
        for (whole, expected) in [
            (half, DisplayRate::Fixed),
            (half - 1, DisplayRate::Variable),
        ] {
            let mut probe = Probe::default();
            probe.watch(1);
            let mut gaps = fixed_gaps(SETTLE as usize + whole);
            gaps.extend(variable_gaps(INTERVALS as usize - whole));
            let (news, _) = feed(&mut probe, 0, &gaps);
            assert_eq!(
                news,
                [DisplayNews::Rate {
                    probe: 1,
                    rate: expected
                }],
                "{whole} whole"
            );
        }
    }

    #[test]
    fn the_tolerance_is_fifty_microseconds_either_side() {
        let refresh = u128::from(REFRESH);
        assert!(is_whole(10 * refresh + TOLERANCE_NS, refresh));
        assert!(is_whole(10 * refresh - TOLERANCE_NS, refresh));
        assert!(!is_whole(10 * refresh + TOLERANCE_NS + 1, refresh));
        assert!(!is_whole(10 * refresh - TOLERANCE_NS - 1, refresh));
        // Less than half a refresh is no whole number of them, however near zero.
        assert!(!is_whole(TOLERANCE_NS, refresh));
        // A huge interval, an hour of a paused film, is read without overflow.
        assert!(is_whole(864_000 * refresh, refresh));
    }

    #[test]
    fn nothing_is_told_without_a_probe() {
        let mut probe = Probe::default();
        let (news, _) = feed(&mut probe, 0, &fixed_gaps(100));
        assert!(news.is_empty());
        assert_eq!(probe.changed(), None);
    }

    #[test]
    fn frames_without_a_hardware_time_are_not_read() {
        let mut probe = Probe::default();
        probe.watch(1);
        for _ in 0..1000 {
            assert_eq!(probe.presented(None), None);
        }
        // A frame between two without a time makes no interval with either.
        let mut at = 0;
        for _ in 0..(SETTLE + INTERVALS) {
            at += u128::from(REFRESH) * 10;
            assert_eq!(probe.presented(frame(at)), None);
            assert_eq!(probe.presented(None), None);
        }
    }

    #[test]
    fn a_time_that_goes_back_makes_no_interval() {
        let mut probe = Probe::default();
        probe.watch(1);
        let (_, at) = feed(&mut probe, 10_000_000_000, &fixed_gaps(SETTLE as usize));
        for index in 0..u128::from(INTERVALS) * 2 {
            assert_eq!(probe.presented(frame(at - index)), None);
        }
    }

    #[test]
    fn a_change_restarts_a_probe_under_way() {
        let mut probe = Probe::default();
        probe.watch(3);
        let (news, at) = feed(
            &mut probe,
            0,
            &fixed_gaps((SETTLE + INTERVALS) as usize - 1),
        );
        assert!(news.is_empty());
        assert_eq!(probe.changed(), None, "a probe under way tells nothing");
        // The settle and every interval again, then the display, now variable, is told.
        let (news, _) = feed(
            &mut probe,
            at,
            &variable_gaps((SETTLE + INTERVALS) as usize),
        );
        assert_eq!(
            news,
            [DisplayNews::Rate {
                probe: 3,
                rate: DisplayRate::Variable
            }]
        );
    }

    #[test]
    fn a_change_after_a_probe_is_told_once_until_the_next_probe() {
        let mut probe = Probe::default();
        probe.watch(1);
        let (_, at) = feed(&mut probe, 0, &fixed_gaps((SETTLE + INTERVALS) as usize));
        assert_eq!(probe.changed(), Some(DisplayNews::Changed));
        assert_eq!(probe.changed(), None);
        let (news, at) = feed(&mut probe, at, &fixed_gaps(100));
        assert!(news.is_empty(), "no probe asked again: {news:?}");
        probe.watch(2);
        let (news, _) = feed(&mut probe, at, &fixed_gaps((SETTLE + INTERVALS) as usize));
        assert_eq!(
            news,
            [DisplayNews::Rate {
                probe: 2,
                rate: DisplayRate::Fixed
            }]
        );
        assert_eq!(probe.changed(), Some(DisplayNews::Changed));
    }

    #[test]
    fn a_new_refresh_period_is_a_change() {
        let mut probe = Probe::default();
        probe.watch(1);
        let (_, at) = feed(&mut probe, 0, &fixed_gaps((SETTLE + INTERVALS) as usize));
        // The display's mode went from 240 Hz to 60 Hz.
        let sixty = Some(Timed {
            at_ns: at + 16_666_667,
            refresh_ns: 16_666_667,
        });
        assert_eq!(probe.presented(sixty), Some(DisplayNews::Changed));
        // During a probe, it starts the probe again: no verdict from the frames before.
        probe.watch(2);
        let (news, at) = feed(
            &mut probe,
            at + 20_000_000,
            &fixed_gaps(SETTLE as usize + 4),
        );
        assert!(news.is_empty());
        let sixty = Some(Timed {
            at_ns: at + 16_666_667,
            refresh_ns: 16_666_667,
        });
        assert_eq!(probe.presented(sixty), None);
        let mut at = at + 16_666_667;
        let mut news = Vec::new();
        for _ in 0..(SETTLE + INTERVALS) {
            at += 2 * 16_666_667;
            news.extend(probe.presented(Some(Timed {
                at_ns: at,
                refresh_ns: 16_666_667,
            })));
        }
        assert!(news.is_empty(), "the settle and the chain again: {news:?}");
        at += 2 * 16_666_667;
        assert_eq!(
            probe.presented(Some(Timed {
                at_ns: at,
                refresh_ns: 16_666_667,
            })),
            Some(DisplayNews::Rate {
                probe: 2,
                rate: DisplayRate::Fixed
            })
        );
    }

    #[test]
    fn a_new_probe_replaces_one_under_way() {
        let mut probe = Probe::default();
        probe.watch(1);
        let (_, at) = feed(&mut probe, 0, &fixed_gaps(SETTLE as usize + 10));
        probe.watch(2);
        let (news, _) = feed(&mut probe, at, &fixed_gaps((SETTLE + INTERVALS) as usize));
        assert_eq!(
            news,
            [DisplayNews::Rate {
                probe: 2,
                rate: DisplayRate::Fixed
            }]
        );
    }

    #[test]
    fn display_sync_repeats_after_a_switch_are_not_read_as_a_fixed_rate() {
        // At 480 Hz, a 24 fps frame mpv timed under display sync shows 20 refreshes, each a
        // present, and mpv's swapchain holds 2 more: all left over when the probe is asked.
        const REFRESH_480: u32 = 2_083_333;
        let mut probe = Probe::default();
        probe.watch(2);
        let mut at = 1_000_000_000_u128;
        let mut news = Vec::new();
        for _ in 0..22 {
            at += u128::from(REFRESH_480);
            news.extend(probe.presented(Some(Timed {
                at_ns: at,
                refresh_ns: REFRESH_480,
            })));
        }
        for gap in variable_gaps(40) {
            at += gap;
            news.extend(probe.presented(Some(Timed {
                at_ns: at,
                refresh_ns: REFRESH_480,
            })));
        }
        assert_eq!(
            news,
            [DisplayNews::Rate {
                probe: 2,
                rate: DisplayRate::Variable
            }]
        );
    }

    #[test]
    fn a_new_refresh_period_behind_an_untimed_frame_is_still_a_change() {
        let mut probe = Probe::default();
        probe.watch(1);
        let (_, at) = feed(&mut probe, 0, &fixed_gaps((SETTLE + INTERVALS) as usize));
        assert_eq!(probe.presented(None), None);
        let sixty = Some(Timed {
            at_ns: at + 16_666_667,
            refresh_ns: 16_666_667,
        });
        assert_eq!(probe.presented(sixty), Some(DisplayNews::Changed));
    }

    #[test]
    fn a_refresh_period_of_zero_is_never_whole() {
        assert!(!is_whole(41_666_667, 0));
    }
}
