// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The relay: mpv's picture in a plane of the interface's window, on Linux ([ADR-0015]).
//!
//! **Two connections.** mpv speaks Wayland to the relay only, over one end of a socket pair it
//! inherits (`WAYLAND_SOCKET`); the relay's side is a Wayland server in pure Rust. The relay
//! speaks to the compositor on the interface's own connection (the window's), through a queue of
//! its own on it (libwayland, joined from the `wl_display` the interface hands over).
//!
//! **Nothing of mpv's is forwarded as it is.** The relay keeps the state of each of mpv's objects,
//! checks each request against what the compositor would refuse ([`check`]), and issues its own
//! requests on the window's connection. A refused request ends mpv's connection with a protocol
//! error, never the window's: a compositor's protocol error closes the whole connection it is
//! raised on. Buffers pass as file descriptors and are never mapped here.
//!
//! **What mpv sees.** A list of globals, not a filter: `wl_compositor`, `wl_shm`,
//! `zwp_linux_dmabuf_v1` (version 4, its feedback forwarded), `wp_viewporter`,
//! `wp_fractional_scale_manager_v1`, `wp_presentation`, `wp_content_type_manager_v1` when the
//! compositor offers them, each `wl_output` mirrored, and `xdg_wm_base`, which the relay plays
//! itself ([`shell`]). No seat, subcompositor, data device, idle inhibitor, decoration or sync
//! object: without `wp_linux_drm_syncobj_manager_v1`, Mesa relies on the dma-bufs' implicit
//! fences.
//!
//! **The plane.** mpv's toplevel surface is shown on a surface the relay made at start: a
//! subsurface of the window's parent surface, placed below the surface the interface named, its
//! input region empty so that the pointer reaches the interface, with a viewport the relay owns.
//! mpv's other surfaces stay unmapped. See [`surface`] for how mpv's commits and the interface's
//! changes reach that surface.
//!
//! **mpv's socket is read by the relay itself** ([`socket`]), which counts every request, registry
//! and file descriptor there before its Wayland server parses them. One wake-up thus handles a
//! bounded part of what mpv sends, whatever it sends.
//!
//! **One thread**, `aerith-relay`, asleep in `poll` on mpv's socket, the relay's socket pair to its
//! server, the window's connection and a stop signal. It follows libwayland's protocol for reading
//! a shared connection: it prepares to read its own queue, polls, and reads or cancels the moment
//! `poll` returns, so that another thread reading the same connection, the interface's, never waits
//! on it longer than that one read. It holds the state's lock only while it handles what it read,
//! never while it sleeps or reads. It wakes when mpv sends something, when the compositor sends
//! anything on the window's connection, the interface's own events included (the queues share
//! its socket), and once after each change of the interface's that mpv is told of. When mpv
//! draws nothing and the compositor sends nothing, it wakes for nothing.
//!
//! [ADR-0015]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0015-a-relay-for-mpvs-wayland-surface.md

mod check;
mod host;
mod serve;
mod shell;
mod socket;
mod surface;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io;
use std::net::Shutdown;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use rustix::event::{PollFd, PollFlags};
use wayland_client::backend::{Backend, ObjectId as HostId, WaylandError};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_buffer::WlBuffer as HostBuffer;
use wayland_client::protocol::wl_output::WlOutput as HostOutput;
use wayland_client::protocol::wl_surface::WlSurface as HostSurface;
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::content_type::v1::client::wp_content_type_v1::WpContentTypeV1 as HostContentType;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1 as HostFractionalScale;
use wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_v1::WpFractionalScaleV1 as MpvFractionalScale;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 as HostParams;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 as MpvParams;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1 as MpvDmabufFeedback;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport as HostViewport;
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, GlobalId, ObjectId};
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::{Display, DisplayHandle};

use crate::plane::{Presentation, Window};
use host::{FeedbackCache, HostGlobals};
use socket::{MpvSocket, Read};
use surface::{PlaneState, Surface};

/// The side facing mpv is wayland-server's backend in pure Rust, never libwayland-server
/// ([ADR-0015]): this compiles only while the two are the same type, that is while
/// `wayland-backend`'s `server_system` feature is off, whatever crate turns it on.
///
/// [ADR-0015]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0015-a-relay-for-mpvs-wayland-surface.md
const _: fn(
    wayland_backend::rs::server::Backend<State>,
) -> wayland_server::backend::Backend<State> = |backend| backend;

