// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! mpv's surfaces, and how the one with the toplevel role reaches the plane's surface.
//!
//! **mpv's state is held here.** Each request of mpv's on a surface changes its pending state in
//! the relay only. At mpv's commit the relay checks the whole new state, keeps it, and issues it
//! on the plane's surface in one go, ending with a commit of its own. A change of the interface's
//! (a place, a show, a hide), made from another thread under the same lock, therefore never
//! commits half of mpv's state.
//!
//! **In step with the interface.** The plane's surface runs desynchronized: mpv's frames show as
//! soon as mpv commits them, without the interface drawing. A change of the interface's that
//! touches the plane's own state while it is shown (its size, showing, hiding) switches it to
//! synchronized first, so that the change waits for the interface's next commit of its parent
//! and shows in the same frame as the interface's own. A frame callback the relay asks with the
//! change tells when the compositor drew it; the surface then runs desynchronized again. Until
//! then, mpv's frames wait for the interface's commits too, which an interface that is moving the
//! plane makes at every frame.
//!
//! **Opaque.** mpv's buffers carry an alpha channel: libplacebo asks the Vulkan driver for
//! premultiplied alpha, which Mesa offers on Wayland, so Mesa picks formats with alpha
//! (`ARGB2101010` and the like). mpv says its picture is opaque through its surface's opaque region
//! instead. The relay passes it on: without it, the compositor must draw whatever lies under the
//! plane, and can never show mpv's buffer straight on the screen.
//!
//! **Buffers.** A buffer of mpv's is released to mpv when no surface of mpv's holds it any more
//! and the compositor has released it too. The buffer on screen when the plane is hidden is kept
//! that way, and shown again when the plane is shown, without mpv drawing again.

use std::mem;

use wayland_client::protocol::wl_buffer::WlBuffer as HostBuffer;
use wayland_client::protocol::wl_output::{Transform as HostTransform, WlOutput as HostOutput};
use wayland_client::{Proxy, WEnum};
use wayland_protocols::wp::content_type::v1::client::wp_content_type_v1::Type as HostContentType;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 as HostParams;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1 as MpvDmabufFeedback;
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback;
use wayland_protocols::wp::viewporter::server::wp_viewport::{self, WpViewport};
use wayland_protocols::xdg::shell::server::xdg_surface::XdgSurface;
use wayland_server::backend::ObjectId;
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, New, Resource};

use super::check::{self, CommitState, Source};
use super::host::{FeedbackEvent, FeedbackKind, HostCallback, replay_feedback};
use super::{Buffer, Made, State};

/// One of mpv's surfaces.
pub(super) struct Surface {
    resource: WlSurface,
    pending: Pending,
    /// The state mpv last committed.
    buffer: Option<ObjectId>,
    /// The size of the buffer mpv last committed, kept when mpv destroys it, as a compositor keeps
    /// it: every later commit is checked against it ([`CommitState::buffer`]).
    buffer_size: (i32, i32),
    scale: i32,
    transform: u32,
    source: Option<Source>,
    destination: Option<(i32, i32)>,
    content_type: u32,
    /// The opaque region mpv last committed, `None` for none.
    opaque_region: Option<Region>,
    /// Whether that region covers the whole surface, as last committed.
    opaque: bool,
    /// What mpv committed and the window's connection has not received yet.
    callbacks: Vec<WlCallback>,
    feedbacks: Vec<WpPresentationFeedback>,
    /// Whether the committed buffer is to be attached again on the plane's surface.
    attach_owed: bool,
    /// The objects mpv made for this surface; one of each at most.
    pub(super) viewport: Option<WpViewport>,
    pub(super) has_fractional_scale: bool,
    pub(super) has_content_type: bool,
    pub(super) has_fifo: bool,
    /// What mpv's fifo asked with the last commit, until it is issued on the plane: a barrier
    /// set, and a wait for the barrier before.
    fifo_barrier: bool,
    fifo_wait: bool,
    /// The shell role mpv gave it: the relay plays `xdg_wm_base`, and its one toplevel is the
    /// plane's.
    pub(super) xdg_surface: Option<XdgSurface>,
    pub(super) toplevel: bool,
}

/// A surface's state as mpv's requests set it, until its commit. `None` leaves the committed
/// value as it is.
#[derive(Default)]
struct Pending {
    buffer: Option<Option<ObjectId>>,
    callbacks: Vec<WlCallback>,
    feedbacks: Vec<WpPresentationFeedback>,
    scale: Option<i32>,
    transform: Option<u32>,
    source: Option<Option<Source>>,
    destination: Option<Option<(i32, i32)>>,
    content_type: Option<u32>,
    opaque_region: Option<Option<Region>>,
    fifo_barrier: bool,
    fifo_wait: bool,
}

impl Surface {
    pub(super) fn new(resource: WlSurface) -> Self {
        Self {
            resource,
            pending: Pending::default(),
            buffer: None,
            buffer_size: (0, 0),
            scale: 1,
            transform: 0,
            source: None,
            destination: None,
            content_type: 0,
            opaque_region: None,
            opaque: false,
            callbacks: Vec::new(),
            feedbacks: Vec::new(),
            attach_owed: false,
            viewport: None,
            has_fractional_scale: false,
            has_content_type: false,
            has_fifo: false,
            fifo_barrier: false,
            fifo_wait: false,
            xdg_surface: None,
            toplevel: false,
        }
    }

