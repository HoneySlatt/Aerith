// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A fake compositor for the relay's tests, and the interface that connects to it.
//!
//! The compositor is a Wayland server in the test's process, on a thread of its own. It records
//! what its client asks of each surface, answers frame callbacks, presentation feedback and buffer
//! releases at once or when the test says, and raises the protocol errors wlroots 0.20 raises on
//! the values the relay must never send, when they are sent and at each commit (a buffer whose
//! size its scale does not divide, a viewport's source outside its buffer or of a fractional size
//! without a destination): such an error, recorded in [`Recorded::errors`], closes the interface's
//! connection, as a real compositor would.
//!
//! The interface ([`Interface`]) is the test's own libwayland connection to it, with a parent
//! surface and, if asked, a subsurface of its own: what the relay joins, as it joins a real
//! window's connection. This file is shared by the integration tests (`tests/common/mod.rs`) and
//! by the relay's unit tests (`src/relay/tests.rs`, through `#[path]`).

use std::collections::HashMap;
use std::io::Write as _;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle};
use rustix::event::{PollFd, PollFlags};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor as ClientCompositor;
use wayland_client::protocol::wl_registry::WlRegistry as ClientRegistry;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor as ClientSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface as ClientSubsurface;
use wayland_client::protocol::wl_surface::WlSurface as ClientSurface;
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};
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
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::{
    self, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::{
    self, ZwpLinuxDmabufFeedbackV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::{
    self, ZwpLinuxDmabufV1,
};
use wayland_protocols::wp::presentation_time::server::wp_presentation::{self, WpPresentation};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::{
    Kind, WpPresentationFeedback,
};
use wayland_protocols::wp::viewporter::server::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::server::wp_viewporter::{self, WpViewporter};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, GlobalId, ObjectId};
use wayland_server::protocol::wl_buffer::{self, WlBuffer};
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::protocol::wl_region::{self, WlRegion};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_subcompositor::{self, WlSubcompositor};
use wayland_server::protocol::wl_subsurface::{self, WlSubsurface};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

/// How long a test waits for the fake compositor to see something.
pub const HOST_WAIT: Duration = Duration::from_secs(10);

/// The refresh rate the fake output announces, in mHz.
pub const REFRESH_MHZ: i32 = 60_000;

/// The refresh period presentations give, in nanoseconds.
pub const REFRESH_NS: u64 = 16_666_666;

/// The fake output's size, which a surface fills when fullscreen.
pub const OUTPUT_SIZE: (i32, i32) = (1920, 1080);

/// Under VRR, the time from one frame presented to the next: a 24 fps film's period, 2.5 refreshes,
/// so that no frame shows at a refresh of the fixed rate.
pub const VRR_FRAME_NS: u64 = 41_666_667;

/// What the fake compositor saw, and how it answers.
#[derive(Default)]
pub struct Recorded {
    pub surfaces: Vec<SurfaceRecord>,
    pub buffers: HashMap<ObjectId, BufferRecord>,
    /// The protocol errors the compositor raised: each closed its client's connection.
    pub errors: Vec<String>,
    /// Whether commits are answered at once: frame callbacks done, feedback presented, the
    /// buffer replaced released.
    pub answer_at_once: bool,
    /// Whether presented frames say they were scanned out.
    pub zero_copy: bool,
    /// Whether dmabufs fail to import.
    pub dmabufs_fail: bool,
    /// Requests of `create_immed`, which the relay must never make.
    pub immediate_dmabufs: u32,
    /// The refresh counter of the last presentation.
    pub msc: u64,
    /// Whether presentations carry no refresh counter, as KWin 6.8's do: only their time.
    pub no_msc: bool,
    /// At a fixed rate, the refreshes from one frame presented to the next; 0 counts as 1.
    pub refreshes_a_frame: u64,
    /// Whether the display runs under VRR: each frame [`VRR_FRAME_NS`] after the last, between
    /// the fixed rate's refreshes.
    pub vrr: bool,
    /// Whether VRR turns on while a surface with a viewport fills the output, and off when its
    /// viewport makes it smaller, at its commit, as KWin's automatic VRR does for fullscreen
    /// windows.
    pub vrr_while_fullscreen: bool,
    /// When the last frame was presented, in nanoseconds; 0 before the first.
    presented_at: u64,
    regions: HashMap<ObjectId, Vec<[i32; 4]>>,
    pools: HashMap<ObjectId, i32>,
    /// The dmabuf parameters objects alive, and the planes added to each.
    pub params: HashMap<ObjectId, u32>,
    /// The `wl_subcompositor` and `wp_viewporter` objects bound, by interface, and whether each
    /// was destroyed since.
    pub bound: Vec<(&'static str, bool)>,
    /// The `wl_output` objects bound.
    outputs: Vec<WlOutput>,
    /// mpv's surfaces' content types, as the relay set them.
    pub content_types: Vec<u32>,
    /// Whether a fractional scale object is told its scale only when a test sends it, through
    /// `fractional_scales`, rather than at once.
    pub fractional_scale_later: bool,
    pub fractional_scales: Vec<WpFractionalScaleV1>,
}

/// One surface, as its client set it.
pub struct SurfaceRecord {
    pub id: ObjectId,
    pub subsurface: Option<SubsurfaceRecord>,
    /// The input region committed: `None` is the default, the whole surface.
    pub input_region: Option<Vec<[i32; 4]>>,
    /// The opaque region committed: `None` is the default, nothing opaque.
    pub opaque_region: Option<Vec<[i32; 4]>>,
    /// The buffer committed.
    pub buffer: Option<ObjectId>,
    /// The size of the buffer last committed, `(0, 0)` for none, kept when it is destroyed, as
    /// wlroots keeps it, and checked at each commit.
    pub buffer_size: (i32, i32),
    pub scale: i32,
    pub transform: u32,
    pub source: Option<[f64; 4]>,
    pub destination: Option<(i32, i32)>,
    /// The surface's viewport, which raises the viewport's errors at a commit.
    viewport: Option<WpViewport>,
    pub commits: u32,
    /// Commits that attached a buffer.
    pub buffer_commits: u32,
    /// Every request on the surface, its subsurface and its viewport, by name, in order.
    pub log: Vec<&'static str>,
    pending: Pending,
    /// Committed frame callbacks and feedback, not answered yet.
    callbacks: Vec<WlCallback>,
    feedbacks: Vec<WpPresentationFeedback>,
    pub destroyed: bool,
}

#[derive(Default)]
struct Pending {
    buffer: Option<Option<ObjectId>>,
    callbacks: Vec<WlCallback>,
    feedbacks: Vec<WpPresentationFeedback>,
    scale: Option<i32>,
    transform: Option<u32>,
    source: Option<Option<[f64; 4]>>,
    destination: Option<Option<(i32, i32)>>,
    input_region: Option<Option<Vec<[i32; 4]>>>,
    opaque_region: Option<Option<Vec<[i32; 4]>>>,
}

/// A surface's subsurface role.
pub struct SubsurfaceRecord {
    pub parent: ObjectId,
    pub sync: bool,
    pub position: (i32, i32),
    /// The surface it was placed below.
    pub below: Option<ObjectId>,
    pub destroyed: bool,
}

/// A buffer the client made.
pub struct BufferRecord {
    pub width: i32,
    pub height: i32,
    pub dmabuf: bool,
    resource: WlBuffer,
    pub destroyed: bool,
}

impl Recorded {
    pub fn surface(&self, id: &ObjectId) -> Option<&SurfaceRecord> {
        self.surfaces.iter().find(|surface| surface.id == *id)
    }

    fn surface_mut(&mut self, id: &ObjectId) -> Option<&mut SurfaceRecord> {
        self.surfaces.iter_mut().find(|surface| surface.id == *id)
    }

    fn error(&mut self, resource: &impl Resource, code: u32, message: String) {
        self.errors.push(message.clone());
        resource.post_error(code, message);
    }

    /// Answers every committed frame callback.
    fn frame_done(&mut self) {
        for surface in &mut self.surfaces {
            for callback in surface.callbacks.drain(..) {
                if callback.is_alive() {
                    callback.done(0);
                }
            }
        }
    }

    /// Presents every committed feedback: at the next frame's refresh at a fixed rate, from 1 s,
    /// or between refreshes under VRR. The refresh counter counts the display's refreshes.
    fn present(&mut self) {
        let last = if self.presented_at == 0 {
            1_000_000_000
        } else {
            self.presented_at
        };
        let refreshes = self.refreshes_a_frame.max(1);
        let at = if self.vrr {
            self.msc += 1;
            last + VRR_FRAME_NS
        } else {
            self.msc += refreshes;
            last + refreshes * REFRESH_NS
        };
        self.presented_at = at;
        let mut flags = Kind::Vsync | Kind::HwClock | Kind::HwCompletion;
        if self.zero_copy {
            flags |= Kind::ZeroCopy;
        }
        let msc = self.msc;
        let (seconds, nanoseconds) = ((at / 1_000_000_000) as u32, (at % 1_000_000_000) as u32);
        let counter = if self.no_msc { 0 } else { msc };
        for surface in &mut self.surfaces {
            for feedback in surface.feedbacks.drain(..) {
                if feedback.is_alive() {
                    feedback.presented(
                        0,
                        seconds,
                        nanoseconds,
                        REFRESH_NS as u32,
                        (counter >> 32) as u32,
                        counter as u32,
                        flags,
                    );
                }
            }
        }
    }
}

/// The fake compositor's state, behind the lock its thread and the test share.
type Shared = Arc<(Mutex<Recorded>, Condvar)>;

struct HostState {
    shared: Shared,
}

impl HostState {
    fn recorded(&self) -> MutexGuard<'_, Recorded> {
        self.shared.0.lock().unwrap()
    }
}

/// The fake compositor.
pub struct FakeHost {
    shared: Shared,
    display: DisplayHandle,
    /// Its one output's global, until it is removed.
    output: Option<GlobalId>,
    stop: UnixStream,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct HostClient;

impl ClientData for HostClient {
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

impl FakeHost {
    /// Starts the fake compositor; it answers commits at once.
    pub fn start() -> Self {
        Self::start_without(&[])
    }

    /// Starts the fake compositor without the globals `missing` names.
    pub fn start_without(missing: &[&str]) -> Self {
        let mut display: Display<HostState> = Display::new().unwrap();
        let handle = display.handle();
        let offered = |name: &str| !missing.contains(&name);
        handle.create_global::<HostState, WlCompositor, ()>(6, ());
        handle.create_global::<HostState, WlSubcompositor, ()>(1, ());
        if offered("wp_viewporter") {
            handle.create_global::<HostState, WpViewporter, ()>(1, ());
        }
        handle.create_global::<HostState, WlShm, ()>(1, ());
        let output = handle.create_global::<HostState, WlOutput, ()>(4, ());
        handle.create_global::<HostState, WpPresentation, ()>(2, ());
        handle.create_global::<HostState, ZwpLinuxDmabufV1, ()>(4, ());
        handle.create_global::<HostState, WpFractionalScaleManagerV1, ()>(1, ());
        handle.create_global::<HostState, WpContentTypeManagerV1, ()>(1, ());
        handle.create_global::<HostState, WpFifoManagerV1, ()>(1, ());
        let shared: Shared = Arc::default();
        shared.0.lock().unwrap().answer_at_once = true;
        let (stop, stopped) = UnixStream::pair().unwrap();
        let mut state = HostState {
            shared: Arc::clone(&shared),
        };
        let thread = thread::Builder::new()
            .name("fake-compositor".to_owned())
            .spawn(move || {
                loop {
                    let ready = {
                        let poll_fd = display.backend().poll_fd();
                        let mut fds = [
                            PollFd::new(&stopped, PollFlags::IN),
                            PollFd::new(&poll_fd, PollFlags::IN),
                        ];
                        rustix::event::poll(&mut fds, None).unwrap();
                        (!fds[0].revents().is_empty(), !fds[1].revents().is_empty())
                    };
                    if ready.0 {
                        return;
                    }
                    if ready.1 {
                        let _ = display.dispatch_clients(&mut state);
                        let _ = display.flush_clients();
                        state.shared.1.notify_all();
                    }
                }
            })
            .unwrap();
        Self {
            shared,
            display: handle,
            output: Some(output),
            stop,
            thread: Some(thread),
        }
    }

    /// Tells every client of the output's state again, then `done`, with a new description if
    /// given, as a compositor does when an output changes.
    pub fn announce_output(&self, description: Option<&str>) {
        self.with(|recorded| {
            recorded.outputs.retain(Resource::is_alive);
            for output in &recorded.outputs {
                output.mode(
                    wl_output::Mode::Current | wl_output::Mode::Preferred,
                    OUTPUT_SIZE.0,
                    OUTPUT_SIZE.1,
                    REFRESH_MHZ,
                );
                if let Some(description) = description {
                    output.description(description.to_owned());
                }
                output.done();
            }
        });
    }

    /// Removes the compositor's one output, as when a screen is unplugged.
    pub fn remove_output(&mut self) {
        if let Some(output) = self.output.take() {
            self.display.remove_global::<HostState>(output);
            let _ = self.display.flush_clients();
        }
    }

    /// Runs `change` on what the compositor recorded, then sends what it sent.
    pub fn with<T>(&self, change: impl FnOnce(&mut Recorded) -> T) -> T {
        let value = change(&mut self.shared.0.lock().unwrap());
        let _ = self.display.clone().flush_clients();
        value
    }

    /// Waits until `found` finds something in what the compositor recorded, and returns it.
    pub fn wait<T>(&self, what: &str, mut found: impl FnMut(&Recorded) -> Option<T>) -> T {
        let deadline = Instant::now() + HOST_WAIT;
        let mut recorded = self.shared.0.lock().unwrap();
        loop {
            if let Some(value) = found(&recorded) {
                return value;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the fake compositor never saw {what}");
            recorded = self.shared.1.wait_timeout(recorded, left).unwrap().0;
        }
    }

    /// Tells every surface committed so far that it left every output its client bound.
    pub fn leave_outputs(&self) {
        let handle = self.display.clone();
        self.with(|recorded| {
            for record in recorded
                .surfaces
                .iter()
                .filter(|surface| surface.commits > 0)
            {
                let Ok(surface) = WlSurface::from_id(&handle, record.id.clone()) else {
                    continue;
                };
                for output in &recorded.outputs {
                    if output.client() == surface.client() {
                        surface.leave(output);
                    }
                }
            }
        });
    }

    /// Answers every frame callback committed so far.
    pub fn frame_done(&self) {
        self.with(Recorded::frame_done);
    }

    /// Connects an interface: a libwayland connection with a parent surface, and a subsurface
    /// of its own above it if `sibling`.
    pub fn connect(&self, sibling: bool) -> Interface {
        let (server, client) = UnixStream::pair().unwrap();
        self.display
            .clone()
            .insert_client(server, Arc::new(HostClient))
            .unwrap();
        Interface::new(Connection::from_socket(client).unwrap(), sibling)
    }
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        let _ = self.stop.write_all(b"x");
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The interface's side: its connection to the fake compositor, its parent surface, and its own
/// subsurface if it has one.
pub struct Interface {
    pub connection: Connection,
    queue: EventQueue<InterfaceState>,
    pub parent: ClientSurface,
    pub sibling: Option<(ClientSurface, ClientSubsurface)>,
}

struct InterfaceState;

impl Interface {
    fn new(connection: Connection, sibling: bool) -> Self {
        let (globals, mut queue) = registry_queue_init::<InterfaceState>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: ClientCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let parent = compositor.create_surface(&qh, ());
        let sibling = sibling.then(|| {
            let subcompositor: ClientSubcompositor = globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let subsurface = subcompositor.get_subsurface(&surface, &parent, &qh, ());
            (surface, subsurface)
        });
        parent.commit();
        queue.roundtrip(&mut InterfaceState).unwrap();
        Self {
            connection,
            queue,
            parent,
            sibling,
        }
    }

    /// The handles the player is given, as a windowing library hands them out: the display, the
    /// parent, and the surface the plane goes below: the interface's subsurface if it has one,
    /// else the parent. They live as long as this interface.
    pub fn handles(
        &self,
    ) -> (
        WaylandDisplayHandle,
        WaylandWindowHandle,
        WaylandWindowHandle,
    ) {
        let display = NonNull::new(self.connection.backend().display_ptr().cast()).unwrap();
        let surface = |surface: &ClientSurface| {
            WaylandWindowHandle::new(NonNull::new(surface.id().as_ptr().cast()).unwrap())
        };
        let below = self
            .sibling
            .as_ref()
            .map_or(&self.parent, |(surface, _)| surface);
        (
            WaylandDisplayHandle::new(display),
            surface(&self.parent),
            surface(below),
        )
    }

    /// Commits the parent surface, as the interface does at each of its frames.
    pub fn commit(&self) {
        self.parent.commit();
        self.connection.flush().unwrap();
    }

    /// A round trip on the interface's connection: it fails once the compositor closed it.
    pub fn roundtrip(&mut self) -> Result<(), String> {
        self.queue
            .roundtrip(&mut InterfaceState)
            .map(|_| ())
            .map_err(|error| error.to_string())?;
        match self.connection.protocol_error() {
            Some(error) => Err(error.to_string()),
            None => Ok(()),
        }
    }

    /// The protocol numbers of the parent and of the interface's own subsurface's surface.
    pub fn numbers(&self) -> (u32, Option<u32>) {
        (
            self.parent.id().protocol_id(),
            self.sibling
                .as_ref()
                .map(|(surface, _)| surface.id().protocol_id()),
        )
    }

    /// The plane's surface: the subsurface of the parent that is not the interface's own.
    pub fn plane<'a>(&self, recorded: &'a Recorded) -> Option<&'a SurfaceRecord> {
        let (parent, own) = self.numbers();
        recorded.surfaces.iter().find(|surface| {
            !surface.destroyed
                && Some(surface.id.protocol_id()) != own
                && surface
                    .subsurface
                    .as_ref()
                    .is_some_and(|sub| sub.parent.protocol_id() == parent && !sub.destroyed)
        })
    }
}

impl wayland_client::Dispatch<ClientRegistry, GlobalListContents> for InterfaceState {
    fn event(
        _: &mut Self,
        _: &ClientRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(InterfaceState: ClientCompositor);
wayland_client::delegate_noop!(InterfaceState: ClientSubcompositor);
wayland_client::delegate_noop!(InterfaceState: ClientSubsurface);
wayland_client::delegate_noop!(InterfaceState: ignore ClientSurface);

/// Binds a global with no data, for the globals whose bind does nothing else.
macro_rules! plain_global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for HostState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                _: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                data_init.init(resource, ());
            }
        }
    )*};
}

