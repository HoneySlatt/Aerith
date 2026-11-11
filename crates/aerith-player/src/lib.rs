// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The one interface through which the rest of Aerith drives playback, whatever the engine.
//!
//! The crate holds no code yet. Its contract and its engines are decided in [ADR-0004]; it arrives
//! at milestone M1.3 of the [plan], with mpv as its first engine.
//!
//! [ADR-0004]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0004-a-player-engine-per-platform.md
//! [plan]: https://github.com/HoneySlatt/Aerith/blob/main/docs/PLAN.md#m13--mpv-driven-through-aerith-player
