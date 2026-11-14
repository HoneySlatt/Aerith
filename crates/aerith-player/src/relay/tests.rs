// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The relay between a fake compositor (`tests/common/host.rs`, which the interface's connection
//! reaches) and a fake mpv, a Wayland client on the relay's socket. Each refusal is proven to end
//! mpv's connection only: the interface's connection answers a round trip afterwards, and the
//! fake compositor raised no error.

#[path = "../../tests/common/host.rs"]
mod host;

use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::content_type::v1::client::wp_content_type_manager_v1::WpContentTypeManagerV1;
use wayland_protocols::wp::content_type::v1::client::wp_content_type_v1::{self, WpContentTypeV1};
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
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

use super::rate::{INTERVALS, SETTLE};
use super::socket::{MOST_FDS_AT_ONCE, MOST_REGISTRIES, MOST_REQUESTS_AT_ONCE};
use super::{DisplayNews, MOST_OBJECTS, MOST_QUEUED_FDS, Relay};
use crate::plane::{DisplayRate, Plane, Window};
use host::{FakeHost, Interface, OUTPUT_SIZE};

const WAIT: Duration = Duration::from_secs(10);

/// What the fake mpv received.
#[derive(Default)]
struct Received {
    /// Each toplevel configure: its size and states.
    configures: Vec<(i32, i32, Vec<u32>)>,
    /// The serial of the last `xdg_surface.configure`.
    serial: Option<u32>,
    callbacks_done: u32,
    released: Vec<wayland_client::backend::ObjectId>,
    presented: u32,
    discarded: u32,
    shm_formats: Vec<u32>,
    created: Vec<WlBuffer>,
    failed: u32,
    feedback_batches: u32,
    format_table: Option<u32>,
    preferred_scale: Option<u32>,
    output_refresh: Option<i32>,
    /// Each `wl_output` event of these kinds received.
    output_names: u32,
    output_descriptions: u32,
    output_dones: u32,
    /// Globals the relay removed.
    globals_removed: u32,
    entered: u32,
    /// The surface's enter and scale events, by name, in order.
    surface_events: Vec<&'static str>,
    /// The toplevel, once configured.
    toplevel: Option<XdgToplevel>,
    /// The toplevel's `xdg_surface`, once configured.
    xdg_surface: Option<XdgSurface>,
}

/// mpv's side: a Wayland client on the relay's socket.
struct FakeMpv {
    connection: Connection,
    queue: EventQueue<Received>,
    received: Received,
    globals: GlobalList,
    qh: QueueHandle<Received>,
}

impl FakeMpv {
    fn connect(stream: UnixStream) -> Self {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, queue) = registry_queue_init::<Received>(&connection).unwrap();
        let qh = queue.handle();
        Self {
            connection,
            queue,
            received: Received::default(),
            globals,
            qh,
        }
    }

    /// A round trip: fails, with the relay's reason, once the relay ended the connection.
    fn roundtrip(&mut self) -> Result<(), String> {
        self.queue
            .roundtrip(&mut self.received)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn bind<I>(&self, version: u32) -> I
    where
        I: Proxy + 'static,
        Received: Dispatch<I, ()>,
    {
        self.globals.bind(&self.qh, version..=version, ()).unwrap()
    }

    fn global_names(&self) -> Vec<String> {
        self.globals
            .contents()
            .clone_list()
            .into_iter()
            .map(|global| global.interface)
            .collect()
    }

    /// A surface with the toplevel role, configured and acknowledged as mpv does it.
    fn toplevel(&mut self) -> (WlSurface, XdgSurface, XdgToplevel) {
        let compositor: WlCompositor = self.bind(6);
        let shell: XdgWmBase = self.bind(6);
        let surface = compositor.create_surface(&self.qh, ());
        let xdg_surface = shell.get_xdg_surface(&surface, &self.qh, ());
        let toplevel = xdg_surface.get_toplevel(&self.qh, ());
        surface.commit();
        self.roundtrip().unwrap();
        assert!(
            self.received.serial.is_some(),
            "a configure answers the first commit"
        );
        (surface, xdg_surface, toplevel)
    }

    /// A shared-memory buffer of `width` by `height` pixels, XRGB8888, its pool already destroyed,
    /// as Mesa makes them.
    fn shm_buffer(&self, width: i32, height: i32) -> WlBuffer {
        let shm: WlShm = self.bind(1);
        let size = width * height * 4;
        let fd = memory(size as u64);
        let pool = shm.create_pool(fd.as_fd(), size, &self.qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Xrgb8888,
            &self.qh,
            (),
        );
        pool.destroy();
        buffer
    }

    /// Commits `buffer` on `surface`, with a frame callback and presentation feedback.
    fn draw(&mut self, surface: &WlSurface, buffer: &WlBuffer) {
        let presentation: WpPresentation = self.bind(2);
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        surface.frame(&self.qh, ());
        presentation.feedback(surface, &self.qh, ());
        surface.commit();
        self.roundtrip().unwrap();
    }

    /// Waits until `found` holds of what mpv received.
    fn wait(&mut self, what: &str, found: impl Fn(&Received) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !found(&self.received) {
            assert!(Instant::now() < deadline, "mpv never received {what}");
            self.roundtrip().unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A memory file of `size` bytes.
fn memory(size: u64) -> OwnedFd {
    named_memory("relay-test", size)
}

/// A memory file of `size` bytes, named `name`, and open to seals.
fn named_memory(name: &str, size: u64) -> OwnedFd {
    let flags = rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING;
    let fd = rustix::fs::memfd_create(name, flags).unwrap();
    rustix::fs::ftruncate(&fd, size).unwrap();
    fd
}

/// What the relay reported as mpv's failure.
type Failures = Arc<Mutex<Vec<String>>>;

/// What the relay told of the display.
type News = Arc<Mutex<Vec<DisplayNews>>>;

/// The relay on `interface`'s window, a fake mpv connected to it, and what it reports.
fn relay(interface: &Interface) -> (Plane, FakeMpv, Failures) {
    let (plane, mpv, failures, _) = relay_telling(interface);
    (plane, mpv, failures)
}

/// [`relay`], and what the relay tells of the display.
fn relay_telling(interface: &Interface) -> (Plane, FakeMpv, Failures, News) {
    let (relay_end, mpv_end) = UnixStream::pair().unwrap();
    let failures = Failures::default();
    let recorded = Arc::clone(&failures);
    let news = News::default();
    let told = Arc::clone(&news);
    let (display, parent, below) = interface.handles();
    // SAFETY: the interface's display and surfaces outlive the plane: each test keeps the
    // interface until after the plane is closed or dropped.
    let window = unsafe { Window::new(display, parent, below) };
    let relay = Relay::start(
        window,
        relay_end,
        Box::new(move |reason| recorded.lock().unwrap().push(reason)),
        Box::new(move |news| told.lock().unwrap().push(news)),
    )
    .unwrap();
    (Plane::new(relay), FakeMpv::connect(mpv_end), failures, news)
}

/// Waits for the relay to report mpv's failure, and returns it.
fn failure(failures: &Failures) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(reason) = failures.lock().unwrap().first() {
            return reason.clone();
        }
        assert!(Instant::now() < deadline, "the relay reported no failure");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Proves that the relay ended mpv's connection for `expected`, and only it: the interface's
/// connection still answers, and the compositor raised no error.
fn ended_mpv_only(
    host: &FakeHost,
    interface: &mut Interface,
    mpv: &mut FakeMpv,
    failures: &Failures,
    expected: &str,
) {
    let reason = failure(failures);
    assert!(reason.contains(expected), "{reason}");
    let deadline = Instant::now() + WAIT;
    while mpv.roundtrip().is_ok() {
        assert!(Instant::now() < deadline, "mpv's connection stays open");
    }
    interface
        .roundtrip()
        .expect("the window's connection lives on");
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

/// A fake compositor, an interface with a subsurface of its own, a shown plane at 320x180, and a
/// fake mpv with its toplevel configured.
fn shown() -> (FakeHost, Interface, Plane, FakeMpv, Failures, WlSurface) {
    let host = FakeHost::start();
    let interface = host.connect(true);
    let (plane, mut mpv, failures) = relay(&interface);
    plane.place(10, 20, 320, 180).unwrap();
    plane.show();
    interface.commit();
    let (surface, _, _) = mpv.toplevel();
    (host, interface, plane, mpv, failures, surface)
}

#[test]
fn the_plane_is_a_subsurface_below_the_given_surface_that_takes_no_input() {
    let host = FakeHost::start();
    let mut interface = host.connect(true);
    let (_plane, _mpv, _failures) = relay(&interface);
    interface.roundtrip().unwrap();
    host.with(|recorded| {
        let plane = interface.plane(recorded).expect("the plane's surface");
        let subsurface = plane.subsurface.as_ref().unwrap();
        let (_, own) = interface.numbers();
        assert_eq!(
            subsurface.below.as_ref().map(|below| below.protocol_id()),
            own
        );
        assert!(
            !subsurface.sync,
            "desynchronized: mpv's frames need no commit of the interface's"
        );
        assert_eq!(
            plane.input_region,
            Some(Vec::new()),
            "an empty input region"
        );
        assert_eq!(plane.buffer, None, "nothing shows before mpv draws");
        assert!(recorded.errors.is_empty(), "{:?}", recorded.errors);
    });
}

#[test]
fn mpv_is_offered_a_list_of_globals_and_nothing_else() {
    let host = FakeHost::start();
    let interface = host.connect(false);
    let (_plane, mpv, _failures) = relay(&interface);
    let mut names = mpv.global_names();
    names.sort();
    assert_eq!(
        names,
        [
            "wl_compositor",
            "wl_output",
            "wl_shm",
            "wp_content_type_manager_v1",
            "wp_fifo_manager_v1",
            "wp_fractional_scale_manager_v1",
            "wp_presentation",
            "wp_viewporter",
            "xdg_wm_base",
            "zwp_linux_dmabuf_v1",
        ]
    );
}

#[test]
fn what_mpv_needs_at_once_is_there_when_it_binds() {
    let host = FakeHost::start();
    let interface = host.connect(false);
    let (_plane, mut mpv, _failures) = relay(&interface);
    let _shm: WlShm = mpv.bind(1);
    let output: WlOutput = mpv.bind(4);
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    dmabuf.get_default_feedback(&mpv.qh, ());
    // One round trip, as mpv and Mesa make: everything is answered within it.
    mpv.roundtrip().unwrap();
    assert_eq!(mpv.received.shm_formats, [0, 1]);
    assert_eq!(mpv.received.output_refresh, Some(host::REFRESH_MHZ));
    assert_eq!(mpv.received.feedback_batches, 1);
    assert_eq!(mpv.received.format_table, Some(16));
    drop(output);
}

#[test]
fn mpvs_frame_reaches_the_plane_with_the_planes_size_and_place() {
    let (host, mut interface, plane, mut mpv, _failures, surface) = shown();
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &buffer);
    host.wait("mpv's frame on the plane", |recorded| {
        let surface = interface.plane(recorded)?;
        (surface.buffer_commits >= 1).then_some(())
    });
    host.with(|recorded| {
        let surface = interface.plane(recorded).unwrap();
        let buffer = &recorded.buffers[surface.buffer.as_ref().unwrap()];
        assert_eq!(
            (buffer.width, buffer.height, buffer.dmabuf),
            (64, 48, false)
        );
        assert_eq!(
            surface.destination,
            Some((320, 180)),
            "the relay's viewport"
        );
        assert_eq!(surface.subsurface.as_ref().unwrap().position, (10, 20));
    });
    interface.roundtrip().unwrap();
    // mpv was told the plane's size, tiled on every side and activated.
    let (width, height, states) = mpv.received.configures.last().unwrap().clone();
    assert_eq!((width, height), (320, 180));
    let activated = u32::from(xdg_toplevel::State::Activated);
    let tiled = [5, 6, 7, 8];
    assert!(
        states.contains(&activated) && tiled.iter().all(|t| states.contains(t)),
        "{states:?}"
    );
    drop(plane);
}

#[test]
fn frame_callbacks_and_presentation_come_back_to_mpv_and_are_counted() {
    let (host, _interface, plane, mut mpv, _failures, surface) = shown();
    let first = mpv.shm_buffer(64, 48);
    let second = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &first);
    mpv.wait("the first frame's callback and feedback", |r| {
        r.callbacks_done >= 1 && r.presented >= 1
    });
    mpv.draw(&surface, &second);
    mpv.wait("the second frame's", |r| {
        r.callbacks_done >= 2 && r.presented >= 2
    });
    // The first buffer comes back once the second replaced it and the compositor let it go.
    mpv.wait("the first buffer's release", |r| {
        r.released.contains(&first.id())
    });
    let counts = plane.presentation();
    assert!(counts.presented >= 2, "{counts:?}");
    assert_eq!(counts.discarded, 0);
    assert_eq!(counts.zero_copy, 0);
    assert!(counts.gaps[1] >= 1, "consecutive refreshes: {counts:?}");
    drop(host);
}

#[test]
fn without_a_refresh_counter_gaps_are_counted_by_time() {
    let (host, _interface, plane, mut mpv, _failures, surface) = shown();
    host.with(|recorded| recorded.no_msc = true);
    let first = mpv.shm_buffer(64, 48);
    let second = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &first);
    mpv.wait("the first frame's feedback", |r| r.presented >= 1);
    mpv.draw(&surface, &second);
    mpv.wait("the second frame's", |r| r.presented >= 2);
    let counts = plane.presentation();
    assert!(
        counts.gaps[1] >= 1,
        "one refresh apart, by time: {counts:?}"
    );
    drop(host);
}

#[test]
fn zero_copy_and_discarded_frames_are_counted() {
    let (host, _interface, plane, mut mpv, _failures, surface) = shown();
    host.with(|recorded| recorded.zero_copy = true);
    let buffer = mpv.shm_buffer(16, 16);
    mpv.draw(&surface, &buffer);
    mpv.wait("a presented frame", |r| r.presented >= 1);
    assert_eq!(
        plane.presentation().zero_copy,
        plane.presentation().presented
    );
    // Hidden, a frame mpv commits all the same is never shown: its feedback is discarded when the
    // next replaces it.
    plane.hide();
    let presentation: WpPresentation = mpv.bind(2);
    presentation.feedback(&surface, &mpv.qh, ());
    surface.commit();
    presentation.feedback(&surface, &mpv.qh, ());
    surface.commit();
    mpv.wait("a discarded frame", |r| r.discarded >= 1);
}

#[test]
fn place_resizes_the_plane_in_step_with_the_interface_and_tells_mpv() {
    let (host, mut interface, plane, mut mpv, _failures, surface) = shown();
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &buffer);
    host.wait("mpv's frame", |r| {
        interface
            .plane(r)
            .filter(|s| s.buffer_commits >= 1)
            .map(|_| ())
    });
    // The compositor answers when told: the change waits for the interface's commit.
    host.with(|recorded| recorded.answer_at_once = false);
    let before = host.with(|r| interface.plane(r).unwrap().log.len());
    plane.place(0, 0, 640, 360).unwrap();
    interface.commit();
    let log = host.wait("the resize", |recorded| {
        let surface = interface.plane(recorded)?;
        surface.log[before..]
            .contains(&"commit")
            .then(|| surface.log[before..].to_vec())
    });
    assert_eq!(
        log,
        [
            "set_position",
            "set_sync",
            "set_destination",
            "frame",
            "commit"
        ],
        "synchronized, then the change, with a callback to know when it was drawn"
    );
    host.with(|recorded| {
        let surface = interface.plane(recorded).unwrap();
        assert_eq!(surface.destination, Some((640, 360)));
        assert!(surface.subsurface.as_ref().unwrap().sync);
    });
    // Drawn: the plane runs desynchronized again.
    host.frame_done();
    host.wait("the plane desynchronized", |recorded| {
        interface
            .plane(recorded)?
            .log
            .last()
            .filter(|name| **name == "set_desync")
            .map(|_| ())
    });
    // mpv, which committed after acknowledging the last configure, is told the new size.
    mpv.roundtrip().unwrap();
    let size = mpv.received.configures.last().map(|c| (c.0, c.1));
    assert_eq!(size, Some((640, 360)), "{:?}", mpv.received.configures);
    interface.roundtrip().unwrap();
}

