// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `xdg_wm_base`, played by the relay: mpv sees its window as a toplevel, tiled to the plane's
//! size so that it takes that size as given, activated while the plane shows and suspended while it
//! is hidden. Nothing of it reaches the window's connection.
//!
//! **Configures are paced by mpv.** A configure stays in flight from when it is sent until mpv's
//! first commit after acknowledging it; a new size meanwhile waits, and only the latest is sent.
//! An interface that resizes the plane at every frame of an animation thus has mpv resize its
//! swapchain at mpv's own pace, while the relay's viewport scales mpv's last frame to each size.
//! A change of state (shown, hidden) is sent at once.
//!
//! **mpv's toplevel is read raw.** Its `set_title` and `set_app_id` take a string, and the parser
//! wayland-server generates for them panics on a string left out (wayland-protocols 0.32.13), as a
//! client may send it: the relay reads the toplevel's requests itself ([`ToplevelData`]), and
//! refuses that one.

use std::collections::VecDeque;
use std::ffi::CString;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use wayland_protocols::xdg::shell::server::xdg_positioner::{self, XdgPositioner};
use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_server::backend::protocol::{Argument, Message, ProtocolError};
use wayland_server::backend::{ClientId, DisconnectReason, Handle, ObjectData, ObjectId};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::State;

/// The most configures mpv may leave unacknowledged; mpv acknowledges each as it comes.
const MOST_UNACKNOWLEDGED: usize = 64;

/// mpv's toplevel: its objects, and where its configures stand.
pub(super) struct Toplevel {
    surface: ObjectId,
    xdg_surface: XdgSurface,
    toplevel: XdgToplevel,
    /// The serials sent and not yet acknowledged, oldest first.
    sent: VecDeque<u32>,
    /// Whether mpv acknowledged a configure at least once: it may attach a buffer from then.
    configured: bool,
    /// Whether the first configure was sent, at mpv's first commit.
    initial_sent: bool,
    /// The configure that waits for mpv's commit after its acknowledgement.
    in_flight: Option<u32>,
    acknowledged: bool,
    /// The size last sent.
    sent_size: (i32, i32),
}

impl State {
    /// Whether mpv may attach a buffer to `surface`: it has no shell role, or acknowledged a
    /// configure.
    pub(super) fn configured(&self, surface: &ObjectId) -> bool {
        self.toplevel
            .as_ref()
            .is_some_and(|toplevel| toplevel.surface == *surface && toplevel.configured)
    }

    /// mpv attached a buffer to a shell surface before acknowledging a configure.
    pub(super) fn refuse_unconfigured_buffer(&self, surface: &ObjectId) {
        if let Some(xdg_surface) = self
            .surfaces
            .get(surface)
            .and_then(|surface| surface.xdg_surface.as_ref())
        {
            xdg_surface.post_error(
                xdg_surface::Error::UnconfiguredBuffer,
                "a buffer was attached before the first configure was acknowledged",
            );
        }
    }

    /// mpv committed the plane's surface: its first commit gets the first configure; a commit
    /// after acknowledging the configure in flight lets a waiting size go.
    pub(super) fn plane_committed(&mut self) {
        let Some(toplevel) = &mut self.toplevel else {
            return;
        };
        if !toplevel.initial_sent {
            toplevel.initial_sent = true;
            self.send_configure();
            return;
        }
        if toplevel.in_flight.is_some() && toplevel.acknowledged {
            toplevel.in_flight = None;
            if Some(toplevel.sent_size) != self.plane.size {
                self.send_configure();
            }
        }
    }

    /// The plane's size changed: mpv is told, unless a configure is in flight.
    pub(super) fn configure_size(&mut self) {
        if self
            .toplevel
            .as_ref()
            .is_some_and(|toplevel| toplevel.initial_sent && toplevel.in_flight.is_none())
        {
            self.send_configure();
        }
    }