plain_global!(
    WlCompositor,
    ZwpLinuxDmabufV1,
    WpFractionalScaleManagerV1,
    WpContentTypeManagerV1,
    WpFifoManagerV1
);

/// Binds a global whose object has a destructor, and records it in [`Recorded::bound`].
macro_rules! recorded_global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for HostState {
            fn bind(
                state: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                _: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                let object = data_init.init(resource, ());
                state.recorded().bound.push((object.id().interface().name, false));
            }
        }
    )*};
}

recorded_global!(WlSubcompositor, WpViewporter);

impl Recorded {
    /// Records that an object bound to a global was destroyed.
    fn unbound(&mut self, interface: &'static str) {
        if let Some(bound) = self
            .bound
            .iter_mut()
            .find(|(name, destroyed)| *name == interface && !destroyed)
        {
            bound.1 = true;
        }
    }
}

impl GlobalDispatch<WlShm, ()> for HostState {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlShm>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let shm = data_init.init(resource, ());
        shm.format(wl_shm::Format::Argb8888);
        shm.format(wl_shm::Format::Xrgb8888);
    }
}

impl GlobalDispatch<WlOutput, ()> for HostState {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<WlOutput>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let output = data_init.init(resource, ());
        output.geometry(
            0,
            0,
            600,
            340,
            wl_output::Subpixel::Unknown,
            "Aerith".to_owned(),
            "Fake output".to_owned(),
            wl_output::Transform::Normal,
        );
        output.mode(
            wl_output::Mode::Current | wl_output::Mode::Preferred,
            OUTPUT_SIZE.0,
            OUTPUT_SIZE.1,
            REFRESH_MHZ,
        );
        output.scale(1);
        if output.version() >= 4 {
            output.name("FAKE-1".to_owned());
            output.description("The fake compositor's output".to_owned());
        }
        output.done();
        // As KWin does, the client's surfaces already committed are on this output, shown or not.
        let committed: Vec<ObjectId> = state
            .recorded()
            .surfaces
            .iter()
            .filter(|surface| surface.commits > 0 && !surface.destroyed)
            .map(|surface| surface.id.clone())
            .collect();
        for id in committed {
            if let Ok(surface) = WlSurface::from_id(handle, id)
                && surface.client().as_ref() == Some(client)
            {
                surface.enter(&output);
            }
        }
        state.recorded().outputs.push(output);
    }
}

