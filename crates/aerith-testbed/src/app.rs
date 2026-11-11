// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The window and its loop: winit's events, the player's, the schedule's cues, and the drawing.
//!
//! **Drawing.** After each turn of winit's loop, the testbed works out what the controls show
//! ([`Frame`]) and draws only when that changed, or when the window's surface owes a commit: a new
//! place of the plane, a configure of winit's. Each frame drawn asks the compositor for a frame
//! callback on the drawn surface, and the next is drawn only once it came: none while the window
//! is not shown. The callback reaches the testbed's own queue on winit's connection
//! ([`crate::surfaces`]). While one is awaited, the loop runs with a deadline, so that winit hands
//! it each wake-up, and dispatches that queue each time: [`FRAME_LATE`] after the frame, then each
//! [`FRAME_LOOK_AGAIN`].
//!
//! **The plane's changes** take effect at the next commit of the window's surface: in the "below"
//! order that is the next frame wgpu presents on it; in the "above" order, the testbed's own
//! commit, which also shows what wgpu presented on its subsurface.
//!
//! **The end.** The schedule prints its summary, the plane closes, then the player goes, then
//! wgpu's surface, the testbed's surfaces and the window, in that order ([`Session::close`]).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aerith_player::{
    Audio, Clock, Ending, Event, OpenId, Player, Settings, Source, Video, Window as PlaneWindow,
};
use raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle,
    WaylandWindowHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::args::{Options, Order};
use crate::draw::{Gpu, Shape, controls_shapes};
use crate::layout::{Controls, Hit, PlaneRect, plane_rect};
use crate::schedule::{Cue, Note, Schedule, Shared, StatsSource, Timeline};
use crate::surfaces::Surfaces;
use crate::visibility::Visibility;

/// How long after a frame drawn the loop looks for its frame callback again, if none came: longer
/// than a refresh of the slowest display. A callback that the relay's thread read in the instant
/// before the loop went to sleep waits in the testbed's queue until then.
const FRAME_LATE: Duration = Duration::from_millis(100);
/// How often the loop looks again after that, while the compositor sends no callback: it sends
/// none while the window is not shown.
const FRAME_LOOK_AGAIN: Duration = Duration::from_secs(1);
/// How long to wait before drawing again when wgpu had no frame to give.
const FRAME_RETRY: Duration = Duration::from_millis(100);
/// What the arrow keys seek by.
const SEEK_STEP: Duration = Duration::from_secs(10);
/// The window's size, unless fullscreen.
const WINDOW_SIZE: LogicalSize<u32> = LogicalSize::new(1280, 720);

/// What reaches the loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    Player(Event),
    /// The player's channel closed: its engine thread ended.
    PlayerGone,
    Schedule(Cue),
}

impl From<Cue> for UserEvent {
    fn from(cue: Cue) -> Self {
        Self::Schedule(cue)
    }
}

/// Opens the window and plays until the run ends; the error says why it failed.
pub fn run(options: Options, cache: &Path) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|error| format!("no Wayland session: {error}"))?;
    let mut app = App {
        proxy: event_loop.create_proxy(),
        options,
        cache: cache.to_owned(),
        session: None,
        outcome: Ok(()),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("the window's loop: {error}"))?;
    app.outcome
}

struct App {
    options: Options,
    cache: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
    session: Option<Session>,
    /// The first failure, if any.
    outcome: Result<(), String>,
}

/// Whether the loop goes on.
enum Flow {
    Continue,
    Quit(Result<(), String>),
}

impl App {
    fn quit(&mut self, event_loop: &ActiveEventLoop, outcome: Result<(), String>) {
        if self.outcome.is_ok() {
            self.outcome = outcome;
        }
        if let Some(session) = self.session.take() {
            session.close();
        }
        event_loop.exit();
    }

