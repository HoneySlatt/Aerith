// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The window's side of the relay: the globals it binds on the interface's connection, the plane's
//! surface it makes there, and what it does with the compositor's events, most of them passed
//! back to mpv.

use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;

use wayland_client::globals::{GlobalList, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::content_type::v1::client::wp_content_type_manager_v1::WpContentTypeManagerV1;
use wayland_protocols::wp::content_type::v1::client::wp_content_type_v1::WpContentTypeV1;
use wayland_protocols::wp::fifo::v1::client::wp_fifo_manager_v1::WpFifoManagerV1;
use wayland_protocols::wp::fifo::v1::client::wp_fifo_v1::WpFifoV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{
    self, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::{
    self, ZwpLinuxDmabufFeedbackV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1 as MpvDmabufFeedback;
use wayland_protocols::wp::presentation_time::client::wp_presentation::{self, WpPresentation};
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback as MpvFeedback;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_server::Resource;
use wayland_server::protocol::wl_callback::WlCallback as MpvCallback;

use super::surface::Presented;
use super::{Host, Output, State, check};

/// The globals the relay binds on the window's connection.
pub(super) struct HostGlobals {
    pub(super) compositor: WlCompositor,
    pub(super) subcompositor: WlSubcompositor,
    pub(super) viewporter: WpViewporter,
    pub(super) shm: Option<WlShm>,
    /// The shared-memory formats the compositor offers that the relay knows.
    pub(super) shm_formats: Vec<u32>,
    pub(super) dmabuf: Option<ZwpLinuxDmabufV1>,
    pub(super) presentation: Option<WpPresentation>,
    /// The clock the compositor's presentation times are read on.
    pub(super) clock: Option<u32>,
    pub(super) fractional_scale: Option<WpFractionalScaleManagerV1>,
    pub(super) content_type: Option<WpContentTypeManagerV1>,
    pub(super) fifo: Option<WpFifoManagerV1>,
}

/// The highest version of each global the relay speaks.
const COMPOSITOR: u32 = 6;
const SHM: u32 = 2;
const DMABUF: u32 = 4;
const PRESENTATION: u32 = 2;
const OUTPUT: u32 = 4;

impl HostGlobals {
    /// Binds what the relay needs, and what it can offer mpv when the compositor has it. Fails
    /// when the compositor lacks what a plane needs: `wl_compositor` 4, `wl_subcompositor`,
    /// `wp_viewporter`; what it bound before is then destroyed, but `wl_compositor`, which has no
    /// destructor.
    pub(super) fn bind(globals: &GlobalList, qh: &QueueHandle<State>) -> io::Result<Self> {
        fn required(
            name: &'static str,
        ) -> impl Fn(wayland_client::globals::BindError) -> io::Error {
            move |error| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("the window's compositor offers no {name}: {error}"),
                )
            }
        }
        let compositor = globals
            .bind(qh, 4..=COMPOSITOR, ())
            .map_err(required("wl_compositor version 4"))?;
        let subcompositor: WlSubcompositor = globals
            .bind(qh, 1..=1, ())
            .map_err(required("wl_subcompositor"))?;
        let viewporter = match globals.bind(qh, 1..=1, ()) {
            Ok(viewporter) => viewporter,
            Err(error) => {
                subcompositor.destroy();
                return Err(required("wp_viewporter")(error));
            }
        };
        Ok(Self {
            compositor,
            subcompositor,
            viewporter,
            shm: globals.bind(qh, 1..=SHM, ()).ok(),
            shm_formats: Vec::new(),
            // Version 4 is the first with feedback, which Mesa needs to choose its formats.
            dmabuf: globals.bind(qh, DMABUF..=DMABUF, ()).ok(),
            presentation: globals.bind(qh, 1..=PRESENTATION, ()).ok(),
            clock: None,
            fractional_scale: globals.bind(qh, 1..=1, ()).ok(),
            content_type: globals.bind(qh, 1..=1, ()).ok(),
            fifo: globals.bind(qh, 1..=1, ()).ok(),
        })
    }
}

impl Host {
    /// Makes the plane's surface: a subsurface of `parent`, desynchronized, below `below`, with an
    /// empty input region and the relay's viewport. Its addition and place are the parent's state:
    /// they take effect at the interface's next commit of the parent.
    pub(super) fn make(
        connection: Connection,
        globals: HostGlobals,
        qh: &QueueHandle<State>,
        parent: &WlSurface,
        below: &WlSurface,
    ) -> Self {
        let surface = globals.compositor.create_surface(qh, ());
        let subsurface = globals
            .subcompositor
            .get_subsurface(&surface, parent, qh, ());
        subsurface.place_below(below);
        subsurface.set_desync();
        // The pointer passes through the video to the interface.
        let region = globals.compositor.create_region(qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        let viewport = globals.viewporter.get_viewport(&surface, qh, ());
        let fractional_scale = globals
            .fractional_scale
            .as_ref()
            .map(|manager| manager.get_fractional_scale(&surface, qh, ()));
        let content_type = globals
            .content_type
            .as_ref()
            .map(|manager| manager.get_surface_content_type(&surface, qh, ()));
        let fifo = globals
            .fifo
            .as_ref()
            .map(|manager| manager.get_fifo(&surface, qh, ()));
        let (default_feedback, surface_feedback) = match &globals.dmabuf {
            Some(dmabuf) => (
                FeedbackCache::new(Some(dmabuf.get_default_feedback(qh, FeedbackKind::Default))),
                FeedbackCache::new(Some(dmabuf.get_surface_feedback(
                    &surface,
                    qh,
                    FeedbackKind::Plane,
                ))),
            ),
            None => (FeedbackCache::new(None), FeedbackCache::new(None)),
        };
        surface.commit();
        Self {
            connection: Some(connection),
            globals,
            surface,
            subsurface,
            viewport,
            fractional_scale,
            content_type,
            fifo,
            default_feedback,
            surface_feedback,
            preferred_scale: None,
            preferred_buffer_scale: None,
            preferred_buffer_transform: None,
            entered: Vec::new(),
        }
    }

    /// Destroys the plane's surface and every global the relay bound, then drops the relay's
    /// queue on the connection. Once only.
    pub(super) fn destroy(&mut self) {
        let Some(connection) = self.connection.take() else {
            return;
        };
        for feedback in [&self.default_feedback, &self.surface_feedback] {
            if let Some(object) = &feedback.object {
                object.destroy();
            }
        }
        if let Some(content_type) = &self.content_type {
            content_type.destroy();
        }
        if let Some(fifo) = &self.fifo {
            fifo.destroy();
        }
        if let Some(fractional_scale) = &self.fractional_scale {
            fractional_scale.destroy();
        }
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        let globals = &self.globals;
        globals.viewporter.destroy();
        globals.subcompositor.destroy();
        if let Some(shm) = globals.shm.as_ref().filter(|shm| shm.version() >= 2) {
            shm.release();
        }
        if let Some(dmabuf) = &globals.dmabuf {
            dmabuf.destroy();
        }
        if let Some(presentation) = &globals.presentation {
            presentation.destroy();
        }
        if let Some(manager) = &globals.fractional_scale {
            manager.destroy();
        }
        if let Some(manager) = &globals.content_type {
            manager.destroy();
        }
        if let Some(manager) = &globals.fifo {
            manager.destroy();
        }
        // wl_compositor up to version 6 has no destructor: the compositor frees it with the
        // connection. The destructors go out at the connection's next flush, the interface's or
        // this one.
        let _ = connection.flush();
        // Dropping the last handle on the relay's queue frees the relay's proxies in libwayland
        // and the queue itself; the connection stays the interface's.
        drop(connection);
    }
}

/// Releases an output the relay bound, when its version has a destructor.
pub(super) fn release_output(output: &WlOutput) {
    if output.version() >= 3 {
        output.release();
    }
}

/// What the compositor says of an output, to repeat to mpv.
#[derive(Clone, Default)]
pub(super) struct OutputInfo {
    pub(super) geometry: Option<Geometry>,
    pub(super) mode: Option<Mode>,
    pub(super) scale: Option<i32>,
    pub(super) name: Option<String>,
    pub(super) description: Option<String>,
}

#[derive(Clone)]
pub(super) struct Geometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) physical_width: i32,
    pub(super) physical_height: i32,
    pub(super) subpixel: u32,
    pub(super) make: String,
    pub(super) model: String,
    pub(super) transform: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Mode {
    pub(super) flags: u32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) refresh: i32,
}

