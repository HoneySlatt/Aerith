// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! mpv's side of the relay: the globals mpv is offered, and every request of mpv's but its
//! surfaces' ([`super::surface`]), its shell's ([`super::shell`]) and colour management's
//! ([`super::colour`]). Each request was counted at mpv's socket already ([`super::socket`]); here
//! each object it makes, and each file it hands over, is counted against the ceilings, the request
//! checked ([`super::check`]), and then either kept in the relay or issued again on the window's
//! connection.

use std::os::fd::AsFd;

use wayland_client::Proxy;
use wayland_client::protocol::wl_shm::Format as HostFormat;
use wayland_protocols::wp::color_management::v1::server::wp_color_manager_v1::WpColorManagerV1;
use wayland_protocols::wp::content_type::v1::server::wp_content_type_manager_v1::{
    self, WpContentTypeManagerV1,
};
use wayland_protocols::wp::content_type::v1::server::wp_content_type_v1::{self, WpContentTypeV1};
use wayland_protocols::wp::fifo::v1::server::wp_fifo_manager_v1::{self, WpFifoManagerV1};
use wayland_protocols::wp::fifo::v1::server::wp_fifo_v1::{self, WpFifoV1};
use wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_manager_v1::{
    self, WpFractionalScaleManagerV1,
};
use wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::Flags as HostFlags;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::{
    self, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::{
    self, ZwpLinuxDmabufV1,
};
use wayland_protocols::wp::presentation_time::server::wp_presentation::{self, WpPresentation};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback;
use wayland_protocols::wp::viewporter::server::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::server::wp_viewporter::{self, WpViewporter};
use wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase;
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::protocol::wl_buffer::{self, WlBuffer};
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::protocol::wl_region::{self, WlRegion};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use super::check::{self, ParamsErrors};
use super::colour::{self, Advertised, Offer};
use super::host::OutputInfo;
use super::surface::{Region, Surface, new_feedback};
use super::{Buffer, Made, Params, ParamsUse, Pool, State};

/// The versions mpv is offered at most: what the relay implements.
const COMPOSITOR: u32 = 6;
const SHM: u32 = 2;
const DMABUF: u32 = 4;
const PRESENTATION: u32 = 2;
const OUTPUT: u32 = 4;
pub(super) const XDG_WM_BASE: u32 = 6;

impl State {
    /// Offers mpv the globals, from what the window's compositor has.
    pub(super) fn offer_globals(&mut self) {
        let display = self.display.clone();
        let globals = &self.host.globals;
        display.create_global::<State, WlCompositor, ()>(
            globals.compositor.version().min(COMPOSITOR),
            (),
        );
        if globals.shm.is_some() {
            display.create_global::<State, WlShm, ()>(SHM, ());
        }
        if globals.dmabuf.is_some() {
            display.create_global::<State, ZwpLinuxDmabufV1, ()>(DMABUF, ());
        }
        display.create_global::<State, WpViewporter, ()>(1, ());
        if let Some(presentation) = &globals.presentation {
            display.create_global::<State, WpPresentation, ()>(
                presentation.version().min(PRESENTATION),
                (),
            );
        }
        if globals.fractional_scale.is_some() {
            display.create_global::<State, WpFractionalScaleManagerV1, ()>(1, ());
        }
        if globals.content_type.is_some() {
            display.create_global::<State, WpContentTypeManagerV1, ()>(1, ());
        }
        if globals.fifo.is_some() {
            display.create_global::<State, WpFifoManagerV1, ()>(1, ());
        }
        if let Some(host_colour) = globals
            .colour
            .as_ref()
            .filter(|host_colour| Offer::offered(&host_colour.advertised))
        {
            display.create_global::<State, WpColorManagerV1, Advertised>(
                host_colour.manager.version().min(colour::VERSION),
                host_colour.advertised,
            );
        }
        display.create_global::<State, XdgWmBase, ()>(XDG_WM_BASE, ());
        let complete: Vec<u32> = self
            .outputs
            .iter()
            .filter(|output| output.complete)
            .map(|output| output.name)
            .collect();
        for name in complete {
            self.publish_output(name);
        }
    }

    /// The compositor said all of an output: mpv is offered it, or told what changed.
    pub(super) fn publish_output(&mut self, name: u32) {
        let display = self.display.clone();
        let Some(output) = self.outputs.iter_mut().find(|output| output.name == name) else {
            return;
        };
        let description = output.info.description.clone();
        let description_changed = description != output.published_description;
        output.published_description = description;
        match output.global {
            None => {
                let version = output.host.version().min(OUTPUT);
                output.global = Some(display.create_global::<State, WlOutput, u32>(version, name));
            }
            Some(_) => {
                output.resources.retain(Resource::is_alive);
                let description = output
                    .info
                    .description
                    .as_ref()
                    .filter(|_| description_changed);
                for resource in &output.resources {
                    send_output_update(resource, &output.info, description);
                }
            }
        }
    }
}

/// Tells mpv's new object everything the compositor said of its output, then `done`.
fn send_output_at_bind(resource: &WlOutput, info: &OutputInfo) {
    send_output_state(resource, info);
    if resource.version() >= 4 {
        if let Some(name) = &info.name {
            resource.name(name.clone());
        }
        if let Some(description) = &info.description {
            resource.description(description.clone());
        }
    }
    if resource.version() >= 2 {
        resource.done();
    }
}

/// Tells mpv's object what changed of its output, then `done`: its state again, and its new
/// `description` if it changed. The name is sent once per object, at its bind.
fn send_output_update(resource: &WlOutput, info: &OutputInfo, description: Option<&String>) {
    send_output_state(resource, info);
    if let Some(description) = description.filter(|_| resource.version() >= 4) {
        resource.description(description.clone());
    }
    if resource.version() >= 2 {
        resource.done();
    }
}

/// An output's geometry, mode and scale, as the compositor said them last.
fn send_output_state(resource: &WlOutput, info: &OutputInfo) {
    if let Some(geometry) = &info.geometry {
        resource.geometry(
            geometry.x,
            geometry.y,
            geometry.physical_width,
            geometry.physical_height,
            wl_output::Subpixel::try_from(geometry.subpixel)
                .unwrap_or(wl_output::Subpixel::Unknown),
            geometry.make.clone(),
            geometry.model.clone(),
            wl_output::Transform::try_from(geometry.transform)
                .unwrap_or(wl_output::Transform::Normal),
        );
    }
    if let Some(mode) = info.mode {
        resource.mode(
            wl_output::Mode::from_bits_truncate(mode.flags),
            mode.width,
            mode.height,
            mode.refresh,
        );
    }
    if resource.version() >= 2 {
        resource.scale(info.scale.unwrap_or(1));
    }
}

/// Makes an object for mpv with `data`, against the ceiling of objects: `None` once mpv's
/// connection is ended for breaking it.
pub(super) fn admit<I: Resource + 'static, U: Send + Sync + 'static>(
    state: &mut State,
    client: &Client,
    resource: New<I>,
    data: U,
    data_init: &mut DataInit<'_, State>,
) -> Option<I>
where
    State: Dispatch<I, U>,
{
    if !state.admit_object(client) {
        return None;
    }
    Some(data_init.init(resource, data))
}

/// Counts the end of an object of mpv's whose death needs nothing else.
macro_rules! counted_destruction {
    ($interface:ty) => {
        fn destroyed(state: &mut Self, _: ClientId, _: &$interface, _: &()) {
            state.object_gone();
        }
    };
}

impl GlobalDispatch<WlCompositor, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WlCompositor>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        admit(state, client, resource, (), data_init);
    }
}

