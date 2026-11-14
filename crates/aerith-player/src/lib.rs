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
//! of the crate's `mpv` module, in `src/mpv.rs`, says how. Its picture is discarded, or shown in a
//! `Plane` of the interface's Wayland window, through a relay of the player's ([ADR-0015]; the
//! crate's `relay` module, in `src/relay/`). Windows joins at milestone M1.6 of the [plan], Android
//! and Apple at their stages. Tracks, subtitles and speed join the interface when a milestone
//! needs them.
//!
//! ```no_run
//! # #[cfg(target_os = "linux")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use aerith_player::{Audio, Event, Player, Settings, Source, Video};
//!
//! let settings = Settings {
//!     engine: "mpv".into(),
//!     video: Video::Discarded,
//!     audio: Audio::System,
//!     cache: "/var/cache/aerith/player".into(),
//! };
//! let (player, events) = Player::start(&settings)?;
//! player.open(&Source::file("film.mkv")?)?;
//! for event in events {
//!     match event {
//!         Event::Started(_) => player.pause()?,
//!         Event::Ended(..) | Event::EngineFailed(_) => break,
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
//! [ADR-0015]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0015-a-relay-for-mpvs-wayland-surface.md
//! [plan]: https://github.com/HoneySlatt/Aerith/blob/main/docs/PLAN.md#m16--windows

mod event;
#[cfg(target_os = "linux")]
mod mpv;
#[cfg(target_os = "linux")]
mod plane;
#[cfg(target_os = "linux")]
mod player;
#[cfg(target_os = "linux")]
mod relay;
mod source;

pub use event::{
    AudioFormat, AudioReport, Clock, Colour, DeclaredDolbyVision, Ending, Event, OpenId, Report,
    VideoReport,
};
#[cfg(target_os = "linux")]
pub use plane::{COUNTED_GAPS, DisplayRate, InvalidPlace, Plane, Presentation, Window};
#[cfg(target_os = "linux")]
pub use player::{Audio, EngineGone, Player, Settings, Video};
pub use source::{Header, InvalidSource, LONGEST_HEADER, LONGEST_URL, MOST_HEADERS, Source};

/// What the fuzz target (`fuzz/`) needs of the crate, and nothing more: built only with the
/// `fuzzing` feature, which nothing that ships turns on.
#[cfg(all(target_os = "linux", feature = "fuzzing"))]
#[doc(hidden)]
pub mod fuzzing {
    use std::io;
    use std::os::unix::net::UnixStream;

    use crate::plane::{Plane, Window};
    use crate::relay::Relay;

    /// The relay on `window`, serving mpv's side on `mpv` as the player does, without mpv.
    /// Memory files pass as dma-bufs, as the relay's own tests have them, so that the fuzz target
    /// reaches the dmabuf checks too. `failed` is called as the player's engine would be.
    pub fn plane(
        window: Window,
        mpv: UnixStream,
        failed: Box<dyn Fn(String) + Send>,
    ) -> io::Result<Plane> {
        let relay = Relay::start(window, mpv, failed, Box::new(|_| {}))?;
        relay.change(|state| state.take_memory_as_dmabuf());
        Ok(Plane::new(relay))
    }
}