/// Which dmabuf feedback the compositor sends on an object: the default one, or the plane
/// surface's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FeedbackKind {
    Default,
    Plane,
}

/// One event of dmabuf feedback, kept to replay to mpv.
#[derive(Clone)]
pub(super) enum FeedbackEvent {
    FormatTable(Arc<OwnedFd>, u32),
    MainDevice(Vec<u8>),
    TrancheTargetDevice(Vec<u8>),
    TrancheFormats(Vec<u8>),
    TrancheFlags(u32),
    TrancheDone,
}

/// The compositor's dmabuf feedback: the last whole batch, which mpv gets at once when it asks,
/// and the batch being received. A batch ends with `done`.
pub(super) struct FeedbackCache {
    pub(super) object: Option<ZwpLinuxDmabufFeedbackV1>,
    pub(super) complete: Option<Vec<FeedbackEvent>>,
    incoming: Vec<FeedbackEvent>,
}

/// The most events of one feedback batch the relay keeps: a few tranches of a handful of events.
const MOST_FEEDBACK_EVENTS: usize = 256;

impl FeedbackCache {
    fn new(object: Option<ZwpLinuxDmabufFeedbackV1>) -> Self {
        Self {
            object,
            complete: None,
            incoming: Vec::new(),
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => {
                state.bind_output(registry, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => state.remove_output(name),
            _ => {}
        }
    }
}

impl State {
    /// Binds every output the compositor has now; later ones come as registry events.
    pub(super) fn watch_outputs(&mut self, globals: &GlobalList) {
        let outputs: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|global| global.interface == WlOutput::interface().name)
                .map(|global| (global.name, global.version))
                .collect()
        });
        let qh = self.qh.clone();
        for (name, version) in outputs {
            self.bind_output(globals.registry(), name, version, &qh);
        }
    }

    /// mpv needs `wl_output` version 2 at least (its `done`); an older output is not mirrored.
    fn bind_output(
        &mut self,
        registry: &WlRegistry,
        name: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        if version < 2 || self.outputs.iter().any(|output| output.name == name) {
            return;
        }
        let host: WlOutput = registry.bind(name, version.min(OUTPUT), qh, name);
        self.outputs.push(Output {
            name,
            host,
            info: OutputInfo::default(),
            complete: false,
            global: None,
            published_description: None,
            resources: Vec::new(),
        });
    }

    fn remove_output(&mut self, name: u32) {
        let Some(index) = self.outputs.iter().position(|output| output.name == name) else {
            return;
        };
        let output = self.outputs.remove(index);
        if let Some(global) = output.global {
            self.display.remove_global::<State>(global);
        }
        self.host.entered.retain(|entered| *entered != output.host);
        release_output(&output.host);
    }

    /// mpv's objects for the output the compositor names.
    pub(super) fn mpv_outputs(
        &self,
        host: &WlOutput,
    ) -> Vec<wayland_server::protocol::wl_output::WlOutput> {
        self.outputs
            .iter()
            .find(|output| output.host == *host)
            .map(|output| output.resources.clone())
            .unwrap_or_default()
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.iter_mut().find(|output| output.name == *name) else {
            return;
        };
        let info = &mut output.info;
        match event {
            wl_output::Event::Geometry {
                x,
                y,
                physical_width,
                physical_height,
                subpixel,
                make,
                model,
                transform,
            } => {
                info.geometry = Some(Geometry {
                    x,
                    y,
                    physical_width,
                    physical_height,
                    subpixel: raw(subpixel),
                    make,
                    model,
                    transform: raw(transform),
                });
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                refresh,
            } => {
                let flags = match flags {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                };
                // The current mode is the one mpv reads its rate from; others are not kept.
                if flags & wl_output::Mode::Current.bits() != 0 {
                    info.mode = Some(Mode {
                        flags,
                        width,
                        height,
                        refresh,
                    });
                }
            }
            wl_output::Event::Scale { factor } => info.scale = Some(factor),
            wl_output::Event::Name { name } => info.name = Some(name),
            wl_output::Event::Description { description } => info.description = Some(description),
            wl_output::Event::Done => {
                output.complete = true;
                let name = output.name;
                state.publish_output(name);
            }
            _ => {}
        }
    }
}