#[test]
fn configures_wait_for_mpv_while_the_plane_resizes() {
    let (_host, _interface, plane, mut mpv, _failures, surface) = shown();
    mpv.roundtrip().unwrap();
    let before = mpv.received.configures.len();
    // mpv acknowledged the first configure but has not committed since: sizes wait.
    plane.place(0, 0, 400, 200).unwrap();
    plane.place(0, 0, 500, 250).unwrap();
    plane.place(0, 0, 600, 300).unwrap();
    mpv.roundtrip().unwrap();
    assert_eq!(
        mpv.received.configures.len(),
        before,
        "nothing new while one is in flight"
    );
    surface.commit();
    mpv.wait("the latest size", |r| {
        r.configures
            .last()
            .is_some_and(|c| (c.0, c.1) == (600, 300))
    });
    assert_eq!(
        mpv.received.configures.len(),
        before + 1,
        "only the latest size is sent"
    );
}

#[test]
fn hiding_suspends_mpv_and_unmaps_the_plane_and_showing_brings_back_its_frame() {
    let (host, mut interface, plane, mut mpv, _failures, surface) = shown();
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &buffer);
    host.wait("mpv's frame", |r| {
        interface
            .plane(r)
            .filter(|s| s.buffer_commits >= 1)
            .map(|_| ())
    });
    plane.hide();
    interface.commit();
    let suspended = u32::from(xdg_toplevel::State::Suspended);
    mpv.wait("suspended", |r| {
        r.configures
            .last()
            .is_some_and(|c| c.2.contains(&suspended))
    });
    host.wait("the plane unmapped", |r| {
        interface
            .plane(r)
            .filter(|s| s.buffer.is_none())
            .map(|_| ())
    });
    // The buffer on screen stays mpv's to keep: it is not released while hidden.
    mpv.roundtrip().unwrap();
    assert!(!mpv.received.released.contains(&buffer.id()));
    let shown_before = host.with(|r| interface.plane(r).unwrap().buffer_commits);
    plane.show();
    interface.commit();
    mpv.wait("activated", |r| {
        r.configures
            .last()
            .is_some_and(|c| !c.2.contains(&suspended))
    });
    host.wait("mpv's frame again, without mpv drawing", |r| {
        interface
            .plane(r)
            .filter(|s| s.buffer_commits > shown_before && s.buffer.is_some())
            .map(|_| ())
    });
    interface.roundtrip().unwrap();
}

#[test]
fn closing_the_plane_destroys_what_the_relay_made_and_leaves_the_window() {
    let (host, mut interface, plane, mut mpv, failures, surface) = shown();
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &buffer);
    host.wait("mpv's frame", |r| {
        interface
            .plane(r)
            .filter(|s| s.buffer_commits >= 1)
            .map(|_| ())
    });
    let plane_id = host.with(|r| interface.plane(r).unwrap().id.clone());
    plane.close();
    interface.roundtrip().unwrap();
    host.with(|recorded| {
        let surface = recorded.surface(&plane_id).unwrap();
        assert!(surface.destroyed, "{:?}", surface.log);
        assert!(surface.subsurface.as_ref().unwrap().destroyed);
        assert!(surface.log.contains(&"destroy viewport"));
        assert!(
            recorded.buffers.values().all(|buffer| buffer.destroyed),
            "mpv's buffers' doubles are destroyed"
        );
        assert!(recorded.errors.is_empty(), "{:?}", recorded.errors);
    });
    let deadline = Instant::now() + WAIT;
    while mpv.roundtrip().is_ok() {
        assert!(
            Instant::now() < deadline,
            "mpv's connection closes with the plane"
        );
    }
    // Closing is no failure, and later calls do nothing.
    assert!(failures.lock().unwrap().is_empty());
    plane.show();
    plane.place(0, 0, 10, 10).unwrap();
    plane.close();
    interface.roundtrip().unwrap();
}