/// The most objects mpv may hold at once on its connection, registries apart
/// ([`socket::MOST_REGISTRIES`]). mpv and Mesa use about a hundred.
const MOST_OBJECTS: usize = 4096;

/// The most file descriptors the relay's Wayland server may hold queued for mpv after a wake-up:
/// those mpv sent with requests that take none, and those that came ahead of their request.
/// libwayland sends at most 28 with a message, which a message cut between two reads may leave
/// waiting; twice that, rounded up, is no client's normal use.
const MOST_QUEUED_FDS: usize = 64;

/// The relay as the player holds it: the shared state and its thread.
pub(crate) struct Relay {
    shared: Arc<Mutex<State>>,
    /// The relay's thread, until the plane is closed.
    thread: Mutex<Option<Running>>,
}

/// The thread, and the end of the socket pair that stops it.
struct Running {
    thread: JoinHandle<()>,
    stop: UnixStream,
}

/// Everything the relay knows, behind one lock: the relay's thread holds it while it handles a
/// wake-up, the interface's thread while it places, shows or hides the plane. So neither ever
/// issues half of the other's change on the window's connection.
pub(super) struct State {
    /// What the relay made on the window's connection.
    host: Host,
    qh: QueueHandle<State>,
    /// mpv's side.
    display: DisplayHandle,
    /// mpv's objects, by their id on mpv's connection.
    surfaces: HashMap<ObjectId, Surface>,
    buffers: HashMap<ObjectId, Buffer>,
    pools: HashMap<ObjectId, Pool>,
    params: HashMap<ObjectId, Params>,
    /// The mpv buffer each buffer made on the window's connection stands for.
    host_buffers: HashMap<HostId, ObjectId>,
    /// What each dmabuf parameters object made on the window's connection was asked to make.
    host_params: HashMap<HostId, ParamsUse>,
    /// The outputs of the window's compositor, each mirrored to mpv.
    outputs: Vec<Output>,
    /// mpv's dmabuf feedback objects, and the surface each asked for (none for the default one).
    feedbacks: Vec<(MpvDmabufFeedback, Option<ObjectId>)>,
    /// mpv's fractional scale objects, and their surface.
    fractional_scales: Vec<(MpvFractionalScale, ObjectId)>,
    plane: PlaneState,
    /// mpv's toplevel, the plane's, and the last configure serial sent.
    toplevel: Option<shell::Toplevel>,
    serial: u32,
    presentation: Presentation,
    /// The refresh the last frame was presented at.
    last_msc: Option<u64>,
    /// mpv's live objects and the pools' file descriptors held, against their ceilings.
    objects: usize,
    fds: usize,
    /// The file descriptors the relay's handlers took from mpv's requests, kept or not: the
    /// others mpv sent wait in the server's queue ([`MOST_QUEUED_FDS`]).
    fds_taken: usize,
    /// The relay thread's wake-ups, for the tests that prove it sleeps.
    #[cfg(test)]
    wake_ups: u64,
    /// Whether a memory file passes as a dma-buf: only the tests and the fuzz target set it, as
    /// no machine without a GPU can make a dma-buf ([`check::dmabuf_size`]).
    memory_stands_for_dmabuf: bool,
}

/// What the relay made on the window's connection.
pub(super) struct Host {
    /// The relay's handle on the window's connection; `None` once the plane is closed.
    connection: Option<Connection>,
    globals: HostGlobals,
    /// The plane's surface, a subsurface of the window's parent surface.
    surface: HostSurface,
    subsurface: wayland_client::protocol::wl_subsurface::WlSubsurface,
    viewport: HostViewport,
    fractional_scale: Option<HostFractionalScale>,
    content_type: Option<HostContentType>,
    /// The compositor's dmabuf feedback, the default one and the plane surface's, as last sent.
    default_feedback: FeedbackCache,
    surface_feedback: FeedbackCache,
    /// What the compositor last said of the plane's surface: its preferred scales and transform,
    /// and the outputs it is on.
    preferred_scale: Option<u32>,
    preferred_buffer_scale: Option<i32>,
    preferred_buffer_transform: Option<u32>,
    entered: Vec<HostOutput>,
}