/// The raw value of an enum the compositor sent.
fn raw<T: Into<u32>>(value: WEnum<T>) -> u32 {
    match value {
        WEnum::Value(value) => value.into(),
        WEnum::Unknown(value) => value,
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlSubcompositor);
delegate_noop!(State: WlSubsurface);
delegate_noop!(State: WlRegion);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: ZwpLinuxDmabufV1);
delegate_noop!(State: WpFractionalScaleManagerV1);
delegate_noop!(State: WpContentTypeManagerV1);
delegate_noop!(State: WpContentTypeV1);
delegate_noop!(State: WpFifoManagerV1);
delegate_noop!(State: WpFifoV1);

impl Dispatch<WlShm, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_shm::Event::Format { format } = event {
            let format = raw(format);
            let formats = &mut state.host.globals.shm_formats;
            if check::shm_pixel_size(format).is_some() && !formats.contains(&format) {
                formats.push(format);
            }
        }
    }
}

impl Dispatch<WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.host.globals.clock = Some(clk_id);
        }
    }
}

impl Dispatch<WlSurface, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => {
                state.host.entered.push(output.clone());
                state.plane_entered(&output);
                state.display_changed();
            }
            wl_surface::Event::Leave { output } => {
                state.host.entered.retain(|entered| *entered != output);
                state.plane_left(&output);
                state.display_changed();
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                state.host.preferred_buffer_scale = Some(factor);
                state.send_preferred_buffer_scale();
            }
            wl_surface::Event::PreferredBufferTransform { transform } => {
                state.host.preferred_buffer_transform = Some(raw(transform));
                state.send_preferred_buffer_transform();
            }
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.host.preferred_scale = Some(scale);
            state.send_preferred_scale();
        }
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, FeedbackKind> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        kind: &FeedbackKind,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        let cache = match kind {
            FeedbackKind::Default => &mut state.host.default_feedback,
            FeedbackKind::Plane => &mut state.host.surface_feedback,
        };
        let event = match event {
            Event::Done => {
                let batch = std::mem::take(&mut cache.incoming);
                cache.complete = Some(batch.clone());
                state.forward_feedback(*kind, &batch);
                return;
            }
            Event::FormatTable { fd, size } => FeedbackEvent::FormatTable(Arc::new(fd), size),
            Event::MainDevice { device } => FeedbackEvent::MainDevice(device),
            Event::TrancheTargetDevice { device } => FeedbackEvent::TrancheTargetDevice(device),
            Event::TrancheFormats { indices } => FeedbackEvent::TrancheFormats(indices),
            Event::TrancheFlags { flags } => FeedbackEvent::TrancheFlags(match flags {
                WEnum::Value(flags) => flags.bits(),
                WEnum::Unknown(bits) => bits,
            }),
            Event::TrancheDone => FeedbackEvent::TrancheDone,
            _ => return,
        };
        // A compositor that never ends its batch is not followed past a bound.
        if cache.incoming.len() < MOST_FEEDBACK_EVENTS {
            cache.incoming.push(event);
        }
    }
}