#[test]
fn a_place_out_of_range_is_refused_before_it_reaches_the_window() {
    let host = FakeHost::start();
    let interface = host.connect(false);
    let (plane, _mpv, _failures) = relay(&interface);
    assert!(plane.place(0, 0, 0, 10).is_err());
    assert!(plane.place(0, 0, 10, 0).is_err());
    assert!(plane.place(0, 0, u32::MAX, 10).is_err());
    assert!(
        plane.place(-5, -5, 1, 1).is_ok(),
        "a plane may start outside its parent"
    );
}

#[test]
fn a_negative_viewport_source_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let viewporter: WpViewporter = mpv.bind(1);
    let viewport = viewporter.get_viewport(&surface, &mpv.qh, ());
    viewport.set_source(-1.0, 0.0, 10.0, 10.0);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "set_source");
}

#[test]
fn a_viewport_destination_of_zero_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let viewporter: WpViewporter = mpv.bind(1);
    let viewport = viewporter.get_viewport(&surface, &mpv.qh, ());
    viewport.set_destination(0, 10);
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "set_destination",
    );
}

#[test]
fn a_zero_buffer_scale_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    surface.set_buffer_scale(0);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "scale");
}

#[test]
fn a_buffer_not_divisible_by_its_scale_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let buffer = mpv.shm_buffer(63, 48);
    surface.set_buffer_scale(2);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "not divisible");
}

#[test]
fn an_attach_offset_on_version_5_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let buffer = mpv.shm_buffer(16, 16);
    surface.attach(Some(&buffer), 4, 0);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "offset");
}

#[test]
fn an_unknown_buffer_transform_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    // Sent raw: the client library sends only the transforms it knows.
    let message = wayland_client::backend::protocol::Message {
        sender_id: surface.id(),
        // wl_surface.set_buffer_transform
        opcode: 7,
        args: [wayland_client::backend::protocol::Argument::Int(8)]
            .into_iter()
            .collect(),
    };
    mpv.connection
        .backend()
        .send_request(message, None, None)
        .unwrap();
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "unknown transform",
    );
}

#[test]
fn a_shared_memory_pool_larger_than_its_file_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let shm: WlShm = mpv.bind(1);
    let fd = memory(4096);
    shm.create_pool(fd.as_fd(), 8192, &mpv.qh, ());
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "fewer than the pool's",
    );
}

#[test]
fn a_buffer_outside_its_pool_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let shm: WlShm = mpv.bind(1);
    let fd = memory(4096);
    let pool = shm.create_pool(fd.as_fd(), 4096, &mpv.qh, ());
    pool.create_buffer(0, 64, 64, 256, wl_shm::Format::Xrgb8888, &mpv.qh, ());
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "does not fit");
}

#[test]
fn a_shared_memory_pool_that_shrinks_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let shm: WlShm = mpv.bind(1);
    let fd = memory(4096);
    let pool = shm.create_pool(fd.as_fd(), 4096, &mpv.qh, ());
    pool.resize(2048);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "shrinking");
}

#[test]
fn more_pools_alive_than_the_ceiling_of_files_end_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let shm: WlShm = mpv.bind(1);
    let fd = memory(4096);
    let pools: Vec<WlShmPool> = (0..=super::check::MOST_POOL_FDS)
        .map(|_| shm.create_pool(fd.as_fd(), 4096, &mpv.qh, ()))
        .collect();
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "files");
    drop(pools);
}

#[test]
fn a_dmabuf_whose_fd_is_no_dmabuf_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    let params = dmabuf.create_params(&mpv.qh, ());
    let fd = memory(4096);
    params.add(fd.as_fd(), 0, 0, 64, 0, 0);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "not a dma-buf");
}

/// The relay on a shown plane that takes memory files as dma-bufs, as no test machine can make
/// one without a GPU.
fn shown_with_memory_dmabufs() -> (FakeHost, Interface, Plane, FakeMpv, Failures, WlSurface) {
    let (host, interface, plane, mpv, failures, surface) = shown();
    plane.relay().change(|state| state.take_memory_as_dmabuf());
    (host, interface, plane, mpv, failures, surface)
}

#[test]
fn a_dmabuf_mpv_asks_is_made_by_the_compositor_first() {
    let (host, mut interface, _plane, mut mpv, _failures, surface) = shown_with_memory_dmabufs();
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    let fd = memory(64 * 16);
    let params = dmabuf.create_params(&mpv.qh, ());
    params.add(fd.as_fd(), 0, 0, 64, 0, 0);
    params.create(
        16,
        16,
        u32::from_le_bytes(*b"XR24"),
        zwp_linux_buffer_params_v1::Flags::empty(),
    );
    mpv.wait("its buffer", |r| !r.created.is_empty());
    let buffer = mpv.received.created[0].clone();
    mpv.draw(&surface, &buffer);
    host.wait("the dmabuf on the plane", |recorded| {
        let surface = interface.plane(recorded)?;
        let buffer = recorded.buffers.get(surface.buffer.as_ref()?)?;
        buffer.dmabuf.then_some(())
    });
    // One that the compositor cannot import fails for mpv as its own would.
    host.with(|recorded| recorded.dmabufs_fail = true);
    let params = dmabuf.create_params(&mpv.qh, ());
    params.add(fd.as_fd(), 0, 0, 64, 0, 0);
    params.create(
        16,
        16,
        u32::from_le_bytes(*b"XR24"),
        zwp_linux_buffer_params_v1::Flags::empty(),
    );
    mpv.wait("the failure", |r| r.failed == 1);
    interface.roundtrip().unwrap();
}

#[test]
fn a_dmabuf_made_at_once_is_still_made_by_the_compositor_first() {
    let (host, mut interface, _plane, mut mpv, _failures, surface) = shown_with_memory_dmabufs();
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    let fd = memory(64 * 16);
    let params = dmabuf.create_params(&mpv.qh, ());
    params.add(fd.as_fd(), 0, 0, 64, 0, 0);
    // As Mesa makes them: the buffer at once, the parameters destroyed, the frame committed.
    let buffer = params.create_immed(
        16,
        16,
        u32::from_le_bytes(*b"XR24"),
        zwp_linux_buffer_params_v1::Flags::empty(),
        &mpv.qh,
        (),
    );
    params.destroy();
    mpv.draw(&surface, &buffer);
    host.wait("the dmabuf on the plane", |recorded| {
        let surface = interface.plane(recorded)?;
        recorded
            .buffers
            .get(surface.buffer.as_ref()?)?
            .dmabuf
            .then_some(())
    });
    host.with(|recorded| assert_eq!(recorded.immediate_dmabufs, 0, "never create_immed"));
    interface.roundtrip().unwrap();
}

#[test]
fn a_dmabuf_outside_its_fd_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown_with_memory_dmabufs();
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    let fd = memory(64 * 16);
    let params = dmabuf.create_params(&mpv.qh, ());
    params.add(fd.as_fd(), 0, 0, 64, 0, 0);
    params.create(
        16,
        17,
        u32::from_le_bytes(*b"XR24"),
        zwp_linux_buffer_params_v1::Flags::empty(),
    );
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "past the dma-buf's end",
    );
}

/// Makes a dmabuf of one plane, `plane` and `size` as given, and creates it with `flags`.
fn dmabuf(mpv: &FakeMpv, plane: u32, size: (i32, i32), flags: u32) {
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    let fd = memory(64 * 16);
    let params = dmabuf.create_params(&mpv.qh, ());
    params.add(fd.as_fd(), plane, 0, 64, 0, 0);
    params.create(
        size.0,
        size.1,
        u32::from_le_bytes(*b"XR24"),
        zwp_linux_buffer_params_v1::Flags::from_bits_retain(flags),
    );
}

#[test]
fn a_dmabuf_plane_past_the_fourth_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown_with_memory_dmabufs();
    dmabuf(&mpv, 4, (16, 16), 0);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "not below 4");
}

#[test]
fn a_dmabuf_without_its_first_plane_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown_with_memory_dmabufs();
    dmabuf(&mpv, 1, (16, 16), 0);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "no plane 0");
}

#[test]
fn a_dmabuf_with_unknown_flags_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown_with_memory_dmabufs();
    dmabuf(&mpv, 0, (16, 16), 8);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "unknown flags");
}

#[test]
fn a_dmabuf_of_no_pixel_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown_with_memory_dmabufs();
    dmabuf(&mpv, 0, (0, 16), 0);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "invalid size");
}

#[test]
fn binding_a_global_not_offered_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let registry: &WlRegistry = mpv.globals.registry();
    let _seat: WlSeat = registry.bind(999, 1, &mpv.qh, ());
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "Invalid binding",
    );
}

#[test]
fn a_buffer_before_the_first_configure_ends_mpv_only() {
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (_plane, mut mpv, failures) = relay(&interface);
    let compositor: WlCompositor = mpv.bind(6);
    let shell: XdgWmBase = mpv.bind(6);
    let surface = compositor.create_surface(&mpv.qh, ());
    let xdg_surface = shell.get_xdg_surface(&surface, &mpv.qh, ());
    let _toplevel = xdg_surface.get_toplevel(&mpv.qh, ());
    let buffer = mpv.shm_buffer(16, 16);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "before the first configure",
    );
}