    /// Whether mpv attached a buffer, now or before.
    pub(super) fn has_buffer(&self) -> bool {
        self.buffer.is_some() || matches!(self.pending.buffer, Some(Some(_)))
    }

    pub(super) fn add_feedback(&mut self, feedback: WpPresentationFeedback) {
        self.pending.feedbacks.push(feedback);
    }

    pub(super) fn set_source(&mut self, source: Option<Source>) {
        self.pending.source = Some(source);
    }

    pub(super) fn set_destination(&mut self, destination: Option<(i32, i32)>) {
        self.pending.destination = Some(destination);
    }

    pub(super) fn set_content_type(&mut self, content_type: u32) {
        self.pending.content_type = Some(content_type);
    }

    pub(super) fn set_fifo_barrier(&mut self) {
        self.pending.fifo_barrier = true;
    }

    pub(super) fn wait_fifo_barrier(&mut self) {
        self.pending.fifo_wait = true;
    }
}

/// What the relay keeps of one of mpv's regions: enough to tell whether it covers a whole surface.
/// mpv makes its opaque region of one rectangle, its whole surface. A region made otherwise is
/// `Other` and counts as covering nothing: the plane then says nothing is opaque, which costs the
/// compositor some drawing and never shows a wrong picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Region {
    Empty,
    Rectangle {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Other,
}

impl Region {
    /// The region with a rectangle added. An empty rectangle adds nothing.
    pub(super) fn add(self, x: i32, y: i32, width: i32, height: i32) -> Self {
        if width <= 0 || height <= 0 {
            return self;
        }
        match self {
            Region::Empty => Region::Rectangle {
                x,
                y,
                width,
                height,
            },
            Region::Rectangle { .. } | Region::Other => Region::Other,
        }
    }

    /// The region with a rectangle of `width` by `height` taken away, wherever it lies.
    pub(super) fn subtract(self, width: i32, height: i32) -> Self {
        if self == Region::Empty || width <= 0 || height <= 0 {
            return self;
        }
        Region::Other
    }

    /// Whether the region holds the whole of a surface of `size`.
    pub(super) fn covers(self, (surface_width, surface_height): (i32, i32)) -> bool {
        let Region::Rectangle {
            x,
            y,
            width,
            height,
        } = self
        else {
            return false;
        };
        surface_width > 0
            && surface_height > 0
            && x <= 0
            && y <= 0
            && i64::from(x) + i64::from(width) >= i64::from(surface_width)
            && i64::from(y) + i64::from(height) >= i64::from(surface_height)
    }
}

/// The plane: where it is, whether it shows, and what the plane's surface holds now.
pub(super) struct PlaneState {
    /// Once closed, nothing more is issued.
    pub(super) closed: bool,
    /// mpv's surface with the toplevel role, shown on the plane's surface.
    pub(super) surface: Option<ObjectId>,
    pub(super) shown: bool,
    position: Option<(i32, i32)>,
    /// The size the interface placed the plane at; the viewport's destination.
    pub(super) size: Option<(i32, i32)>,
    /// Whether the plane's surface is synchronized with its parent.
    sync: bool,
    /// Whether the plane's surface holds a buffer, at least once the parent commits.
    host_mapped: bool,
    issued: Issued,
    /// The number of the last change of the interface's that waits for its commit.
    change: u64,
    /// Whether mpv's surface is yet to be told the outputs the plane is on, and whether it
    /// committed since it became the plane's ([`State::plane_assigned`]).
    enters_held: bool,
    committed_since_assigned: bool,
}

/// What the relay last issued on the plane's surface: the state the compositor holds for it.
struct Issued {
    /// The size of the buffer last attached, `(0, 0)` for none, which the compositor keeps after
    /// the relay destroys that buffer.
    buffer: (i32, i32),
    scale: i32,
    transform: u32,
    source: Option<Source>,
    destination: Option<(i32, i32)>,
    content_type: u32,
    /// Whether the plane's whole surface is said to be opaque.
    opaque: bool,
}

impl Default for PlaneState {
    fn default() -> Self {
        Self {
            closed: false,
            surface: None,
            shown: false,
            position: None,
            size: None,
            sync: false,
            host_mapped: false,
            // A new surface's state.
            issued: Issued {
                buffer: (0, 0),
                scale: 1,
                transform: 0,
                source: None,
                destination: None,
                content_type: 0,
                opaque: false,
            },
            change: 0,
            enters_held: false,
            committed_since_assigned: false,
        }
    }
}

/// Who issues the plane's state: mpv's commit shows as soon as it can; the interface's change
/// waits for the interface's commit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Engine,
    Interface,
}