impl Dispatch<WlCompositor, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                if !state.admit_object(client) {
                    return;
                }
                let surface = data_init.init(id, ());
                state.surfaces.insert(surface.id(), Surface::new(surface));
            }
            wl_compositor::Request::CreateRegion { id } => {
                if !state.admit_object(client) {
                    return;
                }
                let region = data_init.init(id, ());
                state.regions.insert(region.id(), Region::Empty);
            }
            _ => {}
        }
    }

    counted_destruction!(WlCompositor);
}

/// mpv's regions are kept as far as the relay needs them: the plane's input region is always
/// empty, and its opaque region is the plane's whole surface when mpv's covers mpv's whole surface
/// ([`Region`]). Nothing of them is issued as mpv sent it.
impl Dispatch<WlRegion, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlRegion,
        request: wl_region::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(region) = state.regions.get_mut(&resource.id()) else {
            return;
        };
        *region = match request {
            wl_region::Request::Add {
                x,
                y,
                width,
                height,
            } => region.add(x, y, width, height),
            wl_region::Request::Subtract { width, height, .. } => region.subtract(width, height),
            _ => *region,
        };
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &WlRegion, _: &()) {
        state.object_gone();
        state.regions.remove(&resource.id());
    }
}

impl GlobalDispatch<WlShm, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WlShm>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let Some(shm) = admit(state, client, resource, (), data_init) else {
            return;
        };
        for format in &state.host.globals.shm_formats {
            match wl_shm::Format::try_from(*format) {
                Ok(format) => shm.format(format),
                Err(()) => continue,
            }
        }
    }
}

