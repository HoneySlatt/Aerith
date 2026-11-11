// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The plane: the engine's picture inside the interface's window, on Linux ([ADR-0015]).
//!
//! The interface hands the player its Wayland display and window surfaces ([`Window`]); the player
//! shows the engine's picture in a surface of its own, below one of the interface's, and the
//! interface places, shows and hides it through a [`Plane`]. How the engine's requests reach the
//! window is the crate's `relay` module's to say.
//!
//! [ADR-0015]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0015-a-relay-for-mpvs-wayland-surface.md

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;

use raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle};

use crate::relay::Relay;

/// The interface's window, where the plane is shown: its Wayland connection, the surface the
/// plane is a child of, and the surface the plane lies directly below.
///
/// `below` is the parent itself, to show the picture under the window's own content, or a
/// subsurface of the parent's, to show it above the parent and under that subsurface. Anything
/// else is a protocol error on the interface's own connection, which the compositor answers by
/// closing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    display: NonNull<c_void>,
    parent: NonNull<c_void>,
    below: NonNull<c_void>,
}

impl Window {
    /// The window, from the handles the interface's windowing library gives (`raw-window-handle`).
    ///
    /// # Safety
    ///
    /// `display` is a libwayland `wl_display`, and `parent` and `below` are `wl_surface`s on it,
    /// as winit and other libraries hand them out. All three stay alive until the player's plane
    /// is closed: by [`Plane::close`], or by dropping the player. The interface therefore closes
    /// the plane before it destroys its window.
    pub unsafe fn new(
        display: WaylandDisplayHandle,
        parent: WaylandWindowHandle,
        below: WaylandWindowHandle,
    ) -> Self {
        Self {
            display: display.display,
            parent: parent.surface,
            below: below.surface,
        }
    }

    pub(crate) fn display(&self) -> *mut c_void {
        self.display.as_ptr()
    }

    pub(crate) fn parent(&self) -> *mut c_void {
        self.parent.as_ptr()
    }

    pub(crate) fn below(&self) -> *mut c_void {
        self.below.as_ptr()
    }
}

// SAFETY: a `Window` is three addresses; it reads nothing through them itself. The relay uses
// them from the thread that starts the player and then from its own thread, through libwayland
// only, whose `wl_display` and proxies may be used from any thread: libwayland locks the display
// for each request it sends, and the relay reads and dispatches only a queue of its own, never
// the interface's. `Window::new`'s contract keeps them alive while the relay uses them.
unsafe impl Send for Window {}
// SAFETY: as for `Send`: shared references give nothing more than copies of the addresses.
unsafe impl Sync for Window {}

/// The engine's picture in the interface's window. A player started with
/// [`crate::Video::Plane`] has one ([`crate::Player::plane`]); it starts hidden.
///
/// Each method takes effect at the interface's next commit of the parent surface, so that the
/// plane moves, resizes, shows and hides in the same frame as the interface's own content. None
/// waits on the engine: the plane scales the engine's last frame to its new size until the engine
/// draws at that size. Calls may come from any thread.
pub struct Plane {
    relay: Relay,
}

impl Plane {
    pub(crate) fn new(relay: Relay) -> Self {
        Self { relay }
    }

    /// The relay, for its tests.
    #[cfg(test)]
    pub(crate) fn relay(&self) -> &Relay {
        &self.relay
    }

    /// Places the plane: its top-left corner at `x`, `y` and its size `width` by `height`, in the
    /// logical coordinates of the parent surface. The engine is told the new size, and draws at it
    /// at its own pace.
    pub fn place(&self, x: i32, y: i32, width: u32, height: u32) -> Result<(), InvalidPlace> {
        let size = |side: u32| i32::try_from(side).ok().filter(|side| *side > 0);
        let (Some(width), Some(height)) = (size(width), size(height)) else {
            return Err(InvalidPlace);
        };
        self.relay.change(|state| state.place(x, y, width, height));
        Ok(())
    }

    /// Shows the plane, with the engine's last frame, and lets the engine draw. Without a frame
    /// yet, the plane shows from the engine's next one.
    pub fn show(&self) {
        self.relay.change(|state| state.show());
    }

    /// Hides the plane, and tells the engine it is not seen: it stops drawing until shown again.
    /// Playback goes on.
    pub fn hide(&self) {
        self.relay.change(|state| state.hide());
    }

    /// Closes the plane: everything the player made on the interface's connection is destroyed,
    /// before this returns, from the calling thread, which must not be inside a read of that
    /// connection. A call made while another thread closes the plane returns once that one is
    /// done. Later calls on the plane do nothing.
    ///
    /// The engine loses its window and quits: the player then reports
    /// [`crate::Event::EngineFailed`], as for any end of the engine, and a new player is started to
    /// play on. Dropping the player closes the plane too, but the interface closes it itself before
    /// it destroys its window ([`Window::new`]).
    pub fn close(&self) {
        self.relay.close();
    }

    /// What the compositor reported of the engine's frames since the plane was made.
    pub fn presentation(&self) -> Presentation {
        self.relay.presentation()
    }
}

impl fmt::Debug for Plane {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Plane").finish_non_exhaustive()
    }
}

/// The longest gap between two frames presented that [`Presentation::gaps`] counts by itself, in
/// refreshes of the display.
pub const COUNTED_GAPS: usize = 32;

/// The engine's frames, for the bench: how many it drew, and what the compositor reported of them
/// through `wp_presentation`, one report for each frame the plane showed. Counted since the plane
/// was made; read it before and after a run, and subtract.
///
/// The compositor sends no report when it has no `wp_presentation`, or while the plane is hidden
/// or covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Presentation {
    /// Frames the engine committed to the plane, each a new picture, whether the plane showed it or
    /// not. The engine is told when the plane is hidden, and draws none then.
    pub committed: u64,
    /// Frames the compositor put on screen.
    pub presented: u64,
    /// Frames the compositor never showed: replaced before a refresh, or not on screen.
    pub discarded: u64,
    /// Of the frames presented, those scanned out straight from the engine's buffer, without
    /// the compositor drawing them (`zero_copy`).
    pub zero_copy: u64,
    /// `gaps[n]` counts the frames presented `n` refreshes after the frame presented before them:
    /// a film at 23.976 frames a second on a 240 Hz display alternates gaps of 10 and 11;
    /// a longer gap is a frame shown too long. Gaps of [`COUNTED_GAPS`] refreshes and more are in
    /// `longer_gaps`. A refresh counter that goes back (an output changed) starts the gaps again.
    pub gaps: [u64; COUNTED_GAPS],
    /// Gaps of [`COUNTED_GAPS`] refreshes or more.
    pub longer_gaps: u64,
}

/// A plane's width and height are between 1 and 2³¹ − 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidPlace;

impl fmt::Display for InvalidPlace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a plane's width and height are between 1 and 2^31 - 1")
    }
}

impl std::error::Error for InvalidPlace {}