impl GlobalDispatch<WpPresentation, ()> for HostState {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WpPresentation>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        // CLOCK_MONOTONIC, which mpv requires to use presentation times.
        data_init.init(resource, ()).clock_id(1);
    }
}

impl Dispatch<WlCompositor, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                let resource = data_init.init(id, ());
                state.recorded().surfaces.push(SurfaceRecord {
                    id: resource.id(),
                    subsurface: None,
                    input_region: None,
                    opaque_region: None,
                    buffer: None,
                    buffer_size: (0, 0),
                    scale: 1,
                    transform: 0,
                    source: None,
                    destination: None,
                    viewport: None,
                    commits: 0,
                    buffer_commits: 0,
                    log: Vec::new(),
                    pending: Pending::default(),
                    callbacks: Vec::new(),
                    feedbacks: Vec::new(),
                    destroyed: false,
                });
            }
            wl_compositor::Request::CreateRegion { id } => {
                let region = data_init.init(id, ());
                state.recorded().regions.insert(region.id(), Vec::new());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegion, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlRegion,
        request: wl_region::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_region::Request::Add {
            x,
            y,
            width,
            height,
        } = request
            && let Some(rects) = state.recorded().regions.get_mut(&resource.id())
        {
            rects.push([x, y, width, height]);
        }
    }
}

