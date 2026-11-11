// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the testbed makes itself on winit's Wayland connection, beside winit: the frame callbacks
//! that pace its drawing, and, in the "above" order, the surface it draws in and the black pixel
//! of the window's own surface.
//!
//! It joins winit's connection with an event queue of its own (wayland-client over libwayland),
//! as the player's relay does. Nothing reads that queue's events from the socket but the threads
//! that already read it: winit's loop and the relay. The window's loop dispatches them each time
//! it runs ([`Surfaces::frame_done`]).
//!
//! **The "above" order.** The window's surface, winit's, gets an opaque black buffer of one pixel,
//! in shared memory, attached once and never drawn again. The compositor scales it to the window
//! through the viewport winit 0.30.13 gives that surface when the compositor offers fractional
//! scaling (`WindowState::new` and `resize`, in winit's `platform_impl/linux/wayland/window/
//! state.rs`); the compositor's own single-pixel buffers are not used, as Umbriel has none. On it,
//! the player's plane, and above the plane a subsurface of the testbed's, which wgpu draws: its
//! input region is empty, so that the pointer and the keyboard focus stay on winit's surface,
//! whose events winit alone accepts. The subsurface stays synchronized: what wgpu presents there
//! shows at the testbed's next commit of the window's surface, with the plane's own changes. With
//! the controls hidden it holds no buffer, so that the compositor has one picture to show: the
//! video's, which it may then scan out.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::ptr::NonNull;

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::args::Order;

/// The testbed's objects on winit's connection.
pub struct Surfaces {
    connection: Connection,
    queue: EventQueue<Pacing>,
    pacing: Pacing,
    /// winit's surface, the window's own.
    root: WlSurface,
    /// The "above" order's surface and buffer; `None` in the "below" order.
    above: Option<Above>,
}

/// What the "above" order adds.
struct Above {
    /// The surface wgpu draws in, a subsurface of the window's, and its viewport.
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    /// The black pixel on the window's surface, and the memory it lives in.
    black: WlBuffer,
    pool: WlShmPool,
    _memory: File,
}

/// What the queue's events tell.
#[derive(Default)]
struct Pacing {
    /// The compositor asked for the next frame since this was last read.
    frame_done: bool,
}