impl Dispatch<WlShm, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        shm: &WlShm,
        request: wl_shm::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let wl_shm::Request::CreatePool { id, fd, size } = request else {
            return;
        };
        state.took_fd();
        if let Err(refusal) = check::shm_pool(
            &fd,
            size,
            wl_shm::Error::InvalidFd.into(),
            wl_shm::Error::InvalidStride.into(),
        ) {
            shm.post_error(refusal.code, refusal.message);
            return;
        }
        if state.host.globals.shm.is_none()
            || !state.admit_object(client)
            || !state.admit_fd(client)
        {
            return;
        }
        let Some(host_shm) = &state.host.globals.shm else {
            return;
        };
        let host = host_shm.create_pool(fd.as_fd(), size, &state.qh, ());
        let pool = data_init.init(id, ());
        state.pools.insert(pool.id(), Pool { host, fd, size });
    }

    counted_destruction!(WlShm);
}

impl Dispatch<WlShmPool, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &WlShmPool,
        request: wl_shm_pool::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let Some(pool) = state.pools.get_mut(&resource.id()) else {
            return;
        };
        match request {
            wl_shm_pool::Request::CreateBuffer {
                id,
                offset,
                width,
                height,
                stride,
                format,
            } => {
                let format = match format {
                    WEnum::Value(format) => u32::from(format),
                    WEnum::Unknown(format) => format,
                };
                let asked = check::ShmBuffer {
                    offset,
                    width,
                    height,
                    stride,
                    format,
                };
                let errors = check::ShmErrors {
                    invalid_format: wl_shm::Error::InvalidFormat.into(),
                    invalid_stride: wl_shm::Error::InvalidStride.into(),
                };
                let offered = &state.host.globals.shm_formats;
                if let Err(refusal) = check::shm_buffer(asked, pool.size, offered, errors) {
                    resource.post_error(refusal.code, refusal.message);
                    return;
                }
                // Every format the relay offers is one the client library knows.
                let Ok(host_format) = HostFormat::try_from(format) else {
                    return;
                };
                let host = pool.host.create_buffer(
                    offset,
                    width,
                    height,
                    stride,
                    host_format,
                    &state.qh,
                    (),
                );
                if !state.admit_object(client) {
                    host.destroy();
                    return;
                }
                let buffer = data_init.init(id, ());
                state.host_buffers.insert(host.id(), buffer.id());
                state
                    .buffers
                    .insert(buffer.id(), Buffer::new(Some(host), (width, height)));
            }
            wl_shm_pool::Request::Resize { size } => {
                if let Err(refusal) = check::shm_resize(
                    &pool.fd,
                    size,
                    pool.size,
                    wl_shm::Error::InvalidFd.into(),
                    wl_shm::Error::InvalidStride.into(),
                ) {
                    resource.post_error(refusal.code, refusal.message);
                    return;
                }
                pool.host.resize(size);
                pool.size = size;
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &WlShmPool, _: &()) {
        state.object_gone();
        if let Some(pool) = state.pools.remove(&resource.id()) {
            pool.host.destroy();
            state.fds = state.fds.saturating_sub(1);
        }
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlBuffer,
        _: wl_buffer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &WlBuffer, _: &()) {
        state.object_gone();
        state.buffer_destroyed(&resource.id());
    }
}

impl GlobalDispatch<ZwpLinuxDmabufV1, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<ZwpLinuxDmabufV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        admit(state, client, resource, (), data_init);
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &ZwpLinuxDmabufV1,
        request: zwp_linux_dmabuf_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                if state.host.globals.dmabuf.is_none() || !state.admit_object(client) {
                    return;
                }
                let Some(dmabuf) = &state.host.globals.dmabuf else {
                    return;
                };
                let host = dmabuf.create_params(&state.qh, ());
                let params = data_init.init(params_id, ());
                state.params.insert(
                    params.id(),
                    Params {
                        host,
                        planes: [None; check::MOST_PLANES],
                        modifier: None,
                        used: false,
                    },
                );
            }
            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id } => {
                new_feedback(state, client, data_init, id, None);
            }
            zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id, surface } => {
                new_feedback(state, client, data_init, id, Some(surface.id()));
            }
            _ => {}
        }
    }

    counted_destruction!(ZwpLinuxDmabufV1);
}