impl Dispatch<WlSurface, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let mut recorded = state.recorded();
        let recorded = &mut *recorded;
        let id = resource.id();
        match request {
            wl_surface::Request::Attach { buffer, x, y } => {
                if resource.version() >= 5 && (x != 0 || y != 0) {
                    recorded.error(resource, 6, "attach with an offset".to_owned());
                    return;
                }
                let Some(surface) = recorded.surface_mut(&id) else {
                    return;
                };
                surface.log.push(if buffer.is_some() {
                    "attach"
                } else {
                    "attach null"
                });
                surface.pending.buffer = Some(buffer.map(|buffer| buffer.id()));
            }
            wl_surface::Request::Frame { callback } => {
                let callback = data_init.init(callback, ());
                if let Some(surface) = recorded.surface_mut(&id) {
                    surface.log.push("frame");
                    surface.pending.callbacks.push(callback);
                }
            }
            wl_surface::Request::SetBufferScale { scale } => {
                if scale <= 0 {
                    recorded.error(resource, 1, format!("buffer scale {scale}"));
                    return;
                }
                if let Some(surface) = recorded.surface_mut(&id) {
                    surface.log.push("set_buffer_scale");
                    surface.pending.scale = Some(scale);
                }
            }
            wl_surface::Request::SetBufferTransform { transform } => match transform {
                WEnum::Value(transform) => {
                    if let Some(surface) = recorded.surface_mut(&id) {
                        surface.log.push("set_buffer_transform");
                        surface.pending.transform = Some(transform.into());
                    }
                }
                WEnum::Unknown(value) => {
                    recorded.error(resource, 2, format!("buffer transform {value}"));
                }
            },
            wl_surface::Request::SetInputRegion { region } => {
                let rects = region.map(|region| {
                    recorded
                        .regions
                        .get(&region.id())
                        .cloned()
                        .unwrap_or_default()
                });
                if let Some(surface) = recorded.surface_mut(&id) {
                    surface.pending.input_region = Some(rects);
                }
            }
            wl_surface::Request::SetOpaqueRegion { region } => {
                let rects = region.map(|region| {
                    recorded
                        .regions
                        .get(&region.id())
                        .cloned()
                        .unwrap_or_default()
                });
                if let Some(surface) = recorded.surface_mut(&id) {
                    surface.pending.opaque_region = Some(rects);
                }
            }
            wl_surface::Request::Commit => commit(recorded, resource),
            wl_surface::Request::Destroy => {
                if let Some(surface) = recorded.surface_mut(&id) {
                    surface.log.push("destroy");
                    surface.destroyed = true;
                }
            }
            _ => {}
        }
    }
}