/// One of mpv's buffers.
pub(super) struct Buffer {
    /// The buffer made on the window's connection; `None` until the compositor made it, for a
    /// dmabuf mpv asked for with `create_immed`.
    host: Option<HostBuffer>,
    /// Its size in pixels.
    size: (i32, i32),
    /// How many of mpv's surfaces hold it in their committed state.
    attached: u32,
    /// Whether the compositor holds it: attached on the window's connection and not released.
    host_busy: bool,
    /// Whether mpv gave it to a surface since it was last released to mpv.
    owed: bool,
}

/// One of mpv's shared-memory pools.
pub(super) struct Pool {
    host: wayland_client::protocol::wl_shm_pool::WlShmPool,
    /// The pool's file, kept to check a resize against it.
    fd: OwnedFd,
    size: i32,
}

/// One of mpv's dmabuf parameters objects.
pub(super) struct Params {
    host: HostParams,
    planes: [Option<check::Plane>; check::MOST_PLANES],
    modifier: Option<u64>,
    used: bool,
}

/// What a dmabuf parameters object on the window's connection was asked to make. It is kept, and
/// destroyed once the compositor answered: an object destroyed before would lose the answer.
pub(super) struct ParamsUse {
    pub(super) host: HostParams,
    pub(super) made: Made,
}

/// The buffer a dmabuf parameters object makes for mpv.
pub(super) enum Made {
    /// A buffer that mpv gets when the compositor made it: mpv's parameters object, and the size.
    Later { params: MpvParams, size: (i32, i32) },
    /// The buffer mpv already holds (`create_immed`).
    Already(ObjectId),
}

/// One output of the window's compositor, mirrored to mpv.
pub(super) struct Output {
    /// Its global's name on the window's connection.
    name: u32,
    host: HostOutput,
    /// What the compositor said of it, and whether it said all of it once (`done`).
    info: host::OutputInfo,
    complete: bool,
    /// The global offered to mpv, once the output is complete.
    global: Option<GlobalId>,
    /// The description mpv's objects were last told, sent again only when it changes.
    published_description: Option<String>,
    /// mpv's objects for it.
    resources: Vec<WlOutput>,
}

/// How mpv's connection ended, as its connection's data records it.
#[derive(Default)]
struct MpvClient {
    ended: Mutex<Option<DisconnectReason>>,
}

impl ClientData for MpvClient {
    fn disconnected(&self, _client: ClientId, reason: DisconnectReason) {
        *self.ended.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
    }
}