#[test]
fn an_acknowledgement_of_a_serial_never_sent_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let serial = mpv.received.serial.unwrap();
    // The toplevel's own xdg_surface, acknowledging a serial the relay never sent.
    mpv.received_xdg_surface().ack_configure(serial + 1000);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "was not sent");
}

#[test]
fn more_objects_than_the_ceiling_end_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let compositor: WlCompositor = mpv.bind(6);
    // In batches below the ceiling of requests at once: only the ceiling of objects is crossed.
    let mut regions: Vec<WlRegion> = Vec::new();
    while regions.len() <= MOST_OBJECTS {
        for _ in 0..MOST_REQUESTS_AT_ONCE / 4 {
            regions.push(compositor.create_region(&mpv.qh, ()));
        }
        if mpv.roundtrip().is_err() {
            break;
        }
    }
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "objects");
}

#[test]
fn more_requests_at_once_than_the_ceiling_end_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    for _ in 0..MOST_REQUESTS_AT_ONCE * 5 {
        surface.damage_buffer(0, 0, 1, 1);
    }
    let _ = mpv.connection.flush();
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "requests at once",
    );
}

#[test]
fn a_second_toplevel_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let compositor: WlCompositor = mpv.bind(6);
    let shell: XdgWmBase = mpv.bind(6);
    let surface = compositor.create_surface(&mpv.qh, ());
    let xdg_surface = shell.get_xdg_surface(&surface, &mpv.qh, ());
    let _toplevel = xdg_surface.get_toplevel(&mpv.qh, ());
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "one toplevel");
}

#[test]
fn mpvs_content_type_reaches_the_plane_at_its_commit() {
    let (host, mut interface, _plane, mut mpv, _failures, surface) = shown();
    let manager: WpContentTypeManagerV1 = mpv.bind(1);
    let content_type = manager.get_surface_content_type(&surface, &mpv.qh, ());
    content_type.set_content_type(wp_content_type_v1::Type::Video);
    mpv.roundtrip().unwrap();
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.content_types.is_empty(), "not before the commit"));
    surface.commit();
    mpv.roundtrip().unwrap();
    host.wait("the content type", |recorded| {
        (recorded.content_types == [u32::from(wp_content_type_v1::Type::Video)]).then_some(())
    });
}

/// mpv's surface with an opaque region made of `rectangles` and then one taken away from it, each
/// `[x, y, width, height]`, committed with a buffer of 64 by 48: what the plane's surface is told
/// is opaque.
fn plane_opaque_region(
    rectangles: &[[i32; 4]],
    subtracted: Option<[i32; 4]>,
) -> Option<Vec<[i32; 4]>> {
    let (host, interface, _plane, mut mpv, _failures, surface) = shown();
    let compositor: WlCompositor = mpv.bind(6);
    let region = compositor.create_region(&mpv.qh, ());
    for &[x, y, width, height] in rectangles {
        region.add(x, y, width, height);
    }
    if let Some([x, y, width, height]) = subtracted {
        region.subtract(x, y, width, height);
    }
    surface.set_opaque_region(Some(&region));
    // Read when set: what mpv does to the region afterwards changes nothing.
    region.subtract(0, 0, 1, 1);
    region.destroy();
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &buffer);
    host.wait("mpv's frame on the plane", |recorded| {
        (interface.plane(recorded)?.buffer_commits >= 1).then_some(())
    });
    host.with(|recorded| interface.plane(recorded).unwrap().opaque_region.clone())
}

#[test]
fn mpvs_opaque_region_over_its_whole_surface_makes_the_plane_opaque() {
    let opaque = Some(vec![[0, 0, i32::MAX, i32::MAX]]);
    assert_eq!(plane_opaque_region(&[[0, 0, 64, 48]], None), opaque);
    assert_eq!(
        plane_opaque_region(&[[-5, -5, 100, 100]], None),
        opaque,
        "a rectangle past the surface covers it too"
    );
    assert_eq!(
        plane_opaque_region(&[[0, 0, 64, 48]], Some([0, 0, 0, 0])),
        opaque,
        "an empty rectangle taken away changes nothing"
    );
}

#[test]
fn an_opaque_region_short_of_mpvs_whole_surface_leaves_the_plane_without_one() {
    assert_eq!(plane_opaque_region(&[[0, 0, 63, 48]], None), None);
    assert_eq!(plane_opaque_region(&[[1, 0, 64, 48]], None), None);
    assert_eq!(
        plane_opaque_region(&[[0, 0, 32, 48], [32, 0, 32, 48]], None),
        None,
        "two rectangles count as covering nothing, though they cover the surface"
    );
    assert_eq!(
        plane_opaque_region(&[[0, 0, 64, 48]], Some([0, 0, 1, 1])),
        None
    );
    assert_eq!(plane_opaque_region(&[], None), None);
}

#[test]
fn the_plane_stops_being_opaque_at_the_commit_after_mpv_unsets_its_region() {
    let (host, mut interface, _plane, mut mpv, _failures, surface) = shown();
    let compositor: WlCompositor = mpv.bind(6);
    let region = compositor.create_region(&mpv.qh, ());
    region.add(0, 0, 64, 48);
    surface.set_opaque_region(Some(&region));
    region.destroy();
    mpv.roundtrip().unwrap();
    interface.roundtrip().unwrap();
    host.with(|recorded| {
        assert_eq!(
            interface.plane(recorded).unwrap().opaque_region,
            None,
            "not before the commit"
        );
    });
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &buffer);
    host.wait("the opaque region", |recorded| {
        interface
            .plane(recorded)?
            .opaque_region
            .as_ref()
            .map(|_| ())
    });
    surface.set_opaque_region(None);
    mpv.roundtrip().unwrap();
    interface.roundtrip().unwrap();
    host.with(|recorded| {
        assert!(interface.plane(recorded).unwrap().opaque_region.is_some());
    });
    let next = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &next);
    host.wait("no opaque region", |recorded| {
        interface
            .plane(recorded)?
            .opaque_region
            .is_none()
            .then_some(())
    });
}

#[test]
fn a_larger_buffer_without_a_new_region_leaves_the_plane_without_one() {
    let (host, interface, _plane, mut mpv, _failures, surface) = shown();
    let compositor: WlCompositor = mpv.bind(6);
    let region = compositor.create_region(&mpv.qh, ());
    region.add(0, 0, 64, 48);
    surface.set_opaque_region(Some(&region));
    region.destroy();
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &buffer);
    host.wait("the opaque region", |recorded| {
        interface
            .plane(recorded)?
            .opaque_region
            .as_ref()
            .map(|_| ())
    });
    // The region is mpv's surface's: a larger surface is no longer covered by it.
    let larger = mpv.shm_buffer(128, 96);
    mpv.draw(&surface, &larger);
    host.wait("no opaque region", |recorded| {
        interface
            .plane(recorded)?
            .opaque_region
            .is_none()
            .then_some(())
    });
}

#[test]
fn mpvs_fifo_barriers_reach_the_planes_fifo_with_mpvs_commit() {
    let (host, interface, _plane, mut mpv, _failures, surface) = shown();
    let manager: WpFifoManagerV1 = mpv.bind(1);
    let fifo = manager.get_fifo(&surface, &mpv.qh, ());
    let buffer = mpv.shm_buffer(64, 48);
    fifo.wait_barrier();
    fifo.set_barrier();
    mpv.draw(&surface, &buffer);
    host.wait("mpv's frame on the plane", |recorded| {
        (interface.plane(recorded)?.buffer_commits >= 1).then_some(())
    });
    // A frame without fifo requests carries none.
    let next = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &next);
    host.wait("mpv's second frame", |recorded| {
        (interface.plane(recorded)?.buffer_commits >= 2).then_some(())
    });
    host.with(|recorded| {
        let log = &interface.plane(recorded).unwrap().log;
        let commits: Vec<usize> = (0..log.len()).filter(|&i| log[i] == "commit").collect();
        let first_frame = log.iter().position(|name| *name == "attach").unwrap();
        let first_commit = commits.iter().copied().find(|&i| i > first_frame).unwrap();
        let barriers: Vec<usize> = (0..log.len())
            .filter(|&i| log[i] == "set_barrier" || log[i] == "wait_barrier")
            .collect();
        assert_eq!(barriers.len(), 2, "{log:?}");
        assert!(
            barriers
                .iter()
                .all(|&i| i > first_frame && i < first_commit),
            "with the frame's commit: {log:?}"
        );
    });
}

#[test]
fn a_second_fifo_for_a_surface_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let manager: WpFifoManagerV1 = mpv.bind(1);
    let _first = manager.get_fifo(&surface, &mpv.qh, ());
    let _second = manager.get_fifo(&surface, &mpv.qh, ());
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "a fifo already");
}

#[test]
fn a_fifo_request_after_its_surface_is_gone_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let compositor: WlCompositor = mpv.bind(6);
    let other = compositor.create_surface(&mpv.qh, ());
    let manager: WpFifoManagerV1 = mpv.bind(1);
    let fifo = manager.get_fifo(&other, &mpv.qh, ());
    other.destroy();
    fifo.set_barrier();
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "was destroyed");
}

#[test]
fn mpvs_surface_is_told_the_planes_scale() {
    let (host, mut interface, _plane, mut mpv, _failures, surface) = shown();
    let manager: WpFractionalScaleManagerV1 = mpv.bind(1);
    manager.get_fractional_scale(&surface, &mpv.qh, ());
    mpv.roundtrip().unwrap();
    assert_eq!(mpv.received.preferred_scale, Some(180));
    interface.roundtrip().unwrap();
    drop(host);
}