impl Dispatch<WlSurface, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let id = resource.id();
        match request {
            wl_surface::Request::Attach { buffer, x, y } => {
                if resource.version() >= 5 && (x != 0 || y != 0) {
                    resource.post_error(
                        wl_surface::Error::InvalidOffset,
                        "wl_surface.attach: the offset must be zero from version 5",
                    );
                    return;
                }
                // Offsets of older versions move the content within the surface: the plane is
                // placed by the interface, so they are not passed on.
                if let Some(surface) = state.surfaces.get_mut(&id) {
                    surface.pending.buffer = Some(buffer.map(|buffer| buffer.id()));
                }
            }
            wl_surface::Request::Frame { callback } => {
                if !state.admit_object(client) {
                    return;
                }
                let callback = data_init.init(callback, ());
                if let Some(surface) = state.surfaces.get_mut(&id) {
                    surface.pending.callbacks.push(callback);
                }
            }
            wl_surface::Request::SetBufferScale { scale } => {
                match check::buffer_scale(scale, wl_surface::Error::InvalidScale.into()) {
                    Ok(scale) => {
                        if let Some(surface) = state.surfaces.get_mut(&id) {
                            surface.pending.scale = Some(scale);
                        }
                    }
                    Err(refusal) => resource.post_error(refusal.code, refusal.message),
                }
            }
            wl_surface::Request::SetBufferTransform { transform } => match transform {
                WEnum::Value(transform) => {
                    if let Some(surface) = state.surfaces.get_mut(&id) {
                        surface.pending.transform = Some(transform.into());
                    }
                }
                WEnum::Unknown(value) => resource.post_error(
                    wl_surface::Error::InvalidTransform,
                    format!("wl_surface.set_buffer_transform: unknown transform {value}"),
                ),
            },
            wl_surface::Request::SetOpaqueRegion { region } => {
                // The region is read now: mpv may change or destroy it before its commit.
                let region = region.map(|region| {
                    state
                        .regions
                        .get(&region.id())
                        .copied()
                        .unwrap_or(Region::Other)
                });
                if let Some(surface) = state.surfaces.get_mut(&id) {
                    surface.pending.opaque_region = Some(region);
                }
            }
            wl_surface::Request::Commit => state.commit(&id),
            // Damage is not passed on: a new buffer is damaged whole on the plane's surface.
            // The plane's input region is the relay's, always empty. The offset of version 5 is
            // the interface's to set.
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: wayland_server::backend::ClientId,
        resource: &WlSurface,
        _: &(),
    ) {
        state.object_gone();
        state.surface_destroyed(&resource.id());
    }
}

impl Dispatch<WlCallback, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlCallback,
        _: <WlCallback as Resource>::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _: wayland_server::backend::ClientId, _: &WlCallback, _: &()) {
        state.object_gone();
    }
}

impl State {
    /// mpv's commit of a surface: checks the new state whole, keeps it, and issues it on the plane's
    /// surface when it is the plane's.
    fn commit(&mut self, id: &ObjectId) {
        let Some(surface) = self.surfaces.get(id) else {
            return;
        };
        let pending = &surface.pending;
        // A buffer attached now gives its size, none if mpv destroyed it before this commit, as a
        // compositor forgets a buffer destroyed before it was committed; else the last one's stays.
        let buffer_size = match &pending.buffer {
            Some(attached) => self.buffer_size(attached.as_ref()),
            None => surface.buffer_size,
        };
        let transform = pending.transform.unwrap_or(surface.transform);
        let checked = CommitState {
            buffer: buffer_size,
            scale: pending.scale.unwrap_or(surface.scale),
            transposed: transposed(transform),
            source: pending.source.unwrap_or(surface.source),
            destination: pending.destination.unwrap_or(surface.destination),
        };
        if let Err(refusal) = check_commit(checked) {
            self.refuse_commit(id, refusal);
            return;
        }
        let attaches = matches!(pending.buffer, Some(Some(_)));
        // As wlroots: a shell surface holding a buffer before its first configure was acknowledged.
        let holds_buffer = buffer_size.0 > 0 && buffer_size.1 > 0;
        if surface.xdg_surface.is_some() && holds_buffer && !self.configured(id) {
            self.refuse_unconfigured_buffer(id);
            return;
        }
        let is_plane = self.plane.surface.as_ref() == Some(id);
        let Some(surface) = self.surfaces.get_mut(id) else {
            return;
        };
        let pending = mem::take(&mut surface.pending);
        surface.buffer_size = buffer_size;
        let mut released = None;
        if let Some(new) = pending.buffer {
            let old = mem::replace(&mut surface.buffer, new.clone());
            surface.attach_owed = true;
            if old != new {
                if let Some(old) = old {
                    if let Some(buffer) = self.buffers.get_mut(&old) {
                        buffer.attached = buffer.attached.saturating_sub(1);
                    }
                    released = Some(old);
                }
                if let Some(buffer) = new.as_ref().and_then(|new| self.buffers.get_mut(new)) {
                    buffer.attached += 1;
                }
            }
            if let Some(buffer) = new.as_ref().and_then(|new| self.buffers.get_mut(new)) {
                buffer.owed = true;
            }
        }
        if let Some(scale) = pending.scale {
            surface.scale = scale;
        }
        if let Some(transform) = pending.transform {
            surface.transform = transform;
        }
        if let Some(source) = pending.source {
            surface.source = source;
        }
        if let Some(destination) = pending.destination {
            surface.destination = destination;
        }
        if let Some(content_type) = pending.content_type {
            surface.content_type = content_type;
        }
        if let Some(region) = pending.opaque_region {
            surface.opaque_region = region;
        }
        // Fifo requests belong to this commit only.
        surface.fifo_barrier = pending.fifo_barrier;
        surface.fifo_wait = pending.fifo_wait;
        surface.opaque = surface
            .opaque_region
            .is_some_and(|region| region.covers(checked.surface_size()));
        surface.callbacks.extend(pending.callbacks);
        // Feedback of a state that was never shown, replaced by this one, is discarded, as a
        // compositor discards it.
        let superseded = mem::replace(&mut surface.feedbacks, pending.feedbacks);
        for feedback in superseded {
            feedback.discarded();
        }
        if let Some(old) = released {
            self.maybe_release(&old);
        }
        if is_plane {
            self.plane.committed_since_assigned = true;
            self.release_enters();
            if attaches {
                self.presentation.committed += 1;
            }
            self.plane_committed();
            self.issue_plane(Origin::Engine);
        }
    }

