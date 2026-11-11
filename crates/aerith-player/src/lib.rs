// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The one interface through which the rest of Aerith drives playback, whatever the engine
//! ([ADR-0004]).
//!
//! A `Player` opens a [`Source`] (a file, or a stream with its headers), plays, pauses, seeks and
//! stops it. Each command returns at once; what the engine does comes back as [`Event`]s: the first
//! frame, the pause, a [`Clock`] from which the position is read at any moment, a [`Report`] of
//! what the engine actually decodes and outputs, the end, and the engine's failure. Nothing of the
//! engine shows through but the names it gives codecs and colours ([`Report`]).
//!
//! On Linux the engine is mpv, a program apart driven over its IPC ([ADR-0003]); the documentation
//! of the crate's `mpv` module, in `src/mpv.rs`, says how. Windows joins at milestone M1.6 of the [plan], Android and Apple at
//! their stages. Tracks, subtitles and speed join the interface when a milestone needs them.
//!
//! ```no_run
//! # #[cfg(target_os = "linux")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use aerith_player::{Audio, Event, Player, Settings, Source, Video};
//!
//! let settings = Settings {
//!     engine: "mpv".into(),
//!     video: Video::Window,
//!     audio: Audio::System,
//!     cache: "/var/cache/aerith/player".into(),
//! };
//! let (player, events) = Player::start(&settings)?;
//! player.open(&Source::file("film.mkv")?)?;
//! for event in events {
//!     match event {
//!         Event::Started => player.pause()?,
//!         Event::Ended(_) | Event::EngineFailed(_) => break,
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(target_os = "linux"))]
//! # fn main() {}
//! ```
//!
//! [ADR-0003]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0003-mpv-in-its-own-process.md
//! [ADR-0004]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0004-a-player-engine-per-platform.md
//! [plan]: https://github.com/HoneySlatt/Aerith/blob/main/docs/PLAN.md#m16--windows

mod event;
#[cfg(target_os = "linux")]
mod mpv;
#[cfg(target_os = "linux")]
mod player;
mod source;

pub use event::{AudioFormat, AudioReport, Clock, Colour, Ending, Event, Report, VideoReport};
#[cfg(target_os = "linux")]
pub use player::{Audio, EngineGone, Player, Settings, Video};
pub use source::{Header, InvalidSource, LONGEST_HEADER, LONGEST_URL, MOST_HEADERS, Source};