/// Applies a surface's pending state, and answers at once if the compositor does. The new state is
/// checked first, as wlroots 0.20 checks it (`surface_finalize_pending` and the viewporter's
/// `viewport_handle_surface_client_commit`); a refusal is an error that closes the connection.
fn commit(recorded: &mut Recorded, resource: &WlSurface) {
    let id = resource.id();
    let answer = recorded.answer_at_once;
    let buffers = &recorded.buffers;
    let Some(surface) = recorded.surfaces.iter().find(|surface| surface.id == id) else {
        return;
    };
    // A buffer attached now gives its size, none if destroyed before this commit; else the last
    // one's stays.
    let buffer_size = match &surface.pending.buffer {
        Some(attached) => attached
            .as_ref()
            .and_then(|buffer| buffers.get(buffer))
            .filter(|buffer| !buffer.destroyed)
            .map_or((0, 0), |buffer| (buffer.width, buffer.height)),
        None => surface.buffer_size,
    };
    let refusal = commit_refusal(surface, buffer_size);
    if let Some((on_viewport, code, message)) = refusal {
        let viewport = surface.viewport.clone().filter(|_| on_viewport);
        match viewport {
            Some(viewport) => recorded.error(&viewport, code, message),
            None => recorded.error(resource, code, message),
        }
        return;
    }
    let Some(surface) = recorded.surface_mut(&id) else {
        return;
    };
    surface.log.push("commit");
    surface.commits += 1;
    surface.buffer_size = buffer_size;
    let pending = std::mem::take(&mut surface.pending);
    let mut replaced = None;
    if let Some(buffer) = pending.buffer {
        if buffer.is_some() {
            surface.buffer_commits += 1;
        }
        replaced = std::mem::replace(&mut surface.buffer, buffer.clone())
            .filter(|old| Some(old) != buffer.as_ref());
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
    if let Some(region) = pending.input_region {
        surface.input_region = region;
    }
    if let Some(region) = pending.opaque_region {
        surface.opaque_region = region;
    }
    surface.callbacks.extend(pending.callbacks);
    surface.feedbacks.extend(pending.feedbacks);
    let destination = surface.destination;
    if recorded.vrr_while_fullscreen
        && let Some(destination) = destination
    {
        recorded.vrr = destination == OUTPUT_SIZE;
    }
    if answer {
        if let Some(old) = replaced
            && let Some(buffer) = recorded.buffers.get(&old)
            && buffer.resource.is_alive()
        {
            buffer.resource.release();
        }
        recorded.frame_done();
        recorded.present();
    }
}

/// Why wlroots would refuse `surface`'s pending state, with a buffer of `buffer_size`: whether on
/// the viewport (else on the surface), the error's code, and a message.
fn commit_refusal(surface: &SurfaceRecord, buffer_size: (i32, i32)) -> Option<(bool, u32, String)> {
    let pending = &surface.pending;
    let scale = pending.scale.unwrap_or(surface.scale);
    let transform = pending.transform.unwrap_or(surface.transform);
    let source = pending.source.unwrap_or(surface.source);
    let destination = pending.destination.unwrap_or(surface.destination);
    let (width, height) = buffer_size;
    // wlroots only logs it for a surface without a role.
    let has_role = surface.subsurface.is_some();
    if has_role && source.is_none() && (width % scale != 0 || height % scale != 0) {
        return Some((
            false,
            wl_surface::Error::InvalidSize.into(),
            format!("a buffer of {width}x{height} at scale {scale}"),
        ));
    }
    let [x, y, source_width, source_height] = source?;
    if destination.is_none() && (source_width.fract() != 0.0 || source_height.fract() != 0.0) {
        return Some((
            true,
            wp_viewport::Error::BadSize.into(),
            "a fractional source without a destination".to_owned(),
        ));
    }
    if width > 0 && height > 0 {
        let (mut width, mut height) = (width / scale, height / scale);
        // 90 and 270 degrees, flipped or not.
        if transform % 2 == 1 {
            std::mem::swap(&mut width, &mut height);
        }
        if x + source_width > f64::from(width) || y + source_height > f64::from(height) {
            return Some((
                true,
                wp_viewport::Error::OutOfBuffer.into(),
                format!("a source outside a buffer of {width}x{height}"),
            ));
        }
    }
    None
}

impl Dispatch<WlCallback, ()> for HostState {
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
}

impl Dispatch<WlSubcompositor, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlSubcompositor,
        request: wl_subcompositor::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let wl_subcompositor::Request::GetSubsurface {
            id,
            surface,
            parent,
        } = request
        else {
            if let wl_subcompositor::Request::Destroy = request {
                state.recorded().unbound("wl_subcompositor");
            }
            return;
        };
        let mut recorded = state.recorded();
        if surface == parent {
            recorded.error(resource, 1, "a subsurface of itself".to_owned());
            return;
        }
        data_init.init(id, surface.id());
        if let Some(record) = recorded.surface_mut(&surface.id()) {
            record.subsurface = Some(SubsurfaceRecord {
                parent: parent.id(),
                sync: true,
                position: (0, 0),
                below: None,
                destroyed: false,
            });
        }
    }
}