#[test]
fn an_output_left_while_held_is_never_told() {
    let host = FakeHost::start();
    host.with(|recorded| recorded.fractional_scale_later = true);
    let mut interface = host.connect(true);
    let (plane, mut mpv, _failures) = relay(&interface);
    plane.place(10, 20, 320, 180).unwrap();
    plane.show();
    interface.commit();
    let _output: WlOutput = mpv.bind(4);
    let compositor: WlCompositor = mpv.bind(6);
    let shell: XdgWmBase = mpv.bind(6);
    let fractional: WpFractionalScaleManagerV1 = mpv.bind(1);
    let surface = compositor.create_surface(&mpv.qh, ());
    let xdg_surface = shell.get_xdg_surface(&surface, &mpv.qh, ());
    let _toplevel = xdg_surface.get_toplevel(&mpv.qh, ());
    let _scale = fractional.get_fractional_scale(&surface, &mpv.qh, ());
    surface.commit();
    mpv.roundtrip().unwrap();
    interface.roundtrip().unwrap();
    // The plane leaves its output while mpv waits for its scale: mpv is told neither.
    host.leave_outputs();
    interface.roundtrip().unwrap();
    host.with(|recorded| {
        for object in &recorded.fractional_scales {
            object.preferred_scale(180);
        }
    });
    mpv.wait("its scale", |received| received.preferred_scale.is_some());
    interface.roundtrip().unwrap();
    mpv.roundtrip().unwrap();
    let events = &mpv.received.surface_events;
    assert!(
        !events.contains(&"enter") && !events.contains(&"leave"),
        "{events:?}"
    );
}

#[test]
fn an_opaque_region_over_mpvs_viewport_destination_makes_the_plane_opaque() {
    // mpv at a fractional scale: a buffer of 64x48 shown at 32x24 through its own viewport, its
    // region in those 32x24.
    for (rectangle, opaque) in [([0, 0, 32, 24], true), ([0, 0, 31, 24], false)] {
        let (host, interface, _plane, mut mpv, _failures, surface) = shown();
        let compositor: WlCompositor = mpv.bind(6);
        let viewporter: WpViewporter = mpv.bind(1);
        let viewport = viewporter.get_viewport(&surface, &mpv.qh, ());
        viewport.set_destination(32, 24);
        let region = compositor.create_region(&mpv.qh, ());
        let [x, y, width, height] = rectangle;
        region.add(x, y, width, height);
        surface.set_opaque_region(Some(&region));
        region.destroy();
        let buffer = mpv.shm_buffer(64, 48);
        mpv.draw(&surface, &buffer);
        host.wait("mpv's frame on the plane", |recorded| {
            (interface.plane(recorded)?.buffer_commits >= 1).then_some(())
        });
        let issued = host.with(|recorded| interface.plane(recorded).unwrap().opaque_region.clone());
        assert_eq!(issued.is_some(), opaque, "{rectangle:?}");
    }
}

#[test]
fn mpvs_surface_is_told_its_output_once_the_compositor_said_the_scale() {
    let host = FakeHost::start();
    host.with(|recorded| recorded.fractional_scale_later = true);
    let mut interface = host.connect(true);
    let (plane, mut mpv, _failures) = relay(&interface);
    plane.place(10, 20, 320, 180).unwrap();
    plane.show();
    interface.commit();
    let _output: WlOutput = mpv.bind(4);
    let compositor: WlCompositor = mpv.bind(6);
    let shell: XdgWmBase = mpv.bind(6);
    let fractional: WpFractionalScaleManagerV1 = mpv.bind(1);
    let surface = compositor.create_surface(&mpv.qh, ());
    let _scale = fractional.get_fractional_scale(&surface, &mpv.qh, ());
    let xdg_surface = shell.get_xdg_surface(&surface, &mpv.qh, ());
    let _toplevel = xdg_surface.get_toplevel(&mpv.qh, ());
    surface.commit();
    mpv.roundtrip().unwrap();
    interface.roundtrip().unwrap();
    mpv.roundtrip().unwrap();
    assert_eq!(mpv.received.entered, 0, "no output before the scale");
    host.with(|recorded| {
        for object in &recorded.fractional_scales {
            object.preferred_scale(180);
        }
    });
    mpv.wait("its surface's output", |received| received.entered > 0);
    let events = &mpv.received.surface_events;
    let scale = events.iter().position(|event| *event == "preferred_scale");
    let enter = events.iter().position(|event| *event == "enter");
    assert!(scale.is_some() && scale < enter, "{events:?}");
    assert_eq!(mpv.received.entered, 1, "told once");
}

#[test]
fn mpvs_surface_is_told_its_scale_before_its_output() {
    // mpv 0.42 rescales its window by the old scale over the new when its surface enters an
    // output: told the output first, it divides by a scale of zero and asks for a viewport of 0x0.
    let host = FakeHost::start();
    let interface = host.connect(true);
    let (plane, mut mpv, _failures) = relay(&interface);
    plane.place(10, 20, 320, 180).unwrap();
    plane.show();
    interface.commit();
    let _output: WlOutput = mpv.bind(4);
    let compositor: WlCompositor = mpv.bind(6);
    let shell: XdgWmBase = mpv.bind(6);
    let fractional: WpFractionalScaleManagerV1 = mpv.bind(1);
    // In mpv's order: the toplevel, then the fractional scale, then the first commit.
    let surface = compositor.create_surface(&mpv.qh, ());
    let xdg_surface = shell.get_xdg_surface(&surface, &mpv.qh, ());
    let _toplevel = xdg_surface.get_toplevel(&mpv.qh, ());
    let _scale = fractional.get_fractional_scale(&surface, &mpv.qh, ());
    surface.commit();
    mpv.wait("its surface's output", |received| received.entered > 0);
    let events = &mpv.received.surface_events;
    let scale = events.iter().position(|event| *event == "preferred_scale");
    let enter = events.iter().position(|event| *event == "enter");
    assert!(scale.is_some() && scale < enter, "{events:?}");
}

/// mpv's frame of 64x48 on the shown plane, as the compositor holds it.
fn drawn(
    host: &FakeHost,
    interface: &Interface,
    mpv: &mut FakeMpv,
    surface: &WlSurface,
) -> WlBuffer {
    let buffer = mpv.shm_buffer(64, 48);
    mpv.draw(surface, &buffer);
    host.wait("mpv's frame on the plane", |recorded| {
        let plane = interface.plane(recorded)?;
        (plane.buffer_commits >= 1).then_some(())
    });
    buffer
}

#[test]
fn a_source_past_a_buffer_mpv_destroyed_ends_mpv_only() {
    // The compositor keeps the size of the buffer it shows after mpv destroys it, and checks
    // each later commit against it.
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let buffer = drawn(&host, &interface, &mut mpv, &surface);
    let viewporter: WpViewporter = mpv.bind(1);
    let viewport = viewporter.get_viewport(&surface, &mpv.qh, ());
    buffer.destroy();
    viewport.set_source(0.0, 0.0, 1000.0, 1000.0);
    surface.commit();
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "outside the buffer",
    );
}

#[test]
fn a_scale_that_does_not_divide_a_buffer_mpv_destroyed_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let buffer = drawn(&host, &interface, &mut mpv, &surface);
    buffer.destroy();
    surface.set_buffer_scale(5);
    surface.commit();
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "not divisible");
}

#[test]
fn a_buffer_destroyed_before_its_commit_holds_no_size() {
    // As a compositor forgets a buffer destroyed before its commit: no size, so any scale.
    let (host, mut interface, _plane, mut mpv, failures, surface) = shown();
    let buffer = mpv.shm_buffer(63, 48);
    surface.attach(Some(&buffer), 0, 0);
    buffer.destroy();
    surface.set_buffer_scale(2);
    surface.commit();
    mpv.roundtrip().unwrap();
    interface.roundtrip().unwrap();
    assert!(failures.lock().unwrap().is_empty());
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

#[test]
fn a_pool_can_gain_no_seal_once_the_relay_took_it() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let shm: WlShm = mpv.bind(1);
    let fd = named_memory("relay-test-sealed-later", 4096);
    shm.create_pool(fd.as_fd(), 4096, &mpv.qh, ());
    mpv.roundtrip().unwrap();
    // mpv still holds the file: sealing it against writing now would make the compositor's
    // mapping of it fail, on the window's connection.
    assert_eq!(
        rustix::fs::fcntl_add_seals(&fd, rustix::fs::SealFlags::FUTURE_WRITE),
        Err(rustix::io::Errno::PERM)
    );
    interface.roundtrip().unwrap();
    assert!(failures.lock().unwrap().is_empty());
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

#[test]
fn a_pool_sealed_against_writing_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let shm: WlShm = mpv.bind(1);
    let fd = named_memory("relay-test-sealed", 4096);
    rustix::fs::fcntl_add_seals(&fd, rustix::fs::SealFlags::FUTURE_WRITE).unwrap();
    shm.create_pool(fd.as_fd(), 4096, &mpv.qh, ());
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "sealed against writing",
    );
}

/// A Wayland message as bytes: an object, an opcode, and arguments of one word each.
fn raw_message(object: u32, opcode: u16, args: &[u32]) -> Vec<u8> {
    let size = u32::try_from(8 + 4 * args.len()).unwrap();
    let mut bytes = Vec::new();
    bytes.extend(object.to_ne_bytes());
    bytes.extend(((size << 16) | u32::from(opcode)).to_ne_bytes());
    for arg in args {
        bytes.extend(arg.to_ne_bytes());
    }
    bytes
}