impl Relay {
    /// Joins the window's connection, makes the plane's surface on it, hidden, and starts the
    /// relay's thread, which serves mpv on `mpv`. `failed` is called once, from the relay's
    /// thread, if the relay ends mpv's connection or loses the window's. A start that fails leaves
    /// nothing of the relay's on the window's connection but what the protocol cannot destroy: a
    /// registry, and the `wl_compositor` once bound.
    pub(crate) fn start(
        window: Window,
        mpv: UnixStream,
        failed: Box<dyn Fn(String) + Send>,
    ) -> io::Result<Self> {
        // SAFETY: `Window::new`'s contract keeps the display alive until the plane is closed, and
        // the relay drops this backend when the plane closes (`close`), before the interface
        // may destroy the display. A guest backend never closes the display itself.
        let backend = unsafe { Backend::from_foreign_display(window.display().cast()) };
        let connection = Connection::from_backend(backend);
        // What can fail before anything is made on the window's connection, first.
        let parent = window_surface(&connection, window.parent())?;
        let below = window_surface(&connection, window.below())?;
        let display: Display<State> =
            Display::new().map_err(|error| io::Error::other(error.to_string()))?;
        let (socket, server_end) = MpvSocket::new(mpv)?;
        let (stop, stopped) = UnixStream::pair()?;
        let (globals, mut queue) = registry_queue_init::<State>(&connection)
            .map_err(|error| io::Error::other(format!("the window's registry: {error}")))?;
        let qh = queue.handle();
        let host_globals = HostGlobals::bind(&globals, &qh)?;
        let host = Host::make(connection, host_globals, &qh, &parent, &below);
        let mut state = State {
            host,
            qh: qh.clone(),
            display: display.handle(),
            surfaces: HashMap::new(),
            buffers: HashMap::new(),
            pools: HashMap::new(),
            params: HashMap::new(),
            host_buffers: HashMap::new(),
            host_params: HashMap::new(),
            outputs: Vec::new(),
            feedbacks: Vec::new(),
            fractional_scales: Vec::new(),
            plane: PlaneState::default(),
            toplevel: None,
            serial: 0,
            presentation: Presentation::default(),
            last_msc: None,
            objects: 0,
            fds: 0,
            fds_taken: 0,
            #[cfg(test)]
            wake_ups: 0,
            memory_stands_for_dmabuf: false,
        };
        state.watch_outputs(&globals);
        // What the compositor sends in answer to the binds: shared-memory formats, outputs, the
        // presentation clock, dmabuf feedback. mpv is offered it at once when it binds, so it is
        // read before mpv may ask; a compositor that dies here fails the start.
        if let Err(error) = queue.roundtrip(&mut state) {
            state.destroy_host_objects();
            return Err(io::Error::other(format!(
                "the window's connection failed: {error}"
            )));
        }
        state.offer_globals();
        let client = Arc::new(MpvClient::default());
        let mpv_client = match state.display.insert_client(server_end, client.clone()) {
            Ok(mpv_client) => mpv_client.id(),
            Err(error) => {
                state.destroy_host_objects();
                return Err(error);
            }
        };
        let shared = Arc::new(Mutex::new(state));
        let serving = Serving {
            shared: Arc::clone(&shared),
            display,
            queue,
            stopped,
            socket,
            client,
            mpv_client,
            failed,
            reported: false,
            mpv_hung_up: false,
        };
        let thread = thread::Builder::new()
            .name("aerith-relay".to_owned())
            .spawn(move || serving.run());
        match thread {
            Ok(thread) => Ok(Self {
                shared,
                thread: Mutex::new(Some(Running { thread, stop })),
            }),
            Err(error) => {
                lock(&shared).destroy_host_objects();
                Err(error)
            }
        }
    }

    /// Runs `change` on the state, unless the plane is closed, then sends mpv what it was told.
    pub(crate) fn change(&self, change: impl FnOnce(&mut State)) {
        let mut state = lock(&self.shared);
        if state.plane.closed {
            return;
        }
        change(&mut state);
        // A failed flush is mpv's connection breaking; the relay's thread sees it end.
        let _ = state.display.flush_clients();
    }

    /// What the compositor reported of mpv's frames so far.
    pub(crate) fn presentation(&self) -> Presentation {
        lock(&self.shared).presentation
    }

    /// Whether the plane is closed, for the tests.
    #[cfg(test)]
    pub(crate) fn closed(&self) -> bool {
        lock(&self.shared).plane.closed
    }

    /// The relay thread's wake-ups so far, for the tests that prove it sleeps.
    #[cfg(test)]
    pub(crate) fn wake_ups(&self) -> u64 {
        lock(&self.shared).wake_ups
    }

    /// Stops the relay's thread, then destroys everything the relay made on the window's
    /// connection, from the calling thread, and leaves the connection to the interface. mpv's
    /// connection closes. A call made while another closes returns once that one is done; later
    /// calls do nothing.
    pub(crate) fn close(&self) {
        // Held until the end, so that a second caller waits for the destruction.
        let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(running) = thread.take() else { return };
        let _ = running.stop.shutdown(Shutdown::Both);
        // A thread that panicked left the state as it was; it is destroyed all the same.
        let _ = running.thread.join();
        let mut state = lock(&self.shared);
        state.destroy_host_objects();
        state.plane.closed = true;
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.close();
    }
}

