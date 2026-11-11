// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The one interface through which the rest of Aerith drives playback, whatever the engine
//! ([ADR-0004]).
//!
//! The crate holds no code yet: the interface arrives at milestone M1.3 of [`docs/PLAN.md`], with
//! its first engine. What it will hold:
//!
//! - **One contract for every engine**: open, play, pause, seek, tracks, subtitles, speed, the
//!   position and its clock, what the engine actually did (decoder, colour, audio path), and
//!   errors. What an engine cannot do is reported, never faked.
//! - **Three engines behind it**: mpv on Linux and Windows, a program apart driven over its IPC
//!   ([ADR-0003]); Media3 on Android; AVFoundation on Apple platforms.
//!
//! No engine detail leaks out of this crate: the rest of Aerith reaches playback only through it.
//! Decoders written in C never run in the process that holds Aerith's tokens.
//!
//! [ADR-0003]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0003-mpv-in-its-own-process.md
//! [ADR-0004]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0004-a-player-engine-per-platform.md
//! [`docs/PLAN.md`]: https://github.com/HoneySlatt/Aerith/blob/main/docs/PLAN.md