    /// The plane was shown or hidden: mpv is told at once.
    pub(super) fn configure_state(&mut self) {
        if self
            .toplevel
            .as_ref()
            .is_some_and(|toplevel| toplevel.initial_sent)
        {
            self.send_configure();
        }
    }

    fn send_configure(&mut self) {
        let size = self.plane.size.unwrap_or((0, 0));
        let shown = self.plane.shown;
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        let Some(toplevel) = &mut self.toplevel else {
            return;
        };
        let version = toplevel.toplevel.version();
        let mut states = Vec::new();
        if version >= 2 {
            // Tiled on every side: mpv then takes the size as given rather than its own.
            states.extend([
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
            ]);
        }
        if shown {
            states.push(xdg_toplevel::State::Activated);
        } else if version >= 6 {
            states.push(xdg_toplevel::State::Suspended);
        }
        let bytes: Vec<u8> = states
            .into_iter()
            .flat_map(|state| u32::from(state).to_ne_bytes())
            .collect();
        toplevel.toplevel.configure(size.0, size.1, bytes);
        toplevel.xdg_surface.configure(serial);
        if toplevel.sent.len() >= MOST_UNACKNOWLEDGED {
            toplevel.sent.pop_front();
        }
        toplevel.sent.push_back(serial);
        toplevel.in_flight = Some(serial);
        toplevel.acknowledged = false;
        toplevel.sent_size = size;
    }
}

impl GlobalDispatch<XdgWmBase, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<XdgWmBase>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        if state.admit_object(client) {
            data_init.init(resource, ());
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_wm_base::Request::CreatePositioner { id } => {
                if state.admit_object(client) {
                    data_init.init(id, ());
                }
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                let surface_id = surface.id();
                let Some(record) = state.surfaces.get(&surface_id) else {
                    return;
                };
                if record.xdg_surface.is_some() || record.toplevel {
                    resource.post_error(xdg_wm_base::Error::Role, "the surface has a role already");
                    return;
                }
                if record.has_buffer() {
                    resource.post_error(
                        xdg_wm_base::Error::InvalidSurfaceState,
                        "the surface has a buffer already",
                    );
                    return;
                }
                if !state.admit_object(client) {
                    return;
                }
                let xdg_surface = data_init.init(id, surface_id.clone());
                if let Some(record) = state.surfaces.get_mut(&surface_id) {
                    record.xdg_surface = Some(xdg_surface);
                }
            }
            // The relay never pings; destroying the base while surfaces live is mpv's own end.
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &XdgWmBase, _: &()) {
        state.object_gone();
    }
}

/// Positioners are kept and never used: the relay offers no popups.
impl Dispatch<XdgPositioner, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &XdgPositioner,
        _: xdg_positioner::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &XdgPositioner, _: &()) {
        state.object_gone();
    }
}

impl Dispatch<XdgSurface, ObjectId> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &XdgSurface,
        request: xdg_surface::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_surface::Request::GetToplevel { id } => {
                if state.toplevel.is_some() {
                    resource.post_error(
                        xdg_surface::Error::AlreadyConstructed,
                        "the relay shows one toplevel, and has one already",
                    );
                    return;
                }
                if !state.admit_object(client) {
                    return;
                }
                let toplevel = data_init.custom_init(
                    id,
                    Arc::new(ToplevelData {
                        surface: surface.clone(),
                    }),
                );
                if toplevel.version() >= 5 {
                    toplevel.wm_capabilities(Vec::new());
                }
                if let Some(record) = state.surfaces.get_mut(surface) {
                    record.toplevel = true;
                }
                state.toplevel = Some(Toplevel {
                    surface: surface.clone(),
                    xdg_surface: resource.clone(),
                    toplevel,
                    sent: VecDeque::new(),
                    configured: false,
                    initial_sent: false,
                    in_flight: None,
                    acknowledged: false,
                    sent_size: (0, 0),
                });
                state.plane.surface = Some(surface.clone());
                state.plane_assigned();
            }
            xdg_surface::Request::GetPopup { .. } => {
                resource.post_error(
                    xdg_surface::Error::NotConstructed,
                    "the relay offers no popups",
                );
            }
            xdg_surface::Request::SetWindowGeometry { width, height, .. } => {
                if width <= 0 || height <= 0 {
                    resource.post_error(
                        xdg_surface::Error::InvalidSize,
                        format!("set_window_geometry: invalid size {width}x{height}"),
                    );
                }
            }
            xdg_surface::Request::AckConfigure { serial } => state.acknowledge(resource, serial),
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &XdgSurface, surface: &ObjectId) {
        state.object_gone();
        if let Some(record) = state.surfaces.get_mut(surface)
            && record.xdg_surface.as_ref() == Some(resource)
        {
            record.xdg_surface = None;
        }
    }
}

