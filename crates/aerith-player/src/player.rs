// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The player: what starts an engine and drives it.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::event::Event;
use crate::mpv;
use crate::source::Source;

/// How a player is started.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The engine's program: the path of mpv, or its name looked up in `PATH`.
    pub engine: PathBuf,
    /// Where the picture goes.
    pub video: Video,
    /// Where the sound goes.
    pub audio: Audio,
    /// A folder of Aerith's own where the engine keeps what it caches from one run to the next:
    /// its compiled shaders. The engine creates it when it needs it.
    pub cache: PathBuf,
}

/// Where the picture goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Video {
    /// Decoded and timed, never shown: for tests and measures, where no display exists.
    Discarded,
    /// Shown by the engine in a window of its own, through the session's Wayland compositor, and
    /// never on the screen directly. Milestone M1.4 places this picture inside Aerith's window.
    Window,
}

/// Where the sound goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audio {
    /// Decoded and timed, never played: for tests and measures. No sound device is opened.
    Discarded,
    /// Played through the system's sound server, as the engine chooses it.
    System,
}

/// Something the rest of Aerith asks of the engine.
#[derive(Clone, Debug)]
pub(crate) enum Command {
    Open(Source),
    Play,
    Pause,
    Seek(Duration),
    Stop,
}

/// The one interface through which Aerith drives playback ([ADR-0004]).
///
/// Each command returns at once: it is handed to the player's own thread, which talks to the
/// engine, so that the caller never waits on it. What the engine does in answer comes back as
/// [`Event`]s on the receiver [`Player::start`] returns. A command fails only once the engine is
/// gone, which an [`Event::EngineFailed`] has already said.
///
/// On Linux the engine is mpv, a program apart ([ADR-0003]): a crash of mpv ends that program and
/// is reported, and Aerith goes on. Dropping the player asks mpv to quit, waits up to two seconds
/// for it, then kills it; if Aerith itself dies, mpv quits as its connection closes.
///
/// [ADR-0003]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0003-mpv-in-its-own-process.md
/// [ADR-0004]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0004-a-player-engine-per-platform.md
pub struct Player {
    engine: mpv::Engine,
}

impl Player {
    /// Starts the engine, idle, and returns the player and the receiver of its events. It fails
    /// when the engine's program cannot be started; an engine that starts and then fails is
    /// reported by an [`Event::EngineFailed`].
    pub fn start(settings: &Settings) -> io::Result<(Self, Receiver<Event>)> {
        let (engine, events) = mpv::Engine::start(settings)?;
        Ok((Self { engine }, events))
    }

    /// Opens `source` and plays it, unless the player is paused; it replaces what was playing,
    /// which ends as [`crate::Ending::Stopped`]. [`Event::Started`] follows once its first frame
    /// is shown, or [`Event::Ended`] with the reason it failed.
    pub fn open(&self, source: &Source) -> Result<(), EngineGone> {
        self.engine.send(Command::Open(source.clone()))
    }

    /// Plays: the source opened, or the next one.
    pub fn play(&self) -> Result<(), EngineGone> {
        self.engine.send(Command::Play)
    }

    /// Pauses: the source opened, or the next one, which then starts paused on its first frame.
    pub fn pause(&self) -> Result<(), EngineGone> {
        self.engine.send(Command::Pause)
    }

    /// Moves to `position` from the start of the source, to the exact frame. Past the end, the
    /// source ends.
    pub fn seek(&self, position: Duration) -> Result<(), EngineGone> {
        self.engine.send(Command::Seek(position))
    }

    /// Stops the source; the engine stays started, idle, for the next one.
    pub fn stop(&self) -> Result<(), EngineGone> {
        self.engine.send(Command::Stop)
    }
}

/// The engine has stopped: an [`Event::EngineFailed`] said why. A new player must be started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineGone;

impl fmt::Display for EngineGone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the player's engine has stopped")
    }
}

impl std::error::Error for EngineGone {}