/// Sends `bytes` with `fds` on `socket`, in one message.
fn send_with_fds(socket: &UnixStream, bytes: &[u8], fds: &[std::os::fd::BorrowedFd<'_>]) {
    use std::mem::MaybeUninit;
    let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(28))];
    let mut control = rustix::net::SendAncillaryBuffer::new(&mut space);
    assert!(control.push(rustix::net::SendAncillaryMessage::ScmRights(fds)));
    let sent = rustix::net::sendmsg(
        socket,
        &[std::io::IoSlice::new(bytes)],
        &mut control,
        rustix::net::SendFlags::empty(),
    )
    .unwrap();
    assert_eq!(sent, bytes.len());
}

/// Reads and drops what the relay sends on `socket`, until it closes; returns whether it did.
fn drain(socket: &UnixStream) -> Arc<Mutex<bool>> {
    let ended = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&ended);
    let mut reader = socket.try_clone().unwrap();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut bytes = [0u8; 65536];
        while let Ok(1..) = reader.read(&mut bytes) {}
        *flag.lock().unwrap() = true;
    });
    ended
}

/// How many of this process's open files are the memory file named `name`.
fn open_files_named(name: &str) -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .flatten()
        .filter(|entry| {
            std::fs::read_link(entry.path())
                .is_ok_and(|target| target.to_string_lossy().contains(name))
        })
        .count()
}

/// A relay on `interface`'s window whose mpv end already holds `sent` when the relay starts, so
/// that one wake-up reads all of it.
fn relay_after(
    interface: &Interface,
    sent: impl FnOnce(&UnixStream),
) -> (Plane, UnixStream, Failures) {
    let (relay_end, mpv_end) = UnixStream::pair().unwrap();
    sent(&mpv_end);
    let failures = Failures::default();
    let recorded = Arc::clone(&failures);
    let (display, parent, below) = interface.handles();
    // SAFETY: the interface outlives the plane in each test that calls this.
    let window = unsafe { Window::new(display, parent, below) };
    let relay = Relay::start(
        window,
        relay_end,
        Box::new(move |reason| recorded.lock().unwrap().push(reason)),
        Box::new(|_| {}),
    )
    .unwrap();
    (Plane::new(relay), mpv_end, failures)
}

/// Waits until the relay closed mpv's end of `ended`'s socket.
fn wait_closed(ended: &Arc<Mutex<bool>>) {
    let deadline = Instant::now() + WAIT;
    while !*ended.lock().unwrap() {
        assert!(Instant::now() < deadline, "mpv's connection stays open");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn files_sent_with_requests_that_take_none_end_mpv_only_and_are_closed() {
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let name = "relay-test-files-with-sync";
    let memfd = named_memory(name, 16);
    // As the review sent them: 20 wl_display.sync, each with 28 files, read in one wake-up.
    let (_plane, mpv, failures) = relay_after(&interface, |mpv| {
        for n in 0..20 {
            let fds = vec![memfd.as_fd(); 28];
            send_with_fds(mpv, &raw_message(1, 0, &[2 + n]), &fds);
        }
    });
    let ended = drain(&mpv);
    wait_closed(&ended);
    let reason = failure(&failures);
    assert!(
        reason.contains(&format!("{MOST_FDS_AT_ONCE} files at once")),
        "{reason}"
    );
    // The 560 copies the relay received are closed with mpv's connection: only the test's own
    // file remains open.
    let deadline = Instant::now() + WAIT;
    while open_files_named(name) > 1 {
        assert!(
            Instant::now() < deadline,
            "{} copies open",
            open_files_named(name)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

#[test]
fn files_left_queued_wake_up_after_wake_up_end_mpv_only() {
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (_plane, mpv, failures) = relay_after(&interface, |_| {});
    let ended = drain(&mpv);
    let memfd = named_memory("relay-test-files-queued", 16);
    // Each wake-up stays under the ceiling of files at once; what no request takes piles up.
    let mut sent = 0;
    let mut id = 2;
    while failures.lock().unwrap().is_empty() {
        assert!(
            sent <= MOST_QUEUED_FDS + 28,
            "{sent} files sent, mpv still served"
        );
        let fds = vec![memfd.as_fd(); 4];
        send_with_fds(&mpv, &raw_message(1, 0, &[id]), &fds);
        sent += 4;
        id += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
    wait_closed(&ended);
    let reason = failure(&failures);
    assert!(reason.contains("files that no request took"), "{reason}");
    assert!(sent > MOST_QUEUED_FDS, "ended after {sent} files only");
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

#[test]
fn a_flood_of_display_requests_ends_mpv_only() {
    // wl_display's requests are answered by the server alone, but counted at the socket.
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (_plane, mpv, failures) = relay_after(&interface, |mpv| {
        use std::io::Write;
        let mut flood = Vec::new();
        for n in 0..(MOST_REQUESTS_AT_ONCE as u32 + 100) {
            flood.extend(raw_message(1, 0, &[2 + n]));
        }
        (&*mpv).write_all(&flood).unwrap();
    });
    let ended = drain(&mpv);
    wait_closed(&ended);
    let reason = failure(&failures);
    assert!(reason.contains("requests at once"), "{reason}");
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

#[test]
fn a_flood_of_registries_ends_mpv_only() {
    // The review's case: 200,000 registries written as fast as the socket takes them.
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (_plane, mpv, failures) = relay_after(&interface, |_| {});
    let ended = drain(&mpv);
    let writer = {
        let mut mpv = mpv.try_clone().unwrap();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut flood = Vec::new();
            for n in 0..200_000u32 {
                flood.extend(raw_message(1, 1, &[2 + n]));
            }
            // Fails once the relay closes the socket.
            let _ = mpv.write_all(&flood);
        })
    };
    wait_closed(&ended);
    let reason = failure(&failures);
    // Whichever ceiling comes first: the requests of one wake-up, the registries, or the
    // server's own, on answers mpv leaves unread.
    assert!(
        ["requests at once", "registries", "did not read"]
            .iter()
            .any(|ceiling| reason.contains(ceiling)),
        "{reason}"
    );
    writer.join().unwrap();
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

#[test]
fn a_bind_without_an_interface_ends_mpv_only() {
    // wayland-backend 0.3.17 panics at it: the relay refuses it at the socket. Found by the fuzz
    // target.
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (plane, mpv, failures) = relay_after(&interface, |mpv| {
        use std::io::Write;
        let mut bytes = raw_message(1, 1, &[2]); // wl_display.get_registry
        // wl_registry.bind(name 1, a null string, version 1, new id 3)
        bytes.extend(raw_message(2, 0, &[1, 0, 1, 3]));
        (&*mpv).write_all(&bytes).unwrap();
    });
    let ended = drain(&mpv);
    wait_closed(&ended);
    let reason = failure(&failures);
    assert!(reason.contains("without naming its interface"), "{reason}");
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
    // The relay's thread lives on: it closes the plane when asked.
    plane.close();
    interface.roundtrip().unwrap();
}

#[test]
fn a_title_left_out_ends_mpv_only() {
    // wayland-server's generated parser panics at it: the relay reads the toplevel's requests
    // itself. Found by the fuzz target.
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let toplevel = mpv.received.toplevel.clone().expect("a toplevel");
    // Written on the socket itself, the client library having nothing waiting: it sends no null
    // string where the protocol forbids it. xdg_toplevel.set_title, with a string of length 0.
    mpv.connection.flush().unwrap();
    let bytes = raw_message(toplevel.id().protocol_id(), 2, &[0]);
    let sent = rustix::net::send(
        mpv.connection.backend().poll_fd(),
        &bytes,
        rustix::net::SendFlags::NOSIGNAL,
    );
    assert_eq!(sent, Ok(bytes.len()));
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "left out");
}

#[test]
fn a_negative_size_limit_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let toplevel = mpv.received.toplevel.clone().expect("a toplevel");
    toplevel.set_max_size(-1, 10);
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "negative");
}

#[test]
fn more_registries_than_the_ceiling_end_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let display = mpv.connection.display();
    let mut made = 0;
    let mut registries = Vec::new();
    // In batches far below the ceiling of requests at once, whose answers (each global, to each
    // registry) fit the relay's socket pair: only the ceiling of registries is crossed.
    while made <= MOST_REGISTRIES {
        for _ in 0..256 {
            registries.push(display.get_registry(&mpv.qh, ()));
        }
        made += 256;
        if mpv.roundtrip().is_err() {
            break;
        }
    }
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "registries");
    // The registry of mpv's list of globals counts too.
    assert!(
        made + 1 > MOST_REGISTRIES,
        "ended after {made} registries only"
    );
}

#[test]
fn frames_mpv_commits_are_counted_and_while_hidden_not_shown() {
    let (host, interface, plane, mut mpv, _failures, surface) = shown();
    let first = mpv.shm_buffer(64, 48);
    mpv.draw(&surface, &first);
    mpv.wait("the first frame shown", |r| r.presented >= 1);
    assert_eq!(plane.presentation().committed, 1);
    let shown_frames = host.with(|r| interface.plane(r).unwrap().buffer_commits);
    plane.hide();
    let second = mpv.shm_buffer(64, 48);
    let third = mpv.shm_buffer(64, 48);
    surface.attach(Some(&second), 0, 0);
    surface.commit();
    surface.attach(Some(&third), 0, 0);
    surface.commit();
    // A commit that attaches nothing is no frame.
    surface.commit();
    mpv.roundtrip().unwrap();
    assert_eq!(plane.presentation().committed, 3, "counted, hidden or not");
    host.with(|r| {
        assert_eq!(
            interface.plane(r).map_or(0, |plane| plane.buffer_commits),
            shown_frames,
            "none shown while hidden"
        );
    });
}

/// Asserts that the relay's thread wakes no more, once it settled.
fn assert_asleep(plane: &Plane) {
    std::thread::sleep(Duration::from_millis(300));
    let before = plane.relay().wake_ups();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        plane.relay().wake_ups(),
        before,
        "the relay's thread keeps waking"
    );
}

#[test]
fn the_relays_thread_sleeps_once_mpv_hung_up() {
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (plane, mpv, failures) = relay(&interface);
    drop(mpv);
    assert_asleep(&plane);
    assert!(
        failures.lock().unwrap().is_empty(),
        "hanging up is no fault"
    );
    interface.roundtrip().unwrap();
}

#[test]
fn the_relays_thread_sleeps_once_it_ended_mpv_that_keeps_its_socket() {
    let (_host, _interface, plane, mut mpv, failures, surface) = shown();
    surface.set_buffer_scale(0);
    let _ = mpv.roundtrip();
    let _ = failure(&failures);
    assert_asleep(&plane);
    drop(mpv);
}

#[test]
fn the_relays_thread_wakes_for_the_interfaces_own_events() {
    // The relay's queue shares the window's socket (ADR-0015): what the compositor sends the
    // interface wakes the relay's thread too, with no mpv and the plane hidden.
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (plane, mpv, _failures) = relay(&interface);
    drop(mpv);
    assert_asleep(&plane);
    let before = plane.relay().wake_ups();
    for _ in 0..100 {
        interface.roundtrip().unwrap();
    }
    std::thread::sleep(Duration::from_millis(100));
    let woken = plane.relay().wake_ups() - before;
    eprintln!("the relay's thread woke {woken} times for 100 round trips of the interface's");
    assert!(woken > 0, "docs/ARCHITECTURE.md says it wakes");
}

#[test]
fn a_close_made_while_another_closes_returns_once_it_is_done() {
    let (host, mut interface, plane, mut mpv, _failures, surface) = shown();
    let _buffer = drawn(&host, &interface, &mut mpv, &surface);
    let plane = Arc::new(plane);
    let start = Arc::new(std::sync::Barrier::new(8));
    let closers: Vec<_> = (0..8)
        .map(|_| {
            let plane = Arc::clone(&plane);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                plane.close();
                // The interface may destroy its window as soon as close returns.
                plane.relay().closed()
            })
        })
        .collect();
    for closer in closers {
        assert!(
            closer.join().unwrap(),
            "close returned before the plane was closed"
        );
    }
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