impl Dispatch<WlSubsurface, ObjectId> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlSubsurface,
        request: wl_subsurface::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut recorded = state.recorded();
        let recorded = &mut *recorded;
        let parent = recorded
            .surface(surface)
            .and_then(|record| record.subsurface.as_ref())
            .map(|sub| sub.parent.clone());
        if let wl_subsurface::Request::PlaceBelow { sibling } = &request {
            // wlroots: the parent or another subsurface of the same parent.
            let sibling_parent = recorded
                .surface(&sibling.id())
                .and_then(|record| record.subsurface.as_ref())
                .map(|sub| sub.parent.clone());
            if Some(sibling.id()) != parent && sibling_parent != parent {
                recorded.error(
                    resource,
                    0,
                    "place_below: not a parent or sibling".to_owned(),
                );
                return;
            }
        }
        let Some(record) = recorded.surface_mut(surface) else {
            return;
        };
        let Some(sub) = record.subsurface.as_mut() else {
            return;
        };
        let name = match request {
            wl_subsurface::Request::SetPosition { x, y } => {
                sub.position = (x, y);
                "set_position"
            }
            wl_subsurface::Request::PlaceBelow { sibling } => {
                sub.below = Some(sibling.id());
                "place_below"
            }
            wl_subsurface::Request::SetSync => {
                sub.sync = true;
                "set_sync"
            }
            wl_subsurface::Request::SetDesync => {
                sub.sync = false;
                "set_desync"
            }
            wl_subsurface::Request::Destroy => {
                sub.destroyed = true;
                "destroy subsurface"
            }
            _ => "other subsurface request",
        };
        record.log.push(name);
    }
}