/// The state, even if a thread panicked holding it: each change leaves it whole.
fn lock(shared: &Mutex<State>) -> MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A surface of the interface's, from its libwayland pointer, as an object of the relay's
/// connection.
fn window_surface(
    connection: &Connection,
    surface: *mut std::ffi::c_void,
) -> io::Result<HostSurface> {
    // SAFETY: `Window::new`'s contract makes `surface` a live `wl_surface` of the window's
    // connection until the plane is closed; the relay uses it only before then.
    let id = unsafe { HostId::from_ptr(HostSurface::interface(), surface.cast()) }
        .map_err(|_| io::Error::other("a window surface is not a wl_surface"))?;
    HostSurface::from_id(connection, id)
        .map_err(|_| io::Error::other("a window surface is not alive"))
}

/// What the relay's thread owns.
struct Serving {
    shared: Arc<Mutex<State>>,
    display: Display<State>,
    queue: EventQueue<State>,
    /// Readable once the plane closes.
    stopped: UnixStream,
    /// mpv's socket, and the pair to the server.
    socket: MpvSocket,
    client: Arc<MpvClient>,
    mpv_client: ClientId,
    failed: Box<dyn Fn(String) + Send>,
    /// Whether `failed` was called.
    reported: bool,
    /// Whether mpv closed its end of its socket.
    mpv_hung_up: bool,
}

/// What woke the relay's thread.
struct Woken {
    stop: bool,
    host: PollFlags,
    mpv: PollFlags,
    server: PollFlags,
}

impl Serving {
    /// Serves mpv and the window's connection until the plane closes.
    fn run(mut self) {
        loop {
            // Events read already, by this thread or the interface's, are handled first: libwayland
            // refuses to prepare a read while the relay's queue holds some.
            let guard = loop {
                let dispatched = self.queue.dispatch_pending(&mut lock(&self.shared));
                if let Err(error) = dispatched {
                    return self.window_failed(&error.to_string());
                }
                if let Some(guard) = self.queue.prepare_read() {
                    break guard;
                }
            };
            self.settle_mpv_end();
            let host_flushed = match self.queue.flush() {
                Ok(()) => true,
                Err(WaylandError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => false,
                Err(error) => {
                    drop(guard);
                    return self.window_failed(&error.to_string());
                }
            };
            // What the relay's server has for mpv goes out before the relay sleeps.
            let _ = self.display.flush_clients();
            self.socket.write_mpv();
            let woken = self.wait(guard.connection_fd(), host_flushed);
            let woken = match woken {
                Ok(woken) => woken,
                Err(error) => {
                    drop(guard);
                    return self.window_failed(&format!("poll failed: {error}"));
                }
            };
            #[cfg(test)]
            {
                lock(&self.shared).wake_ups += 1;
            }
            // Read or cancel at once: the interface's thread may be waiting on this read.
            if woken
                .host
                .intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP)
            {
                if let Err(error) = guard.read() {
                    let would_block = matches!(&error, WaylandError::Io(e) if e.kind() == io::ErrorKind::WouldBlock);
                    if !would_block {
                        return self.window_failed(&error.to_string());
                    }
                }
            } else {
                drop(guard);
            }
            if woken.stop {
                return self.close_mpv();
            }
            // The server closed its end while mpv's socket is full: mpv is not reading what is
            // left, the end of its connection, and the relay does not wait for it.
            if woken.server.intersects(PollFlags::ERR | PollFlags::HUP)
                && self.socket.waits_for_mpv()
            {
                self.socket.close();
            }
            if !woken.mpv.is_empty() || !woken.server.is_empty() {
                self.serve_mpv();
            }
        }
    }