/// The error codes of `zwp_linux_buffer_params_v1`, for the checks.
fn params_errors() -> ParamsErrors {
    use zwp_linux_buffer_params_v1::Error;
    ParamsErrors {
        plane_idx: Error::PlaneIdx.into(),
        plane_set: Error::PlaneSet.into(),
        incomplete: Error::Incomplete.into(),
        invalid_format: Error::InvalidFormat.into(),
        invalid_dimensions: Error::InvalidDimensions.into(),
        out_of_bounds: Error::OutOfBounds.into(),
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwp_linux_buffer_params_v1::{Error, Request};
        if let Request::Add { .. } = request {
            state.took_fd();
        }
        let Some(params) = state.params.get_mut(&resource.id()) else {
            return;
        };
        if params.used && !matches!(request, Request::Destroy) {
            resource.post_error(
                Error::AlreadyUsed,
                "the parameters were used to make a buffer already",
            );
            return;
        }
        let errors = params_errors();
        match request {
            Request::Add {
                fd,
                plane_idx,
                offset,
                stride,
                modifier_hi,
                modifier_lo,
            } => {
                let modifier = (u64::from(modifier_hi) << 32) | u64::from(modifier_lo);
                let checked =
                    check::dmabuf_size(&fd, errors.out_of_bounds, state.memory_stands_for_dmabuf)
                        .and_then(|size| {
                            check::dmabuf_add(
                                &params.planes,
                                params.modifier,
                                plane_idx,
                                modifier,
                                errors,
                            )
                            .map(|slot| (slot, size))
                        });
                let (slot, size) = match checked {
                    Ok(checked) => checked,
                    Err(refusal) => return resource.post_error(refusal.code, refusal.message),
                };
                // The fd passes to the compositor now; the relay keeps only what it checks
                // against at the creation.
                params.host.add(
                    fd.as_fd(),
                    plane_idx,
                    offset,
                    stride,
                    modifier_hi,
                    modifier_lo,
                );
                params.planes[slot] = Some(check::Plane {
                    offset,
                    stride,
                    size,
                });
                params.modifier = Some(modifier);
            }
            Request::Create {
                width,
                height,
                format,
                flags,
            } => {
                let flags = raw_flags(flags);
                if let Err(refusal) =
                    check::dmabuf_create(&params.planes, width, height, flags, errors)
                {
                    return resource.post_error(refusal.code, refusal.message);
                }
                params.used = true;
                params
                    .host
                    .create(width, height, format, HostFlags::from_bits_retain(flags));
                let host = params.host.clone();
                state.host_params.insert(
                    host.id(),
                    ParamsUse {
                        host,
                        made: Made::Later {
                            params: resource.clone(),
                            size: (width, height),
                        },
                    },
                );
            }
            Request::CreateImmed {
                buffer_id,
                width,
                height,
                format,
                flags,
            } => {
                let flags = raw_flags(flags);
                if let Err(refusal) =
                    check::dmabuf_create(&params.planes, width, height, flags, errors)
                {
                    return resource.post_error(refusal.code, refusal.message);
                }
                // The buffer is counted before anything is issued: refused, it leaves nothing on
                // the window's connection.
                if !state.admit_object(client) {
                    return;
                }
                let Some(params) = state.params.get_mut(&resource.id()) else {
                    return;
                };
                params.used = true;
                // Always `create`, never `create_immed`, on the window's connection: a dmabuf the
                // compositor cannot import is then a `failed` event, not a protocol error that
                // would close the window's connection.
                params
                    .host
                    .create(width, height, format, HostFlags::from_bits_retain(flags));
                let host = params.host.clone();
                let buffer = data_init.init(buffer_id, ());
                state
                    .buffers
                    .insert(buffer.id(), Buffer::new(None, (width, height)));
                state.host_params.insert(
                    host.id(),
                    ParamsUse {
                        host,
                        made: Made::Already(buffer.id()),
                    },
                );
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwpLinuxBufferParamsV1, _: &()) {
        state.object_gone();
        // Parameters used for a creation are destroyed when the compositor answers it.
        if let Some(params) = state.params.remove(&resource.id())
            && !params.used
        {
            params.host.destroy();
        }
    }
}

/// A bitfield's value, known bits or not.
fn raw_flags(flags: WEnum<zwp_linux_buffer_params_v1::Flags>) -> u32 {
    match flags {
        WEnum::Value(flags) => flags.bits(),
        WEnum::Unknown(bits) => bits,
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ZwpLinuxDmabufFeedbackV1,
        _: <ZwpLinuxDmabufFeedbackV1 as Resource>::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwpLinuxDmabufFeedbackV1, _: &()) {
        state.object_gone();
        state.feedbacks.retain(|(feedback, _)| feedback != resource);
    }
}

impl GlobalDispatch<WpViewporter, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WpViewporter>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        admit(state, client, resource, (), data_init);
    }
}