    /// The size of one of mpv's buffers, `(0, 0)` for none or one mpv destroyed.
    fn buffer_size(&self, buffer: Option<&ObjectId>) -> (i32, i32) {
        buffer
            .and_then(|buffer| self.buffers.get(buffer))
            .map_or((0, 0), |buffer| buffer.size)
    }

    /// Ends mpv's connection for a commit the compositor would refuse, with the error the
    /// compositor would raise, on the object it would raise it on.
    fn refuse_commit(&self, id: &ObjectId, refusal: check::Refusal) {
        let Some(surface) = self.surfaces.get(id) else {
            return;
        };
        if refusal.code == u32::from(wl_surface::Error::InvalidSize) {
            surface.resource.post_error(refusal.code, refusal.message);
        } else if let Some(viewport) = &surface.viewport {
            viewport.post_error(refusal.code, refusal.message);
        } else {
            // No viewport to raise it on any more: the surface takes it, so that mpv is ended all
            // the same.
            surface.resource.post_error(refusal.code, refusal.message);
        }
    }

    /// Issues mpv's committed state on the plane's surface, if the plane shows, and ends with a
    /// commit. A buffer the compositor has not made yet (a dmabuf of `create_immed`) holds it
    /// back until it has.
    ///
    /// The state the plane's surface will hold is checked first, as the compositor will check it:
    /// mpv's state, the relay's viewport destination, and the size of the buffer the compositor
    /// holds, which may be one mpv has destroyed since. mpv's commit passed the same checks against
    /// its own surface; this one guards the window's connection whatever the relay merged.
    fn issue_plane(&mut self, origin: Origin) {
        if !self.plane.shown || self.plane.closed {
            return;
        }
        let Some(id) = self.plane.surface.clone() else {
            return;
        };
        let Some(surface) = self.surfaces.get(&id) else {
            return;
        };
        let attached = surface
            .buffer
            .as_ref()
            .and_then(|buffer| self.buffers.get(buffer));
        if attached.is_some_and(|buffer| buffer.host.is_none()) {
            return;
        }
        let host_buffer: Option<HostBuffer> = attached.and_then(|buffer| buffer.host.clone());
        let next = CommitState {
            buffer: if surface.attach_owed {
                attached.map_or((0, 0), |buffer| buffer.size)
            } else {
                self.plane.issued.buffer
            },
            scale: surface.scale,
            transposed: transposed(surface.transform),
            source: surface.source,
            destination: self.plane.size.or(surface.destination),
        };
        if let Err(refusal) = check_commit(next) {
            self.refuse_commit(&id, refusal);
            return;
        }
        self.sync_plane_for(origin, host_buffer.is_some());
        self.attach_owed_buffer(&id, host_buffer, next.buffer);
        self.issue_surface_state(&id);
        self.issue_fifo(&id);
        self.issue_frame_requests(&id);
        if origin == Origin::Interface && self.plane.sync {
            Self::ask_applied(&mut self.plane, &self.host, &self.qh);
        }
        self.host.surface.commit();
    }

    /// Switches the plane's surface to the mode the change needs: mpv's first frame shows without a
    /// commit of the interface's, which may never come; the interface's change to a shown frame
    /// waits for the interface's commit.
    fn sync_plane_for(&mut self, origin: Origin, has_buffer: bool) {
        let plane = &mut self.plane;
        if origin == Origin::Engine && plane.sync && !plane.host_mapped {
            self.host.subsurface.set_desync();
            plane.sync = false;
        }
        if origin == Origin::Interface && !plane.sync && has_buffer {
            self.host.subsurface.set_sync();
            plane.sync = true;
        }
    }

    /// Attaches the buffer of mpv's surface `id` on the plane's surface, if it is owed there.
    fn attach_owed_buffer(
        &mut self,
        id: &ObjectId,
        host_buffer: Option<HostBuffer>,
        size: (i32, i32),
    ) {
        let Some(surface) = self.surfaces.get_mut(id) else {
            return;
        };
        if !surface.attach_owed {
            return;
        }
        surface.attach_owed = false;
        let host = &self.host;
        host.surface.attach(host_buffer.as_ref(), 0, 0);
        if host_buffer.is_some() {
            host.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        }
        self.plane.host_mapped = host_buffer.is_some();
        self.plane.issued.buffer = size;
        if let Some(buffer) = surface
            .buffer
            .as_ref()
            .and_then(|buffer| self.buffers.get_mut(buffer))
        {
            buffer.host_busy = true;
        }
    }