    /// Sleeps until the plane closes, the window's connection has something (`host_fd`), or
    /// mpv's side does: mpv sent something, its socket can take what waits for it, or the server
    /// sent mpv something.
    fn wait(&self, host_fd: BorrowedFd<'_>, host_flushed: bool) -> io::Result<Woken> {
        let mut host_events = PollFlags::IN;
        if !host_flushed {
            host_events |= PollFlags::OUT;
        }
        // While the window's connection cannot take more, mpv's requests wait in its socket: mpv
        // slows down rather than the relay buffering without end.
        let mut mpv_events = PollFlags::empty();
        if host_flushed && !self.socket.waits_for_server() {
            mpv_events |= PollFlags::IN;
        }
        if self.socket.waits_for_mpv() {
            mpv_events |= PollFlags::OUT;
        }
        let server_events = if self.socket.waits_for_mpv() {
            PollFlags::empty()
        } else {
            PollFlags::IN
        };
        let mut fds = Vec::with_capacity(4);
        fds.push(PollFd::new(&self.stopped, PollFlags::IN));
        fds.push(PollFd::from_borrowed_fd(host_fd, host_events));
        // A socket closed is left out: `poll` reports a hang-up even when asked for nothing.
        let mpv = self.socket.mpv().map(|mpv| {
            fds.push(PollFd::from_borrowed_fd(mpv, mpv_events));
            fds.len() - 1
        });
        let server = self.socket.server().map(|server| {
            fds.push(PollFd::from_borrowed_fd(server, server_events));
            fds.len() - 1
        });
        poll_until_ready(&mut fds)?;
        let revents = |index: Option<usize>| index.map_or(PollFlags::empty(), |i| fds[i].revents());
        Ok(Woken {
            stop: !fds[0].revents().is_empty(),
            host: fds[1].revents(),
            mpv: revents(mpv),
            server: revents(server),
        })
    }

    /// Reads what mpv sent, has the server handle it, sends mpv the answers, and reports once if
    /// mpv's connection was ended for breaking a rule.
    ///
    /// mpv may write on while the server handles what was read: the wake-up goes on, its counts
    /// with it, until a read finds nothing more. A wake-up thus ends within the ceilings of
    /// [`socket`], and the state's lock is let go between two parts of it.
    fn serve_mpv(&mut self) {
        self.socket.start_wake_up();
        loop {
            let read = self.socket.read_mpv();
            match &read {
                Read::Refused(reason) => {
                    lock(&self.shared).end_mpv(self.mpv_client.clone(), reason.clone());
                }
                Read::HungUp => self.mpv_hung_up = true,
                Read::Nothing | Read::Passed | Read::ServerFull => {}
            }
            self.dispatch();
            let _ = self.display.flush_clients();
            self.socket.write_mpv();
            if !matches!(read, Read::Passed | Read::ServerFull) {
                break;
            }
        }
        let taken = lock(&self.shared).fds_taken;
        let queued = self.socket.counts.fds_received.saturating_sub(taken);
        if queued > MOST_QUEUED_FDS {
            lock(&self.shared).end_mpv(
                self.mpv_client.clone(),
                format!(
                    "mpv had the relay hold more than {MOST_QUEUED_FDS} files that no request took"
                ),
            );
            self.dispatch();
            self.socket.write_mpv();
        }
        self.report_failure();
    }

    /// Has the server read and handle what reached its socket. An error is mpv's connection
    /// ending, which the server has seen already.
    fn dispatch(&mut self) {
        let mut state = lock(&self.shared);
        let _ = self
            .display
            .backend()
            .dispatch_single_client(&mut state, self.mpv_client.clone());
    }

    /// A handler of the window's events may have ended mpv's connection (a dmabuf the compositor
    /// could not import after mpv was given it): the server lets mpv's objects go, mpv sees the
    /// end, and the failure is reported.
    fn settle_mpv_end(&mut self) {
        let ended = self
            .client
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if ended {
            self.dispatch();
            self.socket.write_mpv();
            self.report_failure();
        }
    }

    /// Calls `failed` once, if mpv's connection was ended other than by mpv itself: for breaking
    /// a rule, or by the server, for a malformed message or for answers mpv left unread.
    fn report_failure(&mut self) {
        let ended = self
            .client
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let reason = match ended {
            Some(DisconnectReason::ProtocolError(error)) => format!(
                "the relay ended mpv's Wayland connection: {} ({}, code {})",
                error.message, error.object_interface, error.code
            ),
            Some(DisconnectReason::ConnectionClosed) if !self.mpv_hung_up => {
                "the relay's Wayland server closed mpv's connection: a malformed message, or \
                 answers mpv did not read"
                    .to_owned()
            }
            _ => return,
        };
        if !self.reported {
            self.reported = true;
            (self.failed)(reason);
        }
    }