    fn follow(&mut self, event_loop: &ActiveEventLoop, flow: Flow) {
        if let Flow::Quit(outcome) = flow {
            self.quit(event_loop, outcome);
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.session.is_some() || self.outcome.is_err() {
            return;
        }
        match Session::start(event_loop, &self.options, &self.cache, self.proxy.clone()) {
            Ok(session) => self.session = Some(session),
            Err(error) => self.quit(event_loop, Err(error)),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let Some(session) = &mut self.session {
            let flow = session.window_event(event);
            self.follow(event_loop, flow);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if let Some(session) = &mut self.session {
            let flow = session.user_event(event);
            self.follow(event_loop, flow);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(session) = &mut self.session else {
            return;
        };
        match session.update(Instant::now()) {
            Ok(Some(deadline)) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            Ok(None) => event_loop.set_control_flow(ControlFlow::Wait),
            Err(error) => self.quit(event_loop, Err(error)),
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(session) = self.session.take() {
            session.close();
        }
    }
}

/// What the controls show in a frame: one is drawn only when this changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    /// The drawn surface's size, in pixels.
    size: (u32, u32),
    shown: bool,
    paused: bool,
    /// The played part of the seek bar, in whole pixels.
    played: u32,
}

/// The window, the player, and what the loop keeps between turns.
struct Session {
    // Dropped in this order, as `close` does: the schedule prints its summary while the relay
    // runs; the player closes its plane; wgpu's surface goes before the surface it draws on; the
    // testbed's surfaces before the window they are made on.
    schedule: Schedule,
    player: Arc<Player>,
    forwarder: Option<JoinHandle<()>>,
    gpu: Gpu,
    surfaces: Surfaces,
    window: Window,
    shared: Arc<Shared>,
    source: Source,
    order: Order,
    letterbox: bool,
    /// What the seek bar spans: the player reports no length yet.
    span: Duration,
    open_id: Option<OpenId>,
    opened: Option<Instant>,
    paused: bool,
    clock: Option<Clock>,
    video_size: Option<(u32, u32)>,
    /// The last report of the picture printed.
    reported: Option<String>,
    visibility: Visibility,
    pointer: Option<PhysicalPosition<f64>>,
    /// Where the plane was last placed.
    placed: Option<PlaneRect>,
    /// The window's surface needs a commit: the plane changed, or winit's configure asks one.
    root_commit_owed: bool,
    /// What the last frame drawn shows.
    drawn: Option<Frame>,
    /// When the last frame drawn asked for a frame callback that has not come yet. No frame is
    /// drawn until it comes.
    frame_asked: Option<Instant>,
    /// Whether that callback was counted late.
    frame_late: bool,
    /// When to try again a frame that wgpu could not give.
    retry_at: Option<Instant>,
    /// In the "above" order, whether the drawn surface holds a buffer.
    drawn_has_buffer: bool,
}

impl Session {
    fn start(
        event_loop: &ActiveEventLoop,
        options: &Options,
        cache: &Path,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        let source =
            if options.source.starts_with("http://") || options.source.starts_with("https://") {
                Source::stream(options.source.clone(), Vec::new())
            } else {
                Source::file(&options.source)
            }
            .map_err(|error| format!("{}: {error}", options.source))?;
        let mut attributes = Window::default_attributes()
            .with_title("Aerith testbed")
            .with_inner_size(WINDOW_SIZE)
            // In the "below" order, the video shows only through a window that is not opaque.
            .with_transparent(options.order == Order::Below);
        if options.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = event_loop
            .create_window(attributes)
            .map_err(|error| format!("the window: {error}"))?;
        let (display, root) = wayland_handles(&window)?;
        // SAFETY: `display` and `root` are winit's, alive while `window` is; the surfaces are
        // dropped before the window, by `Session`'s field order and by `close`, and on a failure
        // below, as locals, in the reverse order of their making.
        let surfaces = unsafe { Surfaces::join(display, root, options.order, logical(&window)) }?;
        let size = window.inner_size();
        // SAFETY: the drawn surface, winit's or the testbed's, and the display outlive the GPU's
        // surface: `gpu` is dropped before `surfaces` and `window`, as above.
        let gpu = unsafe { Gpu::new(display, surfaces.drawn(), (size.width, size.height)) }?;
        eprintln!(
            "aerith-testbed: order {:?}, drawing with {}",
            options.order, gpu.description
        );
        let below = match options.order {
            Order::Below => root,
            Order::Above => surfaces.drawn(),
        };
        // SAFETY: the display and both surfaces outlive the plane: the session closes it before
        // anything else goes, and dropping the player, first of what was made after the
        // surfaces, closes it on a failure too.
        let plane_window = unsafe {
            PlaneWindow::new(
                WaylandDisplayHandle::new(display),
                WaylandWindowHandle::new(root),
                WaylandWindowHandle::new(below),
            )
        };
        let settings = Settings {
            engine: options.mpv.clone(),
            video: Video::Plane(plane_window),
            audio: if options.sound {
                Audio::System
            } else {
                Audio::Discarded
            },
            cache: cache.to_owned(),
        };
        let started = Instant::now();
        let (player, events) = Player::start(&settings)
            .map_err(|error| format!("the player, with {}: {error}", options.mpv.display()))?;
        let player = Arc::new(player);
        let forwarder_proxy = proxy.clone();
        let forwarder = thread::Builder::new()
            .name("aerith-testbed-events".to_owned())
            .spawn(move || forward(&events, &forwarder_proxy))
            .map_err(|error| format!("a thread: {error}"))?;
        let shared = Arc::new(Shared::default());
        let stats = options.stats.then(|| StatsSource {
            player: Arc::clone(&player),
            shared: Arc::clone(&shared),
            // The window's loop runs on the process's first thread, whose id is the process's.
            main_thread: std::process::id(),
        });
        let timeline = Timeline {
            seconds: options.seconds,
            pause_after: options.pause_after,
            toggle_controls: options.toggle_controls,
        };
        let schedule = Schedule::start(started, timeline, stats, proxy)
            .map_err(|error| format!("a thread: {error}"))?;
        let mut session = Self {
            schedule,
            player,
            forwarder: Some(forwarder),
            gpu,
            surfaces,
            window,
            shared,
            source,
            order: options.order,
            letterbox: options.letterbox,
            span: options.seconds,
            open_id: None,
            opened: None,
            paused: false,
            clock: None,
            video_size: None,
            reported: None,
            visibility: Visibility::default(),
            pointer: None,
            placed: None,
            root_commit_owed: true,
            drawn: None,
            frame_asked: None,
            frame_late: false,
            retry_at: None,
            drawn_has_buffer: false,
        };
        // Shown from the start: mpv's first frame appears as soon as it is drawn.
        if let Some(plane) = session.player.plane() {
            plane.show();
        }
        session.place_plane();
        Ok(session)
    }

    /// Ends the run in the order the window's surfaces need.
    fn close(self) {
        let Self {
            schedule,
            player,
            forwarder,
            gpu,
            surfaces,
            window,
            ..
        } = self;
        drop(schedule);
        if let Some(plane) = player.plane() {
            plane.close();
        }
        drop(player);
        if let Some(forwarder) = forwarder {
            let _ = forwarder.join();
        }
        drop(gpu);
        drop(surfaces);
        drop(window);
    }

    fn window_event(&mut self, event: WindowEvent) -> Flow {
        let now = Instant::now();
        match event {
            WindowEvent::CloseRequested => return Flow::Quit(Ok(())),
            WindowEvent::Resized(size) => {
                self.gpu.resize((size.width, size.height));
                self.surfaces.resize(logical(&self.window));
                self.place_plane();
                self.root_commit_owed = true;
            }
            // winit asks for one after each configure, which a commit answers.
            WindowEvent::RedrawRequested => self.root_commit_owed = true,
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = Some(position);
                self.visibility.show(now);
            }
            WindowEvent::CursorLeft { .. } => self.pointer = None,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // A click on hidden controls only shows them.
                let shown = self.visibility.shown();
                self.visibility.show(now);
                if shown {
                    self.click();
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                return self.key(event.logical_key.as_ref(), now);
            }
            _ => {}
        }
        Flow::Continue
    }

    fn key(&mut self, key: Key<&str>, now: Instant) -> Flow {
        let letter =
            |wanted: &str| matches!(key, Key::Character(c) if c.eq_ignore_ascii_case(wanted));
        if letter("h") {
            self.visibility.toggle(now);
            return Flow::Continue;
        }
        self.visibility.show(now);
        match key {
            Key::Named(NamedKey::Space) => self.play_or_pause(),
            Key::Named(NamedKey::ArrowLeft) => self.seek_by(false, now),
            Key::Named(NamedKey::ArrowRight) => self.seek_by(true, now),
            Key::Named(NamedKey::Escape) => return Flow::Quit(Ok(())),
            _ if letter("q") => return Flow::Quit(Ok(())),
            _ if letter("f") => {
                let fullscreen = self.window.fullscreen().is_none();
                self.window
                    .set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
            }
            _ => {}
        }
        Flow::Continue
    }

    /// A click where the pointer is, on the controls.
    fn click(&self) {
        let Some(pointer) = self.pointer else {
            return;
        };
        let scale = self.window.scale_factor();
        let point = ((pointer.x / scale) as f32, (pointer.y / scale) as f32);
        match self.controls().hit(point) {
            Some(Hit::PlayPause) => self.play_or_pause(),
            Some(Hit::Seek(fraction)) => {
                // The engine is gone: its failure, already on its way, ends the run.
                let _ = self.player.seek(self.span.mul_f32(fraction));
            }
            None => {}
        }
    }

    fn play_or_pause(&self) {
        // The engine is gone: its failure, already on its way, ends the run.
        let _ = if self.paused {
            self.player.play()
        } else {
            self.player.pause()
        };
    }

    fn seek_by(&self, forward: bool, now: Instant) {
        let position = self.position(now);
        let target = if forward {
            position.saturating_add(SEEK_STEP)
        } else {
            position.saturating_sub(SEEK_STEP)
        };
        let _ = self.player.seek(target);
    }

    fn user_event(&mut self, event: UserEvent) -> Flow {
        let now = Instant::now();
        match event {
            UserEvent::Schedule(Cue::Open) => {
                let opened = Instant::now();
                // A gone engine has its failure on its way, which ends the run.
                if let Ok(id) = self.player.open(&self.source) {
                    self.open_id = Some(id);
                    self.opened = Some(opened);
                    self.schedule.note(Note::Opened(opened));
                }
            }
            UserEvent::Schedule(Cue::Pause) => {
                let _ = self.player.pause();
            }
            UserEvent::Schedule(Cue::Controls(true)) => self.visibility.show(now),
            UserEvent::Schedule(Cue::Controls(false)) => self.visibility.hide(),
            UserEvent::Schedule(Cue::Quit(outcome)) => return Flow::Quit(outcome),
            UserEvent::Player(event) => return self.player_event(event, now),
            UserEvent::PlayerGone => {
                return Flow::Quit(Err("the player stopped without a word".to_owned()));
            }
        }
        Flow::Continue
    }

    fn player_event(&mut self, event: Event, now: Instant) -> Flow {
        let ours = |id: &OpenId| Some(*id) == self.open_id;
        match event {
            Event::Started(id) if ours(&id) => {
                if let Some(opened) = self.opened {
                    eprintln!(
                        "aerith-testbed: first frame {:.1} ms after the open",
                        (now - opened).as_secs_f64() * 1000.0
                    );
                }
                self.schedule.note(Note::FirstFrame(now));
            }
            Event::Paused(paused) => self.paused = paused,
            Event::Clock(id, clock) if ours(&id) => {
                self.clock = Some(clock);
                *self
                    .shared
                    .clock
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(clock);
            }
            Event::Report(id, report) if ours(&id) => {
                if let Some(video) = &report.video {
                    let drawn = video.drawn.as_ref().map_or("nothing".to_owned(), |colour| {
                        format!("{}/{}", colour.primaries, colour.transfer)
                    });
                    let line = format!(
                        "mpv plays {} {}x{}, decoded by {}, output {} drawing in {drawn}",
                        video.codec,
                        video.width,
                        video.height,
                        video.hardware_decoder.as_deref().unwrap_or("the CPU"),
                        video.output,
                    );
                    // A report comes at each change of the engine's, often of the sound alone.
                    if self.reported.as_ref() != Some(&line) {
                        eprintln!("aerith-testbed: {line}");
                        self.reported = Some(line);
                    }
                    self.video_size = Some((video.width, video.height));
                    self.place_plane();
                }
            }
            Event::PictureFailed(id) if ours(&id) => {
                return Flow::Quit(Err("mpv's video output failed: no picture".to_owned()));
            }
            Event::Ended(id, ending) if ours(&id) => {
                return match ending {
                    Ending::Finished | Ending::Stopped => {
                        eprintln!("aerith-testbed: the source ended");
                        Flow::Quit(Ok(()))
                    }
                    Ending::Failed(reason) => {
                        Flow::Quit(Err(format!("the source failed: {reason}")))
                    }
                };
            }
            Event::Refused { command, reason } => {
                eprintln!("aerith-testbed: mpv refused {command}: {reason}");
            }
            Event::EngineFailed(reason) => {
                return Flow::Quit(Err(format!("the player's engine failed: {reason}")));
            }
            _ => {}
        }
        Flow::Continue
    }

    /// One turn of the loop's work: what came on the testbed's queue, the hide timer, and a frame
    /// if one is due. Returns when the loop must wake next, if anything is timed.
    fn update(&mut self, now: Instant) -> Result<Option<Instant>, String> {
        if self.surfaces.frame_done()? {
            self.frame_asked = None;
        }
        if let Some(asked) = self.frame_asked
            && !self.frame_late
            && now >= asked + FRAME_LATE
        {
            self.frame_late = true;
            self.shared.ui_frames_late.fetch_add(1, Ordering::Relaxed);
        }
        self.visibility.expire(now);
        let frame = self.frame(now);
        let wanted = self.drawn != Some(frame) || self.root_commit_owed;
        let retrying = self.retry_at.is_some_and(|at| now < at);
        if wanted && self.frame_asked.is_none() && !retrying {
            self.present(frame, now)?;
        }
        // While a callback is awaited, the loop needs a deadline: only then does winit hand it the
        // wake-ups that bring nothing of winit's own, as the callback's.
        let next = match (self.frame_asked, self.retry_at) {
            (Some(asked), _) if now < asked + FRAME_LATE => Some(asked + FRAME_LATE),
            (Some(_), _) => Some(now + FRAME_LOOK_AGAIN),
            (None, Some(at)) if now < at => Some(at),
            (None, _) => self.next_change(now),
        };
        Ok([next, self.visibility.deadline()]
            .into_iter()
            .flatten()
            .min())
    }

    /// Draws `frame` and commits what changed.
    fn present(&mut self, frame: Frame, now: Instant) -> Result<(), String> {
        let shapes = self.shapes(frame);
        let (drawing, presented) = match self.order {
            Order::Below => {
                self.surfaces.request_frame();
                // The window's surface itself: its commit carries the plane's changes too.
                (true, self.gpu.draw(&shapes)?)
            }
            Order::Above => {
                let drawing = frame.shown && (self.drawn != Some(frame) || !self.drawn_has_buffer);
                let mut presented = false;
                if drawing {
                    self.surfaces.request_frame();
                    presented = self.gpu.draw(&shapes)?;
                    self.drawn_has_buffer |= presented;
                } else if !frame.shown && self.drawn_has_buffer {
                    self.surfaces.clear_drawn();
                    self.drawn_has_buffer = false;
                }
                // The drawn surface and the plane are synchronized: both change with this commit.
                self.surfaces.commit_root();
                self.root_commit_owed = false;
                (drawing, presented)
            }
        };
        if presented {
            self.frame_asked = Some(now);
            self.frame_late = false;
            self.root_commit_owed = false;
            self.shared.ui_frames.fetch_add(1, Ordering::Relaxed);
        }
        if presented || !drawing {
            self.drawn = Some(frame);
        } else {
            // The frame asked for goes with the next commit; this one is tried again.
            self.retry_at = Some(now + FRAME_RETRY);
        }
        self.surfaces.flush();
        Ok(())
    }

    /// What the controls show at `now`.
    fn frame(&self, now: Instant) -> Frame {
        let size = self.window.inner_size();
        let size = (size.width, size.height);
        if !self.visibility.shown() {
            return Frame {
                size,
                shown: false,
                paused: false,
                played: 0,
            };
        }
        Frame {
            size,
            shown: true,
            paused: self.paused,
            played: (self.progress(now) * self.bar_pixels()).round() as u32,
        }
    }

    /// The shapes of `frame`: none while the controls are hidden.
    fn shapes(&self, frame: Frame) -> Vec<Shape> {
        if !frame.shown {
            return Vec::new();
        }
        let bar = self.bar_pixels();
        let progress = if bar > 0.0 {
            frame.played as f32 / bar
        } else {
            0.0
        };
        controls_shapes(
            &self.controls(),
            self.window.scale_factor() as f32,
            frame.paused,
            progress,
        )
    }

    /// When the seek bar next moves by a pixel, while the controls show and the clock runs.
    fn next_change(&self, now: Instant) -> Option<Instant> {
        let clock = self.clock.filter(|clock| clock.running)?;
        let frame = self.drawn.filter(|frame| frame.shown)?;
        let bar = f64::from(self.bar_pixels());
        if bar <= 0.0 || f64::from(frame.played) >= bar {
            return None;
        }
        let next = (f64::from(frame.played) + 0.5) / bar * self.span.as_secs_f64();
        let left = (next - clock.position_at(now).as_secs_f64()) / clock.rate;
        // At least a millisecond: a rounding that puts the move an instant later than computed
        // costs one more wake-up, never a spin.
        let left = Duration::try_from_secs_f64(left).unwrap_or(Duration::ZERO);
        Some(now + left.max(Duration::from_millis(1)))
    }

    /// The played fraction of the seek bar's span.
    fn progress(&self, now: Instant) -> f32 {
        (self.position(now).as_secs_f64() / self.span.as_secs_f64()).clamp(0.0, 1.0) as f32
    }

    fn position(&self, now: Instant) -> Duration {
        self.clock
            .map_or(Duration::ZERO, |clock| clock.position_at(now))
    }

    fn controls(&self) -> Controls {
        let (width, height) = logical(&self.window);
        Controls::new((width as f32, height as f32))
    }

    /// The seek bar's length on the drawn surface, in pixels.
    fn bar_pixels(&self) -> f32 {
        self.controls().bar.width() * self.window.scale_factor() as f32
    }

    /// Places the plane for the window's size and the video's, if that moved it.
    fn place_plane(&mut self) {
        let rect = plane_rect(logical(&self.window), self.video_size, self.letterbox);
        if self.placed == Some(rect) {
            return;
        }
        if let Some(plane) = self.player.plane() {
            // `plane_rect` never makes a side of zero, the one place a plane refuses.
            let _ = plane.place(rect.x, rect.y, rect.width, rect.height);
        }
        self.placed = Some(rect);
        self.root_commit_owed = true;
    }
}

/// Hands each of the player's events to the loop, until either ends.
fn forward(events: &Receiver<Event>, proxy: &EventLoopProxy<UserEvent>) {
    for event in events {
        if proxy.send_event(UserEvent::Player(event)).is_err() {
            return;
        }
    }
    let _ = proxy.send_event(UserEvent::PlayerGone);
}

/// The window's size in logical pixels, which Wayland places surfaces in.
fn logical(window: &Window) -> (u32, u32) {
    let size: LogicalSize<f64> = window.inner_size().to_logical(window.scale_factor());
    (size.width.round() as u32, size.height.round() as u32)
}

/// winit's `wl_display` and the window's `wl_surface`.
fn wayland_handles(window: &Window) -> Result<(NonNull<c_void>, NonNull<c_void>), String> {
    let display = window
        .display_handle()
        .map_err(|error| format!("the window's display: {error}"))?
        .as_raw();
    let surface = window
        .window_handle()
        .map_err(|error| format!("the window's surface: {error}"))?
        .as_raw();
    match (display, surface) {
        (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) => {
            Ok((display.display, surface.surface))
        }
        _ => Err("the testbed runs on Wayland only".to_owned()),
    }
}