impl Dispatch<WpViewporter, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &WpViewporter,
        request: wp_viewporter::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let wp_viewporter::Request::GetViewport { id, surface } = request else {
            return;
        };
        let surface_id = surface.id();
        if state
            .surfaces
            .get(&surface_id)
            .is_some_and(|surface| surface.viewport.is_some())
        {
            resource.post_error(
                wp_viewporter::Error::ViewportExists,
                "the surface has a viewport already",
            );
            return;
        }
        if !state.admit_object(client) {
            return;
        }
        let viewport = data_init.init(id, surface_id.clone());
        if let Some(surface) = state.surfaces.get_mut(&surface_id) {
            surface.viewport = Some(viewport);
        }
    }

    counted_destruction!(WpViewporter);
}

impl Dispatch<WpViewport, ObjectId> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WpViewport,
        request: wp_viewport::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let bad_value = wp_viewport::Error::BadValue.into();
        let Some(surface) = state.surfaces.get_mut(surface) else {
            if !matches!(request, wp_viewport::Request::Destroy) {
                resource.post_error(wp_viewport::Error::NoSurface, "the surface was destroyed");
            }
            return;
        };
        let checked = match request {
            wp_viewport::Request::SetSource {
                x,
                y,
                width,
                height,
            } => check::viewport_source(x, y, width, height, bad_value)
                .map(|source| surface.set_source(source)),
            wp_viewport::Request::SetDestination { width, height } => {
                check::viewport_destination(width, height, bad_value)
                    .map(|destination| surface.set_destination(destination))
            }
            _ => Ok(()),
        };
        if let Err(refusal) = checked {
            resource.post_error(refusal.code, refusal.message);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &WpViewport, surface: &ObjectId) {
        state.object_gone();
        // The surface's source and destination go at its next commit.
        if let Some(surface) = state.surfaces.get_mut(surface)
            && surface.viewport.as_ref() == Some(resource)
        {
            surface.viewport = None;
            surface.set_source(None);
            surface.set_destination(None);
        }
    }
}

impl GlobalDispatch<WpPresentation, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WpPresentation>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let Some(presentation) = admit(state, client, resource, (), data_init) else {
            return;
        };
        if let Some(clock) = state.host.globals.clock {
            presentation.clock_id(clock);
        }
    }
}

impl Dispatch<WpPresentation, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &WpPresentation,
        request: wp_presentation::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let wp_presentation::Request::Feedback { surface, callback } = request else {
            return;
        };
        if !state.admit_object(client) {
            return;
        }
        let feedback = data_init.init(callback, ());
        match state.surfaces.get_mut(&surface.id()) {
            Some(surface) => surface.add_feedback(feedback),
            None => feedback.discarded(),
        }
    }

    counted_destruction!(WpPresentation);
}

impl Dispatch<WpPresentationFeedback, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WpPresentationFeedback,
        _: <WpPresentationFeedback as Resource>::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    counted_destruction!(WpPresentationFeedback);
}

impl GlobalDispatch<WpFractionalScaleManagerV1, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WpFractionalScaleManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        admit(state, client, resource, (), data_init);
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &WpFractionalScaleManagerV1,
        request: wp_fractional_scale_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use wp_fractional_scale_manager_v1::{Error, Request};
        let Request::GetFractionalScale { id, surface } = request else {
            return;
        };
        let surface_id = surface.id();
        if state
            .surfaces
            .get(&surface_id)
            .is_some_and(|surface| surface.has_fractional_scale)
        {
            resource.post_error(
                Error::FractionalScaleExists,
                "the surface has a fractional scale object already",
            );
            return;
        }
        if !state.admit_object(client) {
            return;
        }
        let object = data_init.init(id, surface_id.clone());
        if let Some(surface) = state.surfaces.get_mut(&surface_id) {
            surface.has_fractional_scale = true;
        }
        state.fractional_scales.push((object, surface_id));
        state.send_preferred_scale();
    }

    counted_destruction!(WpFractionalScaleManagerV1);
}