#[test]
fn a_dmabuf_made_at_once_past_the_ceiling_leaves_nothing_on_the_window() {
    let (host, mut interface, plane, mut mpv, failures, _surface) = shown_with_memory_dmabufs();
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    let fd = memory(64 * 16);
    let params = dmabuf.create_params(&mpv.qh, ());
    params.add(fd.as_fd(), 0, 0, 64, 0, 0);
    mpv.roundtrip().unwrap();
    // mpv is one object below the ceiling: the buffer made at once is the one too many.
    plane.relay().change(|state| state.objects = MOST_OBJECTS);
    let _buffer = params.create_immed(
        16,
        16,
        u32::from_le_bytes(*b"XR24"),
        zwp_linux_buffer_params_v1::Flags::empty(),
        &mpv.qh,
        (),
    );
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "objects");
    // mpv's objects are let go with its connection: its parameters' double on the window's
    // connection is destroyed, and no buffer was made there.
    let deadline = Instant::now() + WAIT;
    while host.with(|recorded| !recorded.params.is_empty()) {
        assert!(
            Instant::now() < deadline,
            "the parameters stay on the window"
        );
        interface.roundtrip().unwrap();
    }
    host.with(|recorded| {
        assert!(recorded.buffers.values().all(|buffer| !buffer.dmabuf));
    });
}

#[test]
fn a_dmabuf_made_at_once_that_the_compositor_cannot_import_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown_with_memory_dmabufs();
    host.with(|recorded| recorded.dmabufs_fail = true);
    let dmabuf: ZwpLinuxDmabufV1 = mpv.bind(4);
    let fd = memory(64 * 16);
    let params = dmabuf.create_params(&mpv.qh, ());
    params.add(fd.as_fd(), 0, 0, 64, 0, 0);
    let _buffer = params.create_immed(
        16,
        16,
        u32::from_le_bytes(*b"XR24"),
        zwp_linux_buffer_params_v1::Flags::empty(),
        &mpv.qh,
        (),
    );
    params.destroy();
    let _ = mpv.roundtrip();
    ended_mpv_only(
        &host,
        &mut interface,
        &mut mpv,
        &failures,
        "could not import",
    );
}

#[test]
fn a_start_on_a_compositor_without_a_viewporter_fails_and_leaves_nothing_bound() {
    let host = FakeHost::start_without(&["wp_viewporter"]);
    let mut interface = host.connect(false);
    let (relay_end, _mpv_end) = UnixStream::pair().unwrap();
    let (display, parent, below) = interface.handles();
    // SAFETY: the interface outlives the relay, which fails to start.
    let window = unsafe { Window::new(display, parent, below) };
    let started = Relay::start(window, relay_end, Box::new(|_| {}), Box::new(|_| {}));
    let error = started.err().expect("no viewporter, no plane");
    assert!(error.to_string().contains("wp_viewporter"), "{error}");
    interface.roundtrip().unwrap();
    host.with(|recorded| {
        assert_eq!(recorded.bound, [("wl_subcompositor", true)], "destroyed");
        assert!(recorded.errors.is_empty(), "{:?}", recorded.errors);
    });
}

#[test]
fn mpv_destroying_its_toplevel_unmaps_the_plane() {
    let (host, mut interface, plane, mut mpv, failures, surface) = shown();
    let _buffer = drawn(&host, &interface, &mut mpv, &surface);
    let toplevel = mpv.received.toplevel.clone().expect("a toplevel");
    toplevel.destroy();
    mpv.roundtrip().unwrap();
    host.wait("the plane unmapped", |recorded| {
        interface
            .plane(recorded)
            .filter(|plane| plane.buffer.is_none())
            .map(|_| ())
    });
    // mpv's surface is no longer the plane's: what mpv commits on it stays with the relay. Its
    // shell surface, without a toplevel, may hold no buffer.
    let commits = host.with(|r| interface.plane(r).unwrap().commits);
    surface.attach(None, 0, 0);
    surface.set_buffer_scale(2);
    surface.commit();
    mpv.roundtrip().unwrap();
    interface.roundtrip().unwrap();
    host.with(|r| assert_eq!(interface.plane(r).unwrap().commits, commits));
    assert!(failures.lock().unwrap().is_empty());
    assert_eq!(plane.presentation().committed, 1, "the plane's frames only");
}

#[test]
fn an_output_the_compositor_removes_is_removed_for_mpv() {
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (_plane, mut mpv, failures) = relay(&interface);
    let _output: WlOutput = mpv.bind(4);
    mpv.roundtrip().unwrap();
    let mut host = host;
    host.remove_output();
    interface.roundtrip().unwrap();
    mpv.wait("the output's global removed", |r| r.globals_removed == 1);
    assert!(!mpv.global_names().contains(&"wl_output".to_owned()));
    // The relay released its own object for it.
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
    assert!(failures.lock().unwrap().is_empty());
}

#[test]
fn an_outputs_name_is_sent_once_and_its_description_when_it_changes() {
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (_plane, mut mpv, _failures) = relay(&interface);
    let _output: WlOutput = mpv.bind(4);
    mpv.roundtrip().unwrap();
    assert_eq!(mpv.received.output_names, 1);
    assert_eq!(mpv.received.output_descriptions, 1);
    host.announce_output(None);
    interface.roundtrip().unwrap();
    mpv.wait("the output's state again", |r| r.output_dones >= 2);
    assert_eq!(mpv.received.output_names, 1, "a name is sent once");
    assert_eq!(mpv.received.output_descriptions, 1, "unchanged");
    host.announce_output(Some("Another description"));
    interface.roundtrip().unwrap();
    mpv.wait("the output's state again", |r| r.output_dones >= 3);
    assert_eq!(mpv.received.output_names, 1);
    assert_eq!(mpv.received.output_descriptions, 2, "changed");
}

#[test]
fn a_popup_ends_mpv_only() {
    let (host, mut interface, _plane, mut mpv, failures, _surface) = shown();
    let xdg_surface = mpv.received_xdg_surface();
    let shell: XdgWmBase = mpv.bind(6);
    let positioner = shell.create_positioner(&mpv.qh, ());
    let _popup = xdg_surface.get_popup(None, &positioner, &mpv.qh, ());
    let _ = mpv.roundtrip();
    ended_mpv_only(&host, &mut interface, &mut mpv, &failures, "no popups");
}