impl Dispatch<WpViewporter, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpViewporter,
        request: wp_viewporter::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_viewporter::Request::GetViewport { id, surface } => {
                let viewport = data_init.init(id, surface.id());
                if let Some(record) = state.recorded().surface_mut(&surface.id()) {
                    record.viewport = Some(viewport);
                }
            }
            wp_viewporter::Request::Destroy => state.recorded().unbound("wp_viewporter"),
            _ => {}
        }
    }
}

impl Dispatch<WpViewport, ObjectId> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WpViewport,
        request: wp_viewport::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut recorded = state.recorded();
        let recorded = &mut *recorded;
        match request {
            wp_viewport::Request::SetSource {
                x,
                y,
                width,
                height,
            } => {
                let unset = [x, y, width, height] == [-1.0; 4];
                if !unset && (x < 0.0 || y < 0.0 || width <= 0.0 || height <= 0.0) {
                    recorded.error(
                        resource,
                        1,
                        format!("set_source({x}, {y}, {width}, {height})"),
                    );
                    return;
                }
                if let Some(record) = recorded.surface_mut(surface) {
                    record.log.push("set_source");
                    record.pending.source = Some((!unset).then_some([x, y, width, height]));
                }
            }
            wp_viewport::Request::SetDestination { width, height } => {
                let unset = (width, height) == (-1, -1);
                if !unset && (width <= 0 || height <= 0) {
                    recorded.error(resource, 1, format!("set_destination({width}, {height})"));
                    return;
                }
                if let Some(record) = recorded.surface_mut(surface) {
                    record.log.push("set_destination");
                    record.pending.destination = Some((!unset).then_some((width, height)));
                }
            }
            wp_viewport::Request::Destroy => {
                // As wlroots: the source and destination go with the viewport.
                if let Some(record) = recorded.surface_mut(surface) {
                    record.log.push("destroy viewport");
                    record.viewport = None;
                    record.pending.source = Some(None);
                    record.pending.destination = Some(None);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlShm, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlShm,
        request: wl_shm::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let wl_shm::Request::CreatePool { id, fd, size } = request else {
            return;
        };
        let mut recorded = state.recorded();
        let file_size = rustix::fs::fstat(&fd)
            .map(|stat| stat.st_size)
            .unwrap_or(-1);
        if size <= 0 || file_size < i64::from(size) {
            recorded.error(
                resource,
                2,
                format!("a pool of {size} in a file of {file_size}"),
            );
            return;
        }
        let pool = data_init.init(id, ());
        recorded.pools.insert(pool.id(), size);
    }
}

impl Dispatch<WlShmPool, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlShmPool,
        request: wl_shm_pool::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let mut recorded = state.recorded();
        let size = recorded
            .pools
            .get(&resource.id())
            .copied()
            .unwrap_or_default();
        match request {
            wl_shm_pool::Request::CreateBuffer {
                id,
                offset,
                width,
                height,
                stride,
                ..
            } => {
                if offset < 0
                    || width <= 0
                    || height <= 0
                    || stride < width * 4
                    || i64::from(offset) + i64::from(stride) * i64::from(height) > i64::from(size)
                {
                    recorded.error(resource, 1, "a buffer outside its pool".to_owned());
                    return;
                }
                let buffer = data_init.init(id, ());
                recorded.buffers.insert(
                    buffer.id(),
                    BufferRecord {
                        width,
                        height,
                        dmabuf: false,
                        resource: buffer,
                        destroyed: false,
                    },
                );
            }
            wl_shm_pool::Request::Resize { size: new } => {
                if new < size {
                    recorded.error(resource, 1, "a pool shrunk".to_owned());
                    return;
                }
                recorded.pools.insert(resource.id(), new);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &WlBuffer,
        request: wl_buffer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_buffer::Request::Destroy = request
            && let Some(buffer) = state.recorded().buffers.get_mut(&resource.id())
        {
            buffer.destroyed = true;
        }
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwpLinuxDmabufV1,
        request: zwp_linux_dmabuf_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                let params = data_init.init(params_id, ());
                state.recorded().params.insert(params.id(), 0);
            }
            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id }
            | zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id, .. } => {
                send_feedback(&data_init.init(id, ()));
            }
            _ => {}
        }
    }
}

