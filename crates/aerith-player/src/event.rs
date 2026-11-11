// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the player tells the rest of Aerith: each change of playback as an [`Event`], and what the
//! engine actually does as a [`Report`].

use std::time::{Duration, Instant};

/// Something that happened in the player. Events come in the order the engine reported them.
/// Those about a source carry the [`OpenId`] that `Player::open` returned for it, so that
/// an event of a source already replaced is told apart.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The source opened and its first frame is shown (or, for a source without picture, its sound
    /// starts). It may start paused, if the player was paused.
    Started(OpenId),
    /// The player was paused (`true`) or plays again (`false`).
    Paused(bool),
    /// Where playback of the source stands. Sent when the relation between the position and time
    /// changes: after the start, after a seek, when playback stops or starts running; not while it
    /// runs, so that a playing player wakes nothing up. [`Clock::position_at`] gives the position
    /// at any moment.
    Clock(OpenId, Clock),
    /// What the engine decodes and outputs for the source, as it reports it. Sent once the source
    /// started, then at each change (a decoder that falls back to software, a new sound format, a
    /// track that ends before the others).
    Report(OpenId, Box<Report>),
    /// The source has a picture the engine could not show, because its video output failed (no
    /// GPU, no Wayland session); it plays on without it.
    PictureFailed(OpenId),
    /// The source stopped playing, or will never start: it failed, or a later open replaced it.
    Ended(OpenId, Ending),
    /// The engine refused a command; playback goes on as before.
    Refused {
        /// The command: `play`, `pause`, `seek` or `stop`. An open that fails ends instead.
        command: &'static str,
        /// The engine's reason, in its words.
        reason: String,
    },
    /// The engine stopped by itself or broke its connection: it crashed, was killed, or quit. The
    /// player can do nothing more, and sends no other event; a new one must be started. Its reason
    /// says how the engine ended.
    EngineFailed(String),
}

/// The number of an open, which the events of its source carry. Opens are numbered from 1, in
/// the order they were made, for the player's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpenId(pub(crate) u64);

/// The position of playback at an instant, and whether it advances from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// The position in the source when the engine was asked.
    pub position: Duration,
    /// When the engine answered.
    pub at: Instant,
    /// Whether the position advances in real time from `at`. It does not while paused, while
    /// seeking, or while a stream waits for data.
    pub running: bool,
}

impl Clock {
    /// The position at `now`, assuming playback ran as the clock said since `at`. Accurate to the
    /// engine's own drift against the system clock, until the next [`Event::Clock`].
    pub fn position_at(&self, now: Instant) -> Duration {
        if self.running {
            self.position
                .saturating_add(now.saturating_duration_since(self.at))
        } else {
            self.position
        }
    }
}

/// How a source stopped playing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    /// It played to its end.
    Finished,
    /// It was stopped, or replaced by another source.
    Stopped,
    /// It could not be opened, or failed while playing; the engine's reason, in its words.
    Failed(String),
}

/// What the engine decodes and outputs, as it reports it, never as the file or its name claim.
///
/// The names of codecs, formats and colours are the engine's own (mpv's: `hevc`, `bt.2020`, `pq`,
/// `5.1(side)`); they will be made common to every engine when a second engine exists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// The picture, if a video track plays.
    pub video: Option<VideoReport>,
    /// The sound, if an audio track plays.
    pub audio: Option<AudioReport>,
}

/// What the engine does with the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoReport {
    /// The codec of the track, such as `hevc`.
    pub codec: String,
    /// The hardware decoder in use, such as `vaapi`; `None` when decoded in software.
    pub hardware_decoder: Option<String>,
    /// The decoded picture's size, in pixels.
    pub width: u32,
    /// The decoded picture's height, in pixels.
    pub height: u32,
    /// The decoded picture's colour.
    pub decoded: Colour,
    /// The video output in use, such as `gpu-next`, or `null` when the picture is discarded.
    pub output: String,
    /// The colour the output draws in; `None` when nothing is drawn.
    pub drawn: Option<Colour>,
}

/// How a picture's colours are to be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Colour {
    /// The primaries, such as `bt.709` or `bt.2020`.
    pub primaries: String,
    /// The transfer function, such as `bt.1886`, `pq` or `hlg`.
    pub transfer: String,
    /// The peak brightness relative to SDR white, when the source tells one: above 1 for HDR.
    pub peak: Option<f64>,
}

/// What the engine does with the sound.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioReport {
    /// The codec of the track, such as `eac3` or `truehd`.
    pub codec: String,
    /// The decoded sound.
    pub decoded: AudioFormat,
    /// The audio output in use, such as `pipewire`, or `null` when the sound is discarded.
    pub output: String,
    /// What reaches the output; `None` until the output opens.
    pub sent: Option<AudioFormat>,
}

/// A sound's format.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioFormat {
    /// The sample format, such as `floatp`, or the encoded stream passed through, such as
    /// `spdif-eac3`.
    pub format: String,
    /// Samples a second.
    pub sample_rate: u32,
    /// The channel layout, such as `stereo` or `7.1`.
    pub channels: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_position_saturates_rather_than_panics() {
        let at = Instant::now();
        let clock = Clock {
            position: Duration::MAX,
            at,
            running: true,
        };
        assert_eq!(
            clock.position_at(at + Duration::from_secs(3600)),
            Duration::MAX
        );
    }

    #[test]
    fn a_running_clock_advances_and_a_stopped_one_does_not() {
        let at = Instant::now();
        let running = Clock {
            position: Duration::from_secs(10),
            at,
            running: true,
        };
        let later = at + Duration::from_millis(1500);
        assert_eq!(running.position_at(later), Duration::from_millis(11_500));
        // An instant before the clock's own reads as the clock's position, never less.
        assert_eq!(running.position_at(at), Duration::from_secs(10));
        let stopped = Clock {
            running: false,
            ..running
        };
        assert_eq!(stopped.position_at(later), Duration::from_secs(10));
    }
}