    /// Closes mpv's connection as the plane closes: its objects are let go, their doubles on the
    /// window's connection destroyed, and its socket closed, so that mpv sees the end at once.
    fn close_mpv(&mut self) {
        self.display
            .handle()
            .backend_handle()
            .kill_client(self.mpv_client.clone(), DisconnectReason::ConnectionClosed);
        // The dispatch of a client ended frees it; its error is that end.
        self.dispatch();
        // Events read last are handled, not left in the relay's queue: an event left there holds
        // its object, which holds the queue, and neither would be freed.
        let _ = self.queue.dispatch_pending(&mut lock(&self.shared));
        let _ = self.queue.flush();
        self.socket.close();
    }

    /// The window's connection failed: mpv can no longer be shown. Its connection is closed, and
    /// the failure reported; the thread then waits for the plane to close.
    fn window_failed(mut self, error: &str) {
        if !self.reported {
            self.reported = true;
            (self.failed)(format!("the window's Wayland connection failed: {error}"));
        }
        self.close_mpv();
        // Nothing more can be done on a broken connection but wait to be closed.
        let mut fds = [PollFd::new(&self.stopped, PollFlags::IN)];
        let _ = poll_until_ready(&mut fds);
    }
}

/// `poll` without a timeout, again when a signal interrupts it.
fn poll_until_ready(fds: &mut [PollFd<'_>]) -> io::Result<()> {
    loop {
        match rustix::event::poll(fds, None) {
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

impl State {
    /// Counts one of mpv's objects, created by the request being handled, and ends mpv's
    /// connection past the ceiling. Returns whether to make it.
    pub(super) fn admit_object(&mut self, client: &wayland_server::Client) -> bool {
        if self.objects >= MOST_OBJECTS {
            self.end_mpv(
                client.id(),
                format!("mpv made more than {MOST_OBJECTS} objects"),
            );
            return false;
        }
        self.objects += 1;
        true
    }

    /// Counts an object of mpv's gone.
    pub(super) fn object_gone(&mut self) {
        self.objects = self.objects.saturating_sub(1);
    }

    /// Ends mpv's connection where no protocol error fits: a ceiling broken, or a dmabuf the
    /// compositor could not import after mpv was given it. mpv sees its connection close, and the
    /// player reports `reason`. The handlers' other refusals raise the protocol's own errors.
    pub(super) fn end_mpv(&self, client: ClientId, reason: String) {
        self.display.backend_handle().kill_client(
            client,
            DisconnectReason::ProtocolError(wayland_server::backend::protocol::ProtocolError {
                // wl_display's `no_memory`.
                code: 2,
                object_id: 1,
                object_interface: "wl_display".to_owned(),
                message: reason,
            }),
        );
    }

    /// Lets a memory file pass as a dma-buf ([`check::dmabuf_size`]), for the tests and the fuzz
    /// target, which no machine without a GPU can give a dma-buf.
    #[cfg(any(test, feature = "fuzzing"))]
    pub(crate) fn take_memory_as_dmabuf(&mut self) {
        self.memory_stands_for_dmabuf = true;
    }

    /// A handler took a file descriptor from one of mpv's requests.
    pub(super) fn took_fd(&mut self) {
        self.fds_taken += 1;
    }

    /// The file descriptor of a new pool, against the ceiling. Returns whether to keep it.
    pub(super) fn admit_fd(&mut self, client: &wayland_server::Client) -> bool {
        if self.fds >= check::MOST_POOL_FDS {
            self.end_mpv(
                client.id(),
                format!(
                    "mpv had the relay hold more than {} files",
                    check::MOST_POOL_FDS
                ),
            );
            return false;
        }
        self.fds += 1;
        true
    }

    /// Destroys everything the relay made on the window's connection, mpv's objects' doubles
    /// included, so that nothing of the relay's outlives the plane there. The interface's own
    /// objects are left alone.
    fn destroy_host_objects(&mut self) {
        for (_, buffer) in self.buffers.drain() {
            if let Some(host) = buffer.host {
                host.destroy();
            }
        }
        for (_, pool) in self.pools.drain() {
            pool.host.destroy();
        }
        for (_, params) in self.params.drain() {
            params.host.destroy();
        }
        for (_, waiting) in self.host_params.drain() {
            waiting.host.destroy();
        }
        self.host_buffers.clear();
        for output in self.outputs.drain(..) {
            host::release_output(&output.host);
        }
        self.host.destroy();
    }
}