    /// Issues what of mpv's surface `id` differs from what the plane's surface holds: its scale,
    /// transform, viewport, content type and whether it is opaque. The viewport's destination is
    /// the plane's size once placed, else mpv's own. An opaque plane's region is the largest
    /// rectangle: the compositor keeps the part within the surface, whatever size the plane takes
    /// later.
    fn issue_surface_state(&mut self, id: &ObjectId) {
        let Some(surface) = self.surfaces.get(id) else {
            return;
        };
        let host = &self.host;
        let issued = &mut self.plane.issued;
        if surface.scale != issued.scale {
            host.surface.set_buffer_scale(surface.scale);
            issued.scale = surface.scale;
        }
        if surface.transform != issued.transform {
            if let Ok(transform) = HostTransform::try_from(surface.transform) {
                host.surface.set_buffer_transform(transform);
            }
            issued.transform = surface.transform;
        }
        if surface.source != issued.source {
            match surface.source {
                Some(source) => {
                    host.viewport
                        .set_source(source.x, source.y, source.width, source.height);
                }
                None => host.viewport.set_source(-1.0, -1.0, -1.0, -1.0),
            }
            issued.source = surface.source;
        }
        let destination = self.plane.size.or(surface.destination);
        if destination != issued.destination {
            let (width, height) = destination.unwrap_or((-1, -1));
            host.viewport.set_destination(width, height);
            issued.destination = destination;
        }
        if surface.content_type != issued.content_type {
            if let (Some(content_type), Ok(value)) = (
                &host.content_type,
                HostContentType::try_from(surface.content_type),
            ) {
                content_type.set_content_type(value);
            }
            issued.content_type = surface.content_type;
        }
        if surface.opaque != issued.opaque {
            let region = surface.opaque.then(|| {
                let region = host.globals.compositor.create_region(&self.qh, ());
                region.add(0, 0, i32::MAX, i32::MAX);
                region
            });
            host.surface.set_opaque_region(region.as_ref());
            if let Some(region) = region {
                region.destroy();
            }
            issued.opaque = surface.opaque;
        }
    }

    /// Issues the fifo requests mpv made with its commit on the plane's own fifo, once. The
    /// compositor ignores them while the plane is synchronized, as the protocol says.
    fn issue_fifo(&mut self, id: &ObjectId) {
        let Some(surface) = self.surfaces.get_mut(id) else {
            return;
        };
        let (barrier, wait) = (
            mem::take(&mut surface.fifo_barrier),
            mem::take(&mut surface.fifo_wait),
        );
        let Some(fifo) = &self.host.fifo else {
            return;
        };
        if barrier {
            fifo.set_barrier();
        }
        if wait {
            fifo.wait_barrier();
        }
    }

    /// Asks the compositor for what mpv asked with its commit: frame callbacks, and presentation
    /// feedback, one for each frame shown whether mpv asked or not, as it is what the relay counts.
    fn issue_frame_requests(&mut self, id: &ObjectId) {
        let Some(surface) = self.surfaces.get_mut(id) else {
            return;
        };
        let host = &self.host;
        if !surface.callbacks.is_empty() {
            let callbacks = mem::take(&mut surface.callbacks);
            host.surface.frame(&self.qh, HostCallback::Frame(callbacks));
        }
        let feedbacks = mem::take(&mut surface.feedbacks);
        match &host.globals.presentation {
            Some(presentation) if self.plane.host_mapped || !feedbacks.is_empty() => {
                presentation.feedback(&host.surface, &self.qh, feedbacks);
            }
            _ => {
                for feedback in feedbacks {
                    feedback.discarded();
                }
            }
        }
    }

    /// Numbers a change of the interface's and asks the compositor to say when it drew it. Called
    /// before the commit that carries it.
    fn ask_applied(
        plane: &mut PlaneState,
        host: &super::Host,
        qh: &wayland_client::QueueHandle<State>,
    ) {
        plane.change += 1;
        host.surface.frame(qh, HostCallback::Applied(plane.change));
    }

    /// The compositor drew the interface's change numbered `change`: unless a later one waits,
    /// the plane's surface runs desynchronized again.
    pub(super) fn plane_applied(&mut self, change: u64) {
        if change == self.plane.change && self.plane.sync && !self.plane.closed {
            self.host.subsurface.set_desync();
            self.plane.sync = false;
        }
    }

    /// Places the plane: its top-left corner at `x`, `y` in the parent surface, its size `width` by
    /// `height`, both in the parent's logical coordinates. The position is the parent's state; the
    /// size waits for the parent's commit while the plane shows.
    pub(crate) fn place(&mut self, x: i32, y: i32, width: i32, height: i32) {
        if self.plane.position != Some((x, y)) {
            self.host.subsurface.set_position(x, y);
            self.plane.position = Some((x, y));
        }
        if self.plane.size == Some((width, height)) {
            return;
        }
        self.plane.size = Some((width, height));
        // No check is needed before this commit: setting a destination only lifts one of the
        // compositor's checks (`check::commit`), and changes nothing the others look at.
        if self.plane.host_mapped && !self.plane.sync {
            self.host.subsurface.set_sync();
            self.plane.sync = true;
        }
        self.host.viewport.set_destination(width, height);
        self.plane.issued.destination = Some((width, height));
        if self.plane.sync {
            Self::ask_applied(&mut self.plane, &self.host, &self.qh);
        }
        self.host.surface.commit();
        self.configure_size();
    }