impl Surfaces {
    /// Joins winit's connection, and makes what `order` needs, for a window of `logical` size.
    ///
    /// # Safety
    ///
    /// `display` is winit's `wl_display` and `root` its window's `wl_surface`, both alive until
    /// this value is dropped.
    pub unsafe fn join(
        display: NonNull<c_void>,
        root: NonNull<c_void>,
        order: Order,
        logical: (u32, u32),
    ) -> Result<Self, String> {
        // SAFETY: the caller keeps the display alive while this value, which owns the backend,
        // lives. A guest backend never closes the display itself.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        // SAFETY: the caller makes `root` a live `wl_surface` of this display for as long as this
        // value lives.
        let root_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), root.as_ptr().cast()) }
            .map_err(|_| "winit's window surface is not a wl_surface".to_owned())?;
        let root = WlSurface::from_id(&connection, root_id)
            .map_err(|_| "winit's window surface is gone".to_owned())?;
        let (globals, queue) = registry_queue_init::<Pacing>(&connection)
            .map_err(|error| format!("the window's registry: {error}"))?;
        let mut surfaces = Self {
            connection,
            queue,
            pacing: Pacing::default(),
            root,
            above: None,
        };
        if order == Order::Below {
            return Ok(surfaces);
        }
        let offered = |interface: &str| {
            globals
                .contents()
                .with_list(|list| list.iter().any(|global| global.interface == interface))
        };
        if !offered("wp_fractional_scale_manager_v1") || !offered("wp_viewporter") {
            return Err(
                "the \"above\" order needs a compositor that offers fractional scaling \
                and viewports: winit then gives the window's surface the viewport that scales \
                its black pixel"
                    .to_owned(),
            );
        }
        let qh = surfaces.queue.handle();
        let bind_error = |error| format!("the window's compositor: {error}");
        let compositor: WlCompositor = globals.bind(&qh, 4..=4, ()).map_err(bind_error)?;
        let subcompositor: WlSubcompositor = globals.bind(&qh, 1..=1, ()).map_err(bind_error)?;
        let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ()).map_err(bind_error)?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(bind_error)?;

        let memory = black_pixel().map_err(|error| format!("the black pixel's memory: {error}"))?;
        let pool = shm.create_pool(memory.as_fd(), 4, &qh, ());
        let black = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Xrgb8888, &qh, ());
        surfaces.root.attach(Some(&black), 0, 0);
        if surfaces.root.version() >= 4 {
            surfaces.root.damage_buffer(0, 0, 1, 1);
        } else {
            surfaces.root.damage(0, 0, i32::MAX, i32::MAX);
        }

        let surface = compositor.create_surface(&qh, ());
        let nowhere = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&nowhere));
        nowhere.destroy();
        // Synchronized, as a new subsurface is, and on top of the window's surface.
        let subsurface = subcompositor.get_subsurface(&surface, &surfaces.root, &qh, ());
        let viewport = viewporter.get_viewport(&surface, &qh, ());
        // What these two made lives on without them; the compositor and shared memory have no
        // request that releases them, and live as long as the connection.
        subcompositor.destroy();
        viewporter.destroy();
        surfaces.above = Some(Above {
            surface,
            subsurface,
            viewport,
            black,
            pool,
            _memory: memory,
        });
        surfaces.resize(logical);
        Ok(surfaces)
    }

    /// The surface the testbed draws in, and paces itself on: its own in the "above" order,
    /// winit's otherwise.
    pub fn drawn(&self) -> NonNull<c_void> {
        let surface = self
            .above
            .as_ref()
            .map_or(&self.root, |above| &above.surface);
        NonNull::new(surface.id().as_ptr().cast()).expect("a live surface has an address")
    }

    /// Follows the window's new logical size: the "above" order's surface is drawn at the
    /// window's size in pixels and shown at its logical size.
    pub fn resize(&self, (width, height): (u32, u32)) {
        if let Some(above) = &self.above {
            let side = |side: u32| i32::try_from(side.max(1)).unwrap_or(i32::MAX);
            above.viewport.set_destination(side(width), side(height));
        }
    }

    /// Asks the compositor to say when it wants the next frame of the surface drawn. Made before
    /// the frame is presented, it goes with that frame's commit.
    pub fn request_frame(&mut self) {
        let surface = self
            .above
            .as_ref()
            .map_or(&self.root, |above| &above.surface);
        surface.frame(&self.queue.handle(), ());
    }

    /// In the "above" order, takes the drawn surface's buffer away, so that the compositor sees
    /// the video alone. Shown at the next [`Self::commit_root`].
    pub fn clear_drawn(&self) {
        if let Some(above) = &self.above {
            above.surface.attach(None, 0, 0);
            above.surface.commit();
        }
    }

    /// Commits the window's surface: what changed below it (the plane, the drawn surface) shows.
    pub fn commit_root(&self) {
        self.root.commit();
    }

    /// Sends what was asked; a failure is the connection breaking, which winit reports.
    pub fn flush(&self) {
        let _ = self.connection.flush();
    }

    /// Handles what the compositor sent the testbed's queue, read already by another thread, and
    /// returns whether it asked for the next frame since the last call.
    pub fn frame_done(&mut self) -> Result<bool, String> {
        self.queue
            .dispatch_pending(&mut self.pacing)
            .map_err(|error| format!("the window's connection: {error}"))?;
        Ok(std::mem::take(&mut self.pacing.frame_done))
    }
}

impl Drop for Surfaces {
    /// Destroys what the testbed made, before winit destroys the window: wgpu's surface on the
    /// "above" order's surface is gone by then ([`crate::app`]).
    fn drop(&mut self) {
        if let Some(above) = self.above.take() {
            above.viewport.destroy();
            above.subsurface.destroy();
            above.surface.destroy();
            above.black.destroy();
            above.pool.destroy();
        }
        self.flush();
    }
}

/// Four bytes of shared memory holding one black pixel, XRGB: all zero. A file of the session's
/// runtime folder, a tmpfs, unlinked at once.
fn black_pixel() -> std::io::Result<File> {
    let folder = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = folder.join(format!("aerith-testbed-black-{}", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    file.set_len(4)?;
    Ok(file)
}

impl Dispatch<WlCallback, ()> for Pacing {
    fn event(
        pacing: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            pacing.frame_done = true;
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Pacing {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Globals that come and go later change nothing the testbed uses.
    }
}

delegate_noop!(Pacing: WlCompositor);
delegate_noop!(Pacing: WlSubcompositor);
delegate_noop!(Pacing: WlSubsurface);
delegate_noop!(Pacing: WlShmPool);
delegate_noop!(Pacing: WpViewporter);
delegate_noop!(Pacing: WpViewport);
delegate_noop!(Pacing: wayland_client::protocol::wl_region::WlRegion);
delegate_noop!(Pacing: ignore WlShm);
delegate_noop!(Pacing: ignore WlBuffer);
delegate_noop!(Pacing: ignore WlSurface);