impl State {
    /// mpv acknowledged the configure `serial`: one the relay sent, and not older than one
    /// acknowledged before.
    fn acknowledge(&mut self, resource: &XdgSurface, serial: u32) {
        let known = self
            .toplevel
            .as_mut()
            .filter(|toplevel| toplevel.xdg_surface == *resource);
        let Some(toplevel) = known else {
            resource.post_error(xdg_surface::Error::InvalidSerial, "no configure was sent");
            return;
        };
        let Some(index) = toplevel.sent.iter().position(|sent| *sent == serial) else {
            resource.post_error(
                xdg_surface::Error::InvalidSerial,
                format!(
                    "ack_configure: serial {serial} was not sent, or is older than one acknowledged"
                ),
            );
            return;
        };
        toplevel.sent.drain(..=index);
        toplevel.configured = true;
        if toplevel.in_flight == Some(serial) {
            toplevel.acknowledged = true;
        }
    }
}

/// mpv's toplevel, read raw: what wayland-server's generated parser would do, without its panic
/// on a string left out.
pub(super) struct ToplevelData {
    /// mpv's surface with the toplevel role.
    surface: ObjectId,
}

/// `xdg_toplevel`'s requests the relay reads.
const SET_MAX_SIZE: u16 = 7;
const SET_MIN_SIZE: u16 = 8;

impl ObjectData<State> for ToplevelData {
    fn request(
        self: Arc<Self>,
        handle: &Handle,
        _: &mut State,
        client: ClientId,
        message: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<State>>> {
        // mpv's title, its window's states and its own size limits have no meaning for a plane,
        // which the interface sizes: only what the protocol forbids is refused.
        let left_out = |arg: &Argument<ObjectId, OwnedFd>| matches!(arg, Argument::Str(None));
        if message.args.iter().any(left_out) {
            // As wayland-server ends a client whose request it cannot parse.
            handle.kill_client(
                client,
                DisconnectReason::ProtocolError(ProtocolError {
                    // wl_display's invalid_method.
                    code: 1,
                    object_id: 1,
                    object_interface: "wl_display".to_owned(),
                    message: "a string of xdg_toplevel's was left out".to_owned(),
                }),
            );
        } else if let (SET_MAX_SIZE | SET_MIN_SIZE, [Argument::Int(width), Argument::Int(height)]) =
            (message.opcode, &message.args[..])
            && (*width < 0 || *height < 0)
        {
            let reason = format!("a size limit of {width}x{height} is negative");
            handle.post_error(
                message.sender_id,
                xdg_toplevel::Error::InvalidSize.into(),
                CString::new(reason).unwrap_or_default(),
            );
        }
        // No request of a toplevel's makes an object.
        None
    }

    fn destroyed(self: Arc<Self>, _: &Handle, state: &mut State, _: ClientId, object: ObjectId) {
        state.object_gone();
        if state
            .toplevel
            .as_ref()
            .is_some_and(|toplevel| toplevel.toplevel.id() == object)
        {
            state.toplevel = None;
            if let Some(record) = state.surfaces.get_mut(&self.surface) {
                record.toplevel = false;
            }
            state.plane_lost();
        }
    }
}