    /// Shows the plane: mpv is told it is no longer suspended, and its last frame shows at the
    /// interface's next commit; without one, mpv's next frame shows as soon as mpv draws it.
    pub(crate) fn show(&mut self) {
        if self.plane.shown {
            return;
        }
        self.plane.shown = true;
        self.configure_state();
        let has_buffer = self
            .plane
            .surface
            .as_ref()
            .and_then(|id| self.surfaces.get_mut(id))
            .filter(|surface| surface.buffer.is_some())
            .map(|surface| surface.attach_owed = true)
            .is_some();
        if has_buffer {
            self.issue_plane(Origin::Interface);
        } else if self.plane.sync {
            self.host.subsurface.set_desync();
            self.plane.sync = false;
        }
    }

    /// Hides the plane: mpv is told it is suspended, and stops drawing; the plane's surface is
    /// unmapped at the interface's next commit. mpv's last buffer is kept for the next show.
    pub(crate) fn hide(&mut self) {
        if !self.plane.shown {
            return;
        }
        self.plane.shown = false;
        self.configure_state();
        if !self.plane.host_mapped {
            return;
        }
        if !self.plane.sync {
            self.host.subsurface.set_sync();
            self.plane.sync = true;
        }
        // No buffer passes every check of the compositor's.
        self.host.surface.attach(None, 0, 0);
        self.host.surface.commit();
        self.plane.host_mapped = false;
        self.plane.issued.buffer = (0, 0);
        if let Some(surface) = self
            .plane
            .surface
            .as_ref()
            .and_then(|id| self.surfaces.get_mut(id))
        {
            surface.attach_owed = true;
        }
    }

    /// The plane's surface shows nothing of mpv's any more: mpv destroyed its toplevel or its
    /// surface. Unmapped at once: there is nothing to keep in step.
    pub(super) fn plane_lost(&mut self) {
        self.plane.surface = None;
        if self.plane.closed {
            return;
        }
        if self.plane.sync {
            self.host.subsurface.set_desync();
            self.plane.sync = false;
        }
        if self.plane.host_mapped {
            self.host.surface.attach(None, 0, 0);
            self.host.surface.commit();
            self.plane.host_mapped = false;
            self.plane.issued.buffer = (0, 0);
        }
    }

    /// mpv destroyed a surface: its buffer is let go, its pending feedback discarded.
    fn surface_destroyed(&mut self, id: &ObjectId) {
        let Some(surface) = self.surfaces.remove(id) else {
            return;
        };
        for feedback in surface
            .feedbacks
            .into_iter()
            .chain(surface.pending.feedbacks)
        {
            feedback.discarded();
        }
        if let Some(buffer) = surface.buffer {
            if let Some(record) = self.buffers.get_mut(&buffer) {
                record.attached = record.attached.saturating_sub(1);
            }
            self.maybe_release(&buffer);
        }
        if self.plane.surface.as_ref() == Some(id) {
            self.plane_lost();
        }
    }

    /// mpv's surface became the plane's: it is told the scales the plane's surface has, and the
    /// outputs it is on once mpv committed it and was told its fractional scale, as a compositor
    /// tells a surface its outputs once it holds content. In that order: mpv 0.42 rescales its
    /// window by its old scale over the new when its surface enters an output, and told the
    /// output first, divides by a scale of zero and asks for a viewport of 0x0. mpv makes its
    /// toplevel before its fractional scale object, and the compositor may send the plane's scale
    /// late (KWin does).
    pub(super) fn plane_assigned(&mut self) {
        self.plane.enters_held = true;
        self.plane.committed_since_assigned = false;
        self.send_preferred_buffer_scale();
        self.send_preferred_buffer_transform();
        self.send_preferred_scale();
    }

    /// Tells mpv's plane surface the outputs the plane is on, if they were held and nothing holds
    /// them any more.
    fn release_enters(&mut self) {
        if !self.plane.enters_held
            || !self.plane.committed_since_assigned
            || self.fractional_scale_owed()
        {
            return;
        }
        self.plane.enters_held = false;
        for output in self.host.entered.clone() {
            self.plane_entered(&output);
        }
    }

    /// The plane's surface entered `output`: so does mpv's.
    pub(super) fn plane_entered(&mut self, output: &HostOutput) {
        if self.plane.enters_held {
            return;
        }
        for (surface, mpv_output) in self.plane_and_mpv_outputs(output) {
            surface.enter(&mpv_output);
        }
    }

    /// The plane's surface left `output`: so does mpv's, unless it was not told it entered.
    pub(super) fn plane_left(&self, output: &HostOutput) {
        if self.plane.enters_held {
            return;
        }
        for (surface, mpv_output) in self.plane_and_mpv_outputs(output) {
            surface.leave(&mpv_output);
        }
    }