/// Sends a batch of feedback to one of mpv's feedback objects, then `done`.
pub(super) fn replay_feedback(target: &MpvDmabufFeedback, batch: &[FeedbackEvent]) {
    use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
    for event in batch {
        match event {
            FeedbackEvent::FormatTable(fd, size) => target.format_table(fd.as_fd(), *size),
            FeedbackEvent::MainDevice(device) => target.main_device(device.clone()),
            FeedbackEvent::TrancheTargetDevice(device) => {
                target.tranche_target_device(device.clone());
            }
            FeedbackEvent::TrancheFormats(indices) => target.tranche_formats(indices.clone()),
            FeedbackEvent::TrancheFlags(flags) => {
                target.tranche_flags(TrancheFlags::from_bits_retain(*flags));
            }
            FeedbackEvent::TrancheDone => target.tranche_done(),
        }
    }
    target.done();
}

/// Why the relay asked the compositor for a callback.
pub(super) enum HostCallback {
    /// mpv's frame callbacks, answered together when the compositor answers this one.
    Frame(Vec<MpvCallback>),
    /// A change of the plane's, numbered: once the compositor has drawn it, the interface has
    /// committed its parent and the plane's surface may run desynchronized again.
    Applied(u64),
}

impl Dispatch<WlCallback, HostCallback> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        purpose: &HostCallback,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wl_callback::Event::Done { callback_data } = event else {
            return;
        };
        match purpose {
            HostCallback::Frame(callbacks) => {
                for callback in callbacks.iter().filter(|callback| callback.is_alive()) {
                    callback.done(callback_data);
                }
            }
            HostCallback::Applied(change) => state.plane_applied(*change),
        }
    }
}

impl Dispatch<WpPresentationFeedback, Vec<MpvFeedback>> for State {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        feedbacks: &Vec<MpvFeedback>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
        let alive = || feedbacks.iter().filter(|feedback| feedback.is_alive());
        match event {
            wp_presentation_feedback::Event::SyncOutput { output } => {
                let outputs = state.mpv_outputs(&output);
                for feedback in alive() {
                    for mpv_output in outputs.iter().filter(|o| o.client() == feedback.client()) {
                        feedback.sync_output(mpv_output);
                    }
                }
            }
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                let flags = match flags {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                };
                let msc = (u64::from(seq_hi) << 32) | u64::from(seq_lo);
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let at_ns = u128::from(seconds) * 1_000_000_000 + u128::from(tv_nsec);
                let hardware_timed = Kind::Vsync | Kind::HwClock;
                state.count_presented(
                    Presented {
                        msc,
                        at_ns,
                        refresh_ns: refresh,
                    },
                    flags & Kind::ZeroCopy.bits() != 0,
                    flags & hardware_timed.bits() == hardware_timed.bits(),
                );
                for feedback in alive() {
                    feedback.presented(
                        tv_sec_hi,
                        tv_sec_lo,
                        tv_nsec,
                        refresh,
                        seq_hi,
                        seq_lo,
                        Kind::from_bits_retain(flags),
                    );
                }
            }
            wp_presentation_feedback::Event::Discarded => {
                state.presentation.discarded += 1;
                for feedback in alive() {
                    feedback.discarded();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        state: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.host_released(&buffer.id());
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for State {
    fn event(
        state: &mut Self,
        params: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                state.dmabuf_created(params, buffer);
            }
            zwp_linux_buffer_params_v1::Event::Failed => state.dmabuf_failed(params),
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}