impl Dispatch<WpFractionalScaleV1, ObjectId> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WpFractionalScaleV1,
        _: <WpFractionalScaleV1 as Resource>::Request,
        _: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &WpFractionalScaleV1,
        surface: &ObjectId,
    ) {
        state.object_gone();
        state
            .fractional_scales
            .retain(|(object, _)| object != resource);
        if let Some(surface) = state.surfaces.get_mut(surface) {
            surface.has_fractional_scale = false;
        }
    }
}

impl GlobalDispatch<WpFifoManagerV1, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WpFifoManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        admit(state, client, resource, (), data_init);
    }
}

/// mpv's fifo, one for a surface at most: its barriers go with the surface's next commit, and
/// reach the plane's own fifo when that commit is issued on the plane ([`Surface`]).
impl Dispatch<WpFifoManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &WpFifoManagerV1,
        request: wp_fifo_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use wp_fifo_manager_v1::{Error, Request};
        let Request::GetFifo { id, surface } = request else {
            return;
        };
        let surface_id = surface.id();
        if state
            .surfaces
            .get(&surface_id)
            .is_some_and(|surface| surface.has_fifo)
        {
            resource.post_error(Error::AlreadyExists, "the surface has a fifo already");
            return;
        }
        if !state.admit_object(client) {
            return;
        }
        data_init.init(id, surface_id.clone());
        if let Some(surface) = state.surfaces.get_mut(&surface_id) {
            surface.has_fifo = true;
        }
    }

    counted_destruction!(WpFifoManagerV1);
}

impl Dispatch<WpFifoV1, ObjectId> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WpFifoV1,
        request: wp_fifo_v1::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use wp_fifo_v1::{Error, Request};
        let barrier = match request {
            Request::SetBarrier => true,
            Request::WaitBarrier => false,
            _ => return,
        };
        let Some(surface) = state.surfaces.get_mut(surface) else {
            resource.post_error(Error::SurfaceDestroyed, "the fifo's surface was destroyed");
            return;
        };
        if barrier {
            surface.set_fifo_barrier();
        } else {
            surface.wait_fifo_barrier();
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &WpFifoV1, surface: &ObjectId) {
        state.object_gone();
        if let Some(surface) = state.surfaces.get_mut(surface) {
            surface.has_fifo = false;
        }
    }
}

impl GlobalDispatch<WpContentTypeManagerV1, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WpContentTypeManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        admit(state, client, resource, (), data_init);
    }
}

impl Dispatch<WpContentTypeManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &WpContentTypeManagerV1,
        request: wp_content_type_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use wp_content_type_manager_v1::{Error, Request};
        let Request::GetSurfaceContentType { id, surface } = request else {
            return;
        };
        let surface_id = surface.id();
        if state
            .surfaces
            .get(&surface_id)
            .is_some_and(|surface| surface.has_content_type)
        {
            resource.post_error(
                Error::AlreadyConstructed,
                "the surface has a content type object already",
            );
            return;
        }
        if !state.admit_object(client) {
            return;
        }
        data_init.init(id, surface_id.clone());
        if let Some(surface) = state.surfaces.get_mut(&surface_id) {
            surface.has_content_type = true;
        }
    }

    counted_destruction!(WpContentTypeManagerV1);
}

impl Dispatch<WpContentTypeV1, ObjectId> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpContentTypeV1,
        request: wp_content_type_v1::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wp_content_type_v1::Request::SetContentType { content_type } = request
            && let Some(surface) = state.surfaces.get_mut(surface)
        {
            // A value the protocol does not define is taken as none.
            let value = match content_type {
                WEnum::Value(value) => u32::from(value),
                WEnum::Unknown(_) => 0,
            };
            surface.set_content_type(value);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &WpContentTypeV1, surface: &ObjectId) {
        state.object_gone();
        if let Some(surface) = state.surfaces.get_mut(surface) {
            surface.has_content_type = false;
            surface.set_content_type(0);
        }
    }
}

impl GlobalDispatch<WlOutput, u32> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<WlOutput>,
        name: &u32,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if !state.admit_object(client) {
            return;
        }
        let resource = data_init.init(resource, *name);
        if let Some(output) = state.outputs.iter_mut().find(|output| output.name == *name) {
            send_output_at_bind(&resource, &output.info);
            output.resources.push(resource);
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlOutput,
        _: wl_output::Request,
        _: &u32,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &WlOutput, name: &u32) {
        state.object_gone();
        if let Some(output) = state.outputs.iter_mut().find(|output| output.name == *name) {
            output.resources.retain(|object| object != resource);
        }
    }
}