    /// mpv's plane surface, with each of mpv's objects for the compositor's `output`.
    fn plane_and_mpv_outputs(
        &self,
        output: &HostOutput,
    ) -> Vec<(WlSurface, wayland_server::protocol::wl_output::WlOutput)> {
        let Some(surface) = self.plane_surface() else {
            return Vec::new();
        };
        self.mpv_outputs(output)
            .into_iter()
            .filter(|mpv_output| mpv_output.client() == surface.client())
            .map(|mpv_output| (surface.clone(), mpv_output))
            .collect()
    }

    /// Tells mpv's plane surface the buffer scale the compositor prefers for the plane's.
    pub(super) fn send_preferred_buffer_scale(&self) {
        if let (Some(surface), Some(scale)) = (
            self.plane_surface_since(6),
            self.host.preferred_buffer_scale,
        ) {
            surface.preferred_buffer_scale(scale);
        }
    }

    /// Tells mpv's plane surface the buffer transform the compositor prefers for the plane's.
    pub(super) fn send_preferred_buffer_transform(&self) {
        let transform = self.host.preferred_buffer_transform.and_then(|transform| {
            wayland_server::protocol::wl_output::Transform::try_from(transform).ok()
        });
        if let (Some(surface), Some(transform)) = (self.plane_surface_since(6), transform) {
            surface.preferred_buffer_transform(transform);
        }
    }

    /// mpv's plane surface, if its version has the events of `version`.
    fn plane_surface_since(&self, version: u32) -> Option<&WlSurface> {
        self.plane_surface()
            .filter(|surface| surface.version() >= version)
    }

    /// Tells mpv's fractional scale objects for its plane surface the scale the compositor prefers.
    pub(super) fn send_preferred_scale(&mut self) {
        let (Some(plane), Some(scale)) = (&self.plane.surface, self.host.preferred_scale) else {
            return;
        };
        for (object, surface) in &self.fractional_scales {
            if surface == plane {
                object.preferred_scale(scale);
            }
        }
        self.release_enters();
    }

    /// Whether mpv's plane surface has a fractional scale object that has not been told a scale
    /// yet: the compositor has not sent the plane's.
    fn fractional_scale_owed(&self) -> bool {
        let Some(plane) = &self.plane.surface else {
            return false;
        };
        self.host.preferred_scale.is_none()
            && self
                .fractional_scales
                .iter()
                .any(|(_, surface)| surface == plane)
    }

    fn plane_surface(&self) -> Option<&WlSurface> {
        let id = self.plane.surface.as_ref()?;
        self.surfaces.get(id).map(|surface| &surface.resource)
    }

    /// The compositor released a buffer.
    pub(super) fn host_released(&mut self, host: &wayland_client::backend::ObjectId) {
        let Some(id) = self.host_buffers.get(host).cloned() else {
            return;
        };
        if let Some(buffer) = self.buffers.get_mut(&id) {
            buffer.host_busy = false;
        }
        self.maybe_release(&id);
    }

    /// Releases a buffer to mpv once neither a surface of mpv's nor the compositor holds it.
    fn maybe_release(&mut self, id: &ObjectId) {
        let Some(buffer) = self.buffers.get_mut(id) else {
            return;
        };
        if buffer.attached == 0 && !buffer.host_busy && buffer.owed {
            buffer.owed = false;
            if let Ok(resource) = WlBuffer::from_id(&self.display, id.clone()) {
                resource.release();
            }
        }
    }

    /// mpv destroyed a buffer: so does the relay, on the window's connection. A surface that
    /// still holds it shows nothing at its next issue.
    pub(super) fn buffer_destroyed(&mut self, id: &ObjectId) {
        if let Some(buffer) = self.buffers.remove(id)
            && let Some(host) = buffer.host
        {
            self.host_buffers.remove(&host.id());
            host.destroy();
        }
    }

    /// The compositor made a dmabuf the relay asked for.
    pub(super) fn dmabuf_created(&mut self, params: &HostParams, host: HostBuffer) {
        let Some(answered) = self.host_params.remove(&params.id()) else {
            host.destroy();
            return;
        };
        answered.host.destroy();
        match answered.made {
            Made::Later { params, size } => {
                let made = params.client().and_then(|client| {
                    if !self.admit_object(&client) {
                        return None;
                    }
                    client
                        .create_resource::<WlBuffer, (), State>(&self.display, 1, ())
                        .ok()
                });
                let Some(buffer) = made.filter(|_| params.is_alive()) else {
                    host.destroy();
                    return;
                };
                params.created(&buffer);
                self.host_buffers.insert(host.id(), buffer.id());
                self.buffers
                    .insert(buffer.id(), Buffer::new(Some(host), size));
            }
            Made::Already(id) => {
                let Some(buffer) = self.buffers.get_mut(&id) else {
                    host.destroy();
                    return;
                };
                self.host_buffers.insert(host.id(), id.clone());
                buffer.host = Some(host);
                // A commit of the plane's may have waited for it.
                let waited = self
                    .plane
                    .surface
                    .as_ref()
                    .and_then(|plane| self.surfaces.get(plane))
                    .is_some_and(|surface| surface.buffer.as_ref() == Some(&id));
                if waited {
                    self.issue_plane(Origin::Engine);
                }
            }
        }
    }