impl FakeMpv {
    /// The `xdg_surface` of the toplevel made by [`FakeMpv::toplevel`].
    fn received_xdg_surface(&self) -> XdgSurface {
        self.received
            .xdg_surface
            .clone()
            .expect("a toplevel was made")
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Received {
    fn event(
        received: &mut Self,
        _: &WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_registry::Event::GlobalRemove { .. } = event {
            received.globals_removed += 1;
        }
    }
}

delegate_noop!(Received: ignore WlRegistry);
delegate_noop!(Received: ignore wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup);
delegate_noop!(Received: wayland_protocols::xdg::shell::client::xdg_positioner::XdgPositioner);

delegate_noop!(Received: WlCompositor);
delegate_noop!(Received: WlRegion);
delegate_noop!(Received: WlShmPool);
delegate_noop!(Received: WpViewporter);
delegate_noop!(Received: WpViewport);
delegate_noop!(Received: ignore WpPresentation);
delegate_noop!(Received: ZwpLinuxDmabufV1);
delegate_noop!(Received: WpFractionalScaleManagerV1);
delegate_noop!(Received: WpContentTypeManagerV1);
delegate_noop!(Received: WpFifoManagerV1);
delegate_noop!(Received: WpFifoV1);
delegate_noop!(Received: WpContentTypeV1);
delegate_noop!(Received: ignore WlSeat);

impl Dispatch<WlSurface, ()> for Received {
    fn event(
        received: &mut Self,
        _: &WlSurface,
        event: wayland_client::protocol::wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_surface::Event;
        match event {
            Event::Enter { .. } => {
                received.entered += 1;
                received.surface_events.push("enter");
            }
            Event::Leave { .. } => received.surface_events.push("leave"),
            Event::PreferredBufferScale { .. } => {
                received.surface_events.push("preferred_buffer_scale");
            }
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for Received {
    fn event(
        received: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            received.callbacks_done += 1;
        }
    }
}

impl Dispatch<WlShm, ()> for Received {
    fn event(
        received: &mut Self,
        _: &WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_shm::Event::Format {
            format: WEnum::Value(format),
        } = event
        {
            received.shm_formats.push(format.into());
        }
    }
}

impl Dispatch<WlBuffer, ()> for Received {
    fn event(
        received: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            received.released.push(buffer.id());
        }
    }
}

impl Dispatch<WlOutput, ()> for Received {
    fn event(
        received: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Mode { refresh, .. } => received.output_refresh = Some(refresh),
            wl_output::Event::Name { .. } => received.output_names += 1,
            wl_output::Event::Description { .. } => received.output_descriptions += 1,
            wl_output::Event::Done => received.output_dones += 1,
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Received {
    fn event(
        _: &mut Self,
        shell: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for Received {
    fn event(
        received: &mut Self,
        surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            received.serial = Some(serial);
            received.xdg_surface = Some(surface.clone());
            // As mpv does: acknowledged as it comes.
            surface.ack_configure(serial);
        }
    }
}

impl Dispatch<XdgToplevel, ()> for Received {
    fn event(
        received: &mut Self,
        toplevel: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure {
            width,
            height,
            states,
        } = event
        {
            let states = states
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| u32::from_ne_bytes(*bytes))
                .collect();
            received.configures.push((width, height, states));
            received.toplevel = Some(toplevel.clone());
        }
    }
}

impl Dispatch<WpPresentationFeedback, ()> for Received {
    fn event(
        received: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented { .. } => received.presented += 1,
            wp_presentation_feedback::Event::Discarded => received.discarded += 1,
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for Received {
    fn event(
        received: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => received.created.push(buffer),
            zwp_linux_buffer_params_v1::Event::Failed => received.failed += 1,
            _ => {}
        }
    }

    wayland_client::event_created_child!(Received, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for Received {
    fn event(
        received: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::FormatTable { size, .. } => {
                received.format_table = Some(size);
            }
            zwp_linux_dmabuf_feedback_v1::Event::Done => received.feedback_batches += 1,
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for Received {
    fn event(
        received: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            received.preferred_scale = Some(scale);
            received.surface_events.push("preferred_scale");
        }
    }
}

/// A fake compositor, an interface, a shown plane at 320x180, a fake mpv with its toplevel
/// configured, and what the relay tells of the display. Each commit is presented at once, at
/// the fake display's pace.
fn shown_telling() -> (FakeHost, Interface, Plane, FakeMpv, News, WlSurface) {
    let host = FakeHost::start();
    let interface = host.connect(true);
    let (plane, mut mpv, _failures, news) = relay_telling(&interface);
    plane.place(10, 20, 320, 180).unwrap();
    plane.show();
    interface.commit();
    let (surface, _, _) = mpv.toplevel();
    (host, interface, plane, mpv, news, surface)
}

/// Has mpv draw `count` frames on `surface`, each presented before the next.
fn play(mpv: &mut FakeMpv, surface: &WlSurface, count: u32) {
    let buffers = [mpv.shm_buffer(64, 48), mpv.shm_buffer(64, 48)];
    for index in 0..count {
        let presented = mpv.received.presented;
        mpv.draw(surface, &buffers[index as usize % 2]);
        mpv.wait("the frame's feedback", |r| r.presented > presented);
    }
}

/// The frames a probe needs: its settle, the frame that starts its chain, then its intervals.
const PROBED: u32 = SETTLE + 1 + INTERVALS;

#[test]
fn frames_at_whole_refreshes_tell_a_fixed_rate() {
    let (host, _interface, plane, mut mpv, news, surface) = shown_telling();
    host.with(|recorded| recorded.refreshes_a_frame = 3);
    play(&mut mpv, &surface, 5);
    plane.relay().probe_asker()(1);
    play(&mut mpv, &surface, PROBED - 1);
    assert!(news.lock().unwrap().is_empty(), "one frame short");
    play(&mut mpv, &surface, 1);
    let rate = DisplayRate::Fixed;
    assert_eq!(
        *news.lock().unwrap(),
        [DisplayNews::Rate { probe: 1, rate }]
    );
    let counts = plane.presentation();
    assert_eq!((counts.probes_ended, counts.display_rate), (1, Some(rate)));
    assert!(counts.last_probe.is_some());
    // A probe tells once.
    play(&mut mpv, &surface, PROBED);
    assert_eq!(news.lock().unwrap().len(), 1);
}

#[test]
fn frames_between_refreshes_tell_vrr() {
    let (host, _interface, plane, mut mpv, news, surface) = shown_telling();
    host.with(|recorded| recorded.vrr = true);
    plane.relay().probe_asker()(4);
    play(&mut mpv, &surface, PROBED);
    let rate = DisplayRate::Variable;
    assert_eq!(
        *news.lock().unwrap(),
        [DisplayNews::Rate { probe: 4, rate }]
    );
    assert_eq!(plane.presentation().display_rate, Some(rate));
}

#[test]
fn nothing_is_told_without_a_probe_asked() {
    let (_host, _interface, plane, mut mpv, news, surface) = shown_telling();
    play(&mut mpv, &surface, 2 * PROBED);
    plane.place(0, 0, 640, 360).unwrap();
    play(&mut mpv, &surface, 2);
    assert!(news.lock().unwrap().is_empty());
    assert_eq!(plane.presentation().probes_ended, 0);
}

#[test]
fn vrr_that_turns_on_with_fullscreen_is_told_after_the_change() {
    let (host, interface, plane, mut mpv, news, surface) = shown_telling();
    host.with(|recorded| recorded.vrr_while_fullscreen = true);
    plane.relay().probe_asker()(1);
    play(&mut mpv, &surface, PROBED);
    let fixed = DisplayNews::Rate {
        probe: 1,
        rate: DisplayRate::Fixed,
    };
    assert_eq!(*news.lock().unwrap(), [fixed]);
    // The window goes fullscreen: the plane fills the output, and the compositor turns VRR on.
    plane
        .place(0, 0, OUTPUT_SIZE.0 as u32, OUTPUT_SIZE.1 as u32)
        .unwrap();
    interface.commit();
    assert_eq!(*news.lock().unwrap(), [fixed, DisplayNews::Changed]);
    // A second change before the player asks again is not told again.
    plane.place(0, 0, 800, 600).unwrap();
    plane
        .place(0, 0, OUTPUT_SIZE.0 as u32, OUTPUT_SIZE.1 as u32)
        .unwrap();
    interface.commit();
    assert_eq!(news.lock().unwrap().len(), 2);
    // The player switched mpv to audio sync, and asks again.
    plane.relay().probe_asker()(2);
    play(&mut mpv, &surface, PROBED);
    host.with(|recorded| assert!(recorded.vrr, "the fake compositor's VRR is on"));
    let variable = DisplayNews::Rate {
        probe: 2,
        rate: DisplayRate::Variable,
    };
    assert_eq!(
        *news.lock().unwrap(),
        [fixed, DisplayNews::Changed, variable]
    );
    assert_eq!(plane.presentation().probes_ended, 2);
}

#[test]
fn a_change_during_a_probe_starts_it_again_untold() {
    let (host, interface, plane, mut mpv, news, surface) = shown_telling();
    plane.relay().probe_asker()(1);
    play(&mut mpv, &surface, PROBED - 1);
    // A new size, and VRR on: the frames before say nothing of the display after.
    host.with(|recorded| recorded.vrr = true);
    plane.place(0, 0, 640, 360).unwrap();
    interface.commit();
    play(&mut mpv, &surface, PROBED - 1);
    assert!(
        news.lock().unwrap().is_empty(),
        "{:?}",
        news.lock().unwrap()
    );
    play(&mut mpv, &surface, 1);
    let rate = DisplayRate::Variable;
    assert_eq!(
        *news.lock().unwrap(),
        [DisplayNews::Rate { probe: 1, rate }]
    );
}

#[test]
fn leaving_an_output_after_a_probe_is_a_change() {
    let (host, _interface, plane, mut mpv, news, surface) = shown_telling();
    plane.relay().probe_asker()(1);
    play(&mut mpv, &surface, PROBED);
    host.leave_outputs();
    let deadline = Instant::now() + WAIT;
    while news.lock().unwrap().len() < 2 {
        assert!(Instant::now() < deadline, "{:?}", news.lock().unwrap());
        mpv.roundtrip().unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(news.lock().unwrap()[1], DisplayNews::Changed);
}

#[test]
fn presentations_without_the_displays_own_time_tell_nothing() {
    let (host, _interface, plane, mut mpv, news, surface) = shown_telling();
    host.with(|recorded| recorded.untimed = true);
    plane.relay().probe_asker()(1);
    play(&mut mpv, &surface, 2 * PROBED);
    assert!(
        news.lock().unwrap().is_empty(),
        "{:?}",
        news.lock().unwrap()
    );
    assert_eq!(plane.presentation().probes_ended, 0);
    assert!(plane.presentation().presented >= u64::from(2 * PROBED));
}

#[test]
fn a_probe_asked_after_the_plane_closed_does_nothing() {
    let (_host, _interface, plane, _mpv, news, _surface) = shown_telling();
    let probes = plane.relay().probe_asker();
    plane.close();
    probes(1);
    assert!(news.lock().unwrap().is_empty());
    assert_eq!(plane.presentation().probes_ended, 0);
}