/// One tranche with one format, XRGB8888 without modifier, on a made-up device.
fn send_feedback(feedback: &ZwpLinuxDmabufFeedbackV1) {
    let table: OwnedFd =
        rustix::fs::memfd_create("fake-format-table", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut entry = Vec::new();
    entry.extend(u32::from_le_bytes(*b"XR24").to_ne_bytes());
    entry.extend(0u32.to_ne_bytes());
    entry.extend(0u64.to_ne_bytes());
    rustix::io::write(&table, &entry).unwrap();
    let device = 0xe280_u64.to_ne_bytes().to_vec();
    feedback.format_table(table.as_fd(), entry.len() as u32);
    feedback.main_device(device.clone());
    feedback.tranche_target_device(device);
    feedback.tranche_formats(0u16.to_ne_bytes().to_vec());
    feedback.tranche_flags(zwp_linux_dmabuf_feedback_v1::TrancheFlags::empty());
    feedback.tranche_done();
    feedback.done();
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for HostState {
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
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for HostState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
        _: &(),
        handle: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut recorded = state.recorded();
        match request {
            zwp_linux_buffer_params_v1::Request::Add { plane_idx, .. } => {
                if plane_idx >= 4 {
                    recorded.error(resource, 1, format!("plane {plane_idx}"));
                    return;
                }
                *recorded.params.entry(resource.id()).or_default() += 1;
            }
            zwp_linux_buffer_params_v1::Request::Create { width, height, .. } => {
                if recorded.dmabufs_fail {
                    resource.failed();
                    return;
                }
                let buffer = client
                    .create_resource::<WlBuffer, (), HostState>(handle, 1, ())
                    .unwrap();
                resource.created(&buffer);
                recorded.buffers.insert(
                    buffer.id(),
                    BufferRecord {
                        width,
                        height,
                        dmabuf: true,
                        resource: buffer,
                        destroyed: false,
                    },
                );
            }
            zwp_linux_buffer_params_v1::Request::CreateImmed { .. } => {
                recorded.immediate_dmabufs += 1;
                recorded.error(
                    resource,
                    7,
                    "create_immed reached the compositor".to_owned(),
                );
            }
            zwp_linux_buffer_params_v1::Request::Destroy => {
                recorded.params.remove(&resource.id());
            }
            _ => {}
        }
    }
}

impl Dispatch<WpPresentation, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpPresentation,
        request: wp_presentation::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_presentation::Request::Feedback { surface, callback } = request {
            let feedback = data_init.init(callback, ());
            if let Some(record) = state.recorded().surface_mut(&surface.id()) {
                record.log.push("feedback");
                record.pending.feedbacks.push(feedback);
            }
        }
    }
}

impl Dispatch<WpPresentationFeedback, ()> for HostState {
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
}

impl Dispatch<WlOutput, ()> for HostState {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlOutput,
        _: wl_output::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpFractionalScaleManagerV1,
        request: wp_fractional_scale_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_fractional_scale_manager_v1::Request::GetFractionalScale { id, .. } = request {
            let object = data_init.init(id, ());
            let mut recorded = state.recorded();
            if !recorded.fractional_scale_later {
                // 1.5, in 120ths.
                object.preferred_scale(180);
            }
            recorded.fractional_scales.push(object);
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for HostState {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WpFractionalScaleV1,
        _: <WpFractionalScaleV1 as Resource>::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WpFifoManagerV1, ()> for HostState {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WpFifoManagerV1,
        request: wp_fifo_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_fifo_manager_v1::Request::GetFifo { id, surface } = request {
            data_init.init(id, surface.id());
        }
    }
}

/// A fifo's requests go to its surface's log, in order with the surface's own.
impl Dispatch<WpFifoV1, ObjectId> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpFifoV1,
        request: wp_fifo_v1::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let name = match request {
            wp_fifo_v1::Request::SetBarrier => "set_barrier",
            wp_fifo_v1::Request::WaitBarrier => "wait_barrier",
            _ => return,
        };
        if let Some(surface) = state.recorded().surface_mut(surface) {
            surface.log.push(name);
        }
    }
}

impl Dispatch<WpContentTypeManagerV1, ()> for HostState {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WpContentTypeManagerV1,
        request: wp_content_type_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_content_type_manager_v1::Request::GetSurfaceContentType { id, .. } = request {
            data_init.init(id, ());
        }
    }
}

impl Dispatch<WpContentTypeV1, ()> for HostState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpContentTypeV1,
        request: wp_content_type_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wp_content_type_v1::Request::SetContentType {
            content_type: WEnum::Value(value),
        } = request
        {
            state.recorded().content_types.push(value.into());
        }
    }
}