    /// The compositor could not make a dmabuf: mpv's `create` fails as its own would; a buffer
    /// mpv holds already (`create_immed`) ends mpv's connection, as wlroots ends a client's.
    pub(super) fn dmabuf_failed(&mut self, params: &HostParams) {
        let Some(answered) = self.host_params.remove(&params.id()) else {
            return;
        };
        answered.host.destroy();
        match answered.made {
            Made::Later { params, .. } => {
                if params.is_alive() {
                    params.failed();
                }
            }
            Made::Already(id) => {
                if let Ok(buffer) = WlBuffer::from_id(&self.display, id)
                    && let Some(client) = buffer.client()
                {
                    self.end_mpv(
                        client.id(),
                        "the compositor could not import a dmabuf made with create_immed"
                            .to_owned(),
                    );
                }
            }
        }
    }

    /// Sends a batch of the compositor's dmabuf feedback to mpv's objects that follow it: the
    /// plane surface's to those for mpv's plane surface, the default one to all others.
    pub(super) fn forward_feedback(&self, kind: FeedbackKind, batch: &[FeedbackEvent]) {
        for (object, surface) in &self.feedbacks {
            let follows = match surface {
                Some(surface) if self.plane.surface.as_ref() == Some(surface) => {
                    FeedbackKind::Plane
                }
                _ => FeedbackKind::Default,
            };
            if follows == kind {
                replay_feedback(object, batch);
            }
        }
    }

    /// The feedback batch mpv gets when it asks for feedback on `surface`.
    pub(super) fn feedback_for(&self, surface: Option<&ObjectId>) -> Option<&Vec<FeedbackEvent>> {
        let plane = surface.is_some() && surface == self.plane.surface.as_ref();
        let plane_batch = self.host.surface_feedback.complete.as_ref();
        let default_batch = self.host.default_feedback.complete.as_ref();
        if plane {
            plane_batch.or(default_batch)
        } else {
            default_batch
        }
    }
}

/// Whether a buffer transform turns the buffer by 90 or 270 degrees, flipped or not: the odd ones.
fn transposed(transform: u32) -> bool {
    transform % 2 == 1
}

/// [`check::commit`], with the protocol's error codes.
fn check_commit(state: CommitState) -> Result<(), check::Refusal> {
    check::commit(
        state,
        wl_surface::Error::InvalidSize.into(),
        wp_viewport::Error::BadSize.into(),
        wp_viewport::Error::OutOfBuffer.into(),
    )
}

impl Buffer {
    pub(super) fn new(host: Option<HostBuffer>, size: (i32, i32)) -> Self {
        Self {
            host,
            size,
            attached: 0,
            host_busy: false,
            owed: false,
        }
    }
}

/// When and how the compositor presented a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Presented {
    /// The refresh counter; 0 when the compositor keeps none, as KWin 6.8.
    pub(super) msc: u64,
    /// When it showed, in nanoseconds on the compositor's presentation clock.
    pub(super) at_ns: u128,
    /// The display's refresh period then; 0 when it is variable or unknown.
    pub(super) refresh_ns: u32,
}

impl Presented {
    /// The refreshes from `last` to this frame: by the refresh counters when the compositor keeps
    /// one, else by the time between them over the refresh period, rounded. `None` when neither
    /// says, or when the counter or the time went back (an output changed).
    fn refreshes_since(&self, last: &Presented) -> Option<u64> {
        if self.msc != 0 && last.msc != 0 {
            return self.msc.checked_sub(last.msc).filter(|gap| *gap > 0);
        }
        if self.refresh_ns == 0 || self.at_ns <= last.at_ns {
            return None;
        }
        let refresh = u128::from(self.refresh_ns);
        let gap = (self.at_ns - last.at_ns + refresh / 2) / refresh;
        u64::try_from(gap).ok().filter(|gap| *gap > 0)
    }
}

impl State {
    /// Counts a frame the compositor presented.
    pub(super) fn count_presented(&mut self, presented: Presented, zero_copy: bool) {
        let counts = &mut self.presentation;
        counts.presented += 1;
        if zero_copy {
            counts.zero_copy += 1;
        }
        if let Some(gap) = self
            .last_presented
            .and_then(|last| presented.refreshes_since(&last))
        {
            match usize::try_from(gap)
                .ok()
                .and_then(|gap| counts.gaps.get_mut(gap))
            {
                Some(count) => *count += 1,
                None => counts.longer_gaps += 1,
            }
        }
        self.last_presented = Some(presented);
    }
}

/// A dmabuf feedback object made by mpv on `surface`, or with no surface for the default one.
pub(super) fn new_feedback(
    state: &mut State,
    client: &Client,
    data_init: &mut DataInit<'_, State>,
    id: New<MpvDmabufFeedback>,
    surface: Option<ObjectId>,
) {
    if !state.admit_object(client) {
        return;
    }
    let object = data_init.init(id, ());
    if let Some(batch) = state.feedback_for(surface.as_ref()) {
        replay_feedback(&object, batch);
    }
    state.feedbacks.push((object, surface));
}
