// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! mpv as the engine, on Linux: a program apart, driven over its JSON IPC ([ADR-0003]).
//!
//! **The connection.** mpv gets one end of a socket pair as its standard input, and reads its IPC
//! there (`--input-ipc-client=fd://0`). No socket file exists, so nothing else can reach mpv's IPC,
//! which can run programs; and mpv quits by itself when the connection closes, so it never
//! outlives Aerith.
//!
//! **Two threads**, both asleep until something arrives, so that a paused or idle player wakes
//! nothing up:
//!
//! - the reader blocks on the socket, and hands each message from mpv to the engine thread;
//! - the engine thread blocks on its inbox, where the player's commands and mpv's messages arrive
//!   in order. It writes the commands, turns mpv's events into [`Event`]s, and reports how mpv
//!   ended when the connection breaks.
//!
//! **What mpv reports** comes from properties mpv is asked to observe: mpv sends them when they
//! change, never at a fixed rate. The position is not observed, since it changes at every frame:
//! it is read when its relation to time changes, and again every 10 s while it runs, with the
//! rate mpv plays at, and sent as a [`Clock`].
//!
//! **Each open is numbered**, and mpv numbers the playlist entry each load makes: the events of a
//! source carry the number of the open it came from, and each open ends with exactly one `Ended`,
//! even when a later open replaces it before it loads.
//!
//! **Display sync.** With a plane, mpv plays each source under audio sync, its default, until the
//! relay tells the display's rate: the engine thread asks the relay for a probe at the source's
//! first frame, and switches mpv to display sync (`video-sync=display-resample`) when the display
//! runs at a fixed rate. When the window or its display changes, it switches mpv back to audio
//! sync and probes again ([ADR-0021]).
//!
//! **Secrets.** A stream's URL and headers travel over the connection only, never on mpv's command
//! line, in its title or in a log; the headers are options of their own load, which no other
//! source carries.
//!
//! [ADR-0003]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0003-mpv-in-its-own-process.md
//! [ADR-0021]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0021-display-sync-at-a-fixed-rate-audio-sync-under-vrr.md

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command as Process, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::event::{
    AudioFormat, AudioReport, Clock, Colour, Ending, Event, OpenId, Report, VideoReport,
};
use crate::plane::{DisplayRate, Plane};
use crate::player::{Audio, Command, EngineGone, Settings, Video};
use crate::relay::{DisplayNews, Relay};
use crate::source::{Header, Kind, Source};

/// The largest message accepted from mpv, in bytes. Its messages are a few hundred bytes; a larger
/// one is a fault, not something to buffer.
const LARGEST_MESSAGE: u64 = 1 << 20;
/// How long mpv may take to exit once asked, or once its connection broke, before it is killed.
const GRACE: Duration = Duration::from_secs(2);
/// How often a running clock is read again: it bounds the clock's drift against mpv's own
/// position to a few milliseconds ([ADR-0016]).
///
/// [ADR-0016]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0016-the-clock-carries-the-engines-rate.md
const READ_AGAIN: Duration = Duration::from_secs(10);

/// The properties the report is made of.
const REPORTED: [&str; 9] = [
    "video-format",
    "hwdec-current",
    "video-params",
    "current-vo",
    "video-target-params",
    "audio-codec-name",
    "audio-params",
    "current-ao",
    "audio-out-params",
];

/// The properties whose changes mpv sends: the pause, whether the clock runs, the tracks (mpv
/// deselects the picture when its video output fails), and the report's. mpv does not always send
/// the report's again for a new source: they are also read at each source's first frame
/// ([`Session::read_report`]).
const OBSERVED: [&str; 12] = [
    "pause",
    "core-idle",
    "track-list",
    REPORTED[0],
    REPORTED[1],
    REPORTED[2],
    REPORTED[3],
    REPORTED[4],
    REPORTED[5],
    REPORTED[6],
    REPORTED[7],
    REPORTED[8],
];

/// mpv's command line: no configuration, script or key binding of the person's or of mpv's own,
/// no terminal, no log; the IPC on standard input; the outputs `settings` asks for.
fn arguments(settings: &Settings) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        // Neither the person's mpv.conf, input.conf, scripts nor resume files.
        "--no-config",
        "--resume-playback=no",
        "--save-position-on-quit=no",
        // None of the scripts and bindings mpv carries itself: Aerith is the interface.
        "--load-scripts=no",
        "--osc=no",
        "--ytdl=no",
        "--load-stats-overlay=no",
        "--load-console=no",
        "--load-auto-profiles=no",
        "--load-select=no",
        "--load-positioning=no",
        "--load-commands=no",
        "--load-context-menu=no",
        "--input-default-bindings=no",
        "--input-builtin-bindings=no",
        "--input-builtin-dragging=no",
        "--input-builtin-drag-and-drop=no",
        "--input-vo-keyboard=no",
        "--input-media-keys=no",
        // Nor mpv's clipboard, which is the person's: on Wayland its backend would also open a
        // connection of its own before the video output, and take the relay's socket from it.
        "--clipboard-backends-clr",
        // Nothing read from or written to the terminal: standard input is the IPC.
        "--terminal=no",
        "--input-ipc-client=fd://0",
        // Stays started between sources.
        "--idle=yes",
        // mpv's default title holds the media's name, and a stream's is its URL, token included:
        // its window and its sound stream show this instead.
        "--title=Aerith",
    ]
    .map(OsString::from)
    .to_vec();
    for (option, folder) in [
        ("--gpu-shader-cache-dir=", "shaders"),
        ("--icc-cache-dir=", "icc"),
    ] {
        let mut argument = OsString::from(option);
        argument.push(settings.cache.join(folder));
        arguments.push(argument);
    }
    match settings.video {
        Video::Discarded => arguments.push("--vo=null".into()),
        Video::Plane(_) => arguments.extend(
            [
                "--vo=gpu-next",
                // Wayland only: never mpv's DRM or direct-display contexts, which would take the
                // screen from the compositor when Wayland fails.
                "--gpu-context=waylandvk,wayland",
                "--hwdec=auto-safe",
                // The window and the video output are made at start, ready before the first open.
                "--force-window=yes",
                // The pointer is the interface's.
                "--input-cursor-passthrough=yes",
            ]
            .map(OsString::from),
        ),
    }
    if settings.audio == Audio::Discarded {
        arguments.push("--ao=null".into());
    }
    arguments
}

/// What reaches the engine thread, in order.
enum Message {
    /// A command of the player's.
    Command(Command),
    /// A message from mpv.
    Received(Value),
    /// mpv's connection broke, for this reason.
    Disconnected(String),
    /// How to ask the relay for a probe of the display's rate, sent once, with a plane, before
    /// any command.
    Probes(Box<dyn Fn(u64) + Send>),
    /// What the relay tells of the display.
    Display(DisplayNews),
    /// The player is dropped: mpv is to quit.
    Close,
}

/// The running engine, as the player holds it.
pub(crate) struct Engine {
    /// The plane mpv's picture shows in, if any. Declared first, it is closed after mpv has quit
    /// (`Drop` closes mpv before the fields drop): mpv never sees its window vanish while it runs.
    plane: Option<Plane>,
    inbox: Sender<Message>,
    /// The number of the last open. Locked while an open is numbered and sent, so that opens made
    /// from several threads reach the engine thread in the order of their numbers.
    opens: Mutex<u64>,
    /// Our end of the connection, to close it on drop even if the engine thread is stuck writing.
    connection: UnixStream,
    /// The engine thread, which returns how mpv ended.
    thread: Option<JoinHandle<String>>,
}

impl Engine {
    /// Starts mpv as `settings` say and the threads that drive it, and, for a plane, the relay
    /// mpv's Wayland connection goes through.
    pub(crate) fn start(settings: &Settings) -> io::Result<(Self, Receiver<Event>)> {
        let (ours, theirs) = UnixStream::pair()?;
        // The GPU driver's own shader caches go to Aerith's folder too, not the person's home.
        let driver_cache = settings.cache.join("driver");
        let mut process = Process::new(&settings.engine);
        process
            .args(arguments(settings))
            .env("MESA_SHADER_CACHE_DIR", &driver_cache)
            .env("__GL_SHADER_DISK_CACHE_PATH", &driver_cache)
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let wayland = match settings.video {
            Video::Discarded => None,
            Video::Plane(window) => {
                let (relay_end, mpv_end) = UnixStream::pair()?;
                inherit_wayland_socket(&mut process, &mpv_end)?;
                Some((window, relay_end, mpv_end))
            }
        };
        let child = process.spawn()?;
        // The spawn closed our copy of mpv's end: when mpv ends, the reader sees the end at once.
        let (mut engine, events) = Self::drive(ours, child, READ_AGAIN)?;
        if let Some((window, relay_end, mpv_end)) = wayland {
            // mpv holds its end now; the relay sees mpv's connection close when mpv ends.
            drop(mpv_end);
            let failed = relay_failure(engine.inbox.clone());
            let display_news = relay_display_news(engine.inbox.clone());
            // A relay that cannot start ends the engine with it: dropping it ends mpv.
            let relay = Relay::start(window, relay_end, failed, display_news)?;
            // Before any command: the player's caller has not had the engine yet.
            let _ = engine.inbox.send(Message::Probes(relay.probes()));
            engine.plane = Some(Plane::new(relay));
        }
        Ok((engine, events))
    }

    /// The plane mpv's picture shows in, if any.
    pub(crate) fn plane(&self) -> Option<&Plane> {
        self.plane.as_ref()
    }

    /// Starts the threads that drive the engine `child` over `connection`, reading a running clock
    /// again every `read_again`. On failure, mpv is ended: closing the connection makes it quit.
    fn drive(
        connection: UnixStream,
        mut child: Child,
        read_again: Duration,
    ) -> io::Result<(Self, Receiver<Event>)> {
        let abandon = |child: Option<&mut Child>, error| {
            let _ = connection.shutdown(Shutdown::Both);
            if let Some(child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(error)
        };
        let (reader, writer) = match connection
            .try_clone()
            .and_then(|reader| Ok((reader, connection.try_clone()?)))
        {
            Ok(clones) => clones,
            Err(error) => return abandon(Some(&mut child), error),
        };
        let (inbox, messages) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        let reader_inbox = inbox.clone();
        let reader = match thread::Builder::new()
            .name("aerith-mpv-read".to_owned())
            .spawn(move || read(reader, &reader_inbox))
        {
            Ok(reader) => reader,
            Err(error) => return abandon(Some(&mut child), error),
        };
        let mut session = Session::new(writer, events, read_again);
        let thread = match thread::Builder::new()
            .name("aerith-mpv".to_owned())
            .spawn(move || session.run(&messages, &mut child, reader))
        {
            Ok(thread) => thread,
            // mpv went with the thread's closure; it quits as the connection closes.
            Err(error) => return abandon(None, error),
        };
        Ok((
            Self {
                plane: None,
                inbox,
                opens: Mutex::new(0),
                connection,
                thread: Some(thread),
            },
            receiver,
        ))
    }

    /// Sends the open of `source`, numbered after the last one.
    pub(crate) fn open(&self, source: Source) -> Result<OpenId, EngineGone> {
        // A thread that panicked holding the lock left a number that is still right.
        let mut last = self
            .opens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *last += 1;
        let open = OpenId(*last);
        self.send(Command::Open(open, source)).map(|()| open)
    }

    pub(crate) fn send(&self, command: Command) -> Result<(), EngineGone> {
        self.inbox
            .send(Message::Command(command))
            .map_err(|_| EngineGone)
    }
}

impl Engine {
    /// Asks mpv to quit, waits for it as [`reap`] does, and says how it ended; `None` once closed.
    /// Dropping the player calls it; its answer lets a test see that mpv quit by itself.
    fn close(&mut self) -> Option<String> {
        // Close first, then the connection: the engine thread reads the close before the end of
        // the connection, and does not report mpv's exit as a failure.
        let _ = self.inbox.send(Message::Close);
        let _ = self.connection.shutdown(Shutdown::Both);
        self.thread.take().and_then(|thread| thread.join().ok())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close();
    }
}

/// What the relay calls when it ends mpv's Wayland connection, or loses the window's: the engine
/// ends mpv as it does when its IPC breaks, and reports the relay's reason. mpv may not quit by
/// itself when its Wayland connection ends.
fn relay_failure(inbox: Sender<Message>) -> Box<dyn Fn(String) + Send> {
    Box::new(move |reason| {
        let _ = inbox.send(Message::Disconnected(reason));
    })
}

/// What the relay calls with its news of the display: they reach the engine thread in order with
/// mpv's messages.
fn relay_display_news(inbox: Sender<Message>) -> Box<dyn Fn(DisplayNews) + Send> {
    Box::new(move |news| {
        let _ = inbox.send(Message::Display(news));
    })
}

/// Where mpv is told the compositor is, besides its inherited socket: a path that cannot exist, as
/// `/dev/null` is no folder. libwayland unsets `WAYLAND_SOCKET` once it took it; a second
/// connection of mpv's then looks at `WAYLAND_DISPLAY`, and without it would try the session's
/// compositor at `$XDG_RUNTIME_DIR/wayland-0`. With this path it fails at once.
///
/// mpv's `wayland` context, its fallback after `waylandvk`, makes such a second connection when
/// Vulkan fails after connecting (no device libplacebo takes, a swapchain refused): that fallback
/// then fails too, and mpv shows nothing rather than open a window of its own. When Vulkan fails
/// before connecting (no Vulkan driver at all, as on older GPUs), the fallback takes the inherited
/// socket and draws through the relay with OpenGL.
const NO_COMPOSITOR: &str = "/dev/null/no-compositor";

/// Has mpv inherit `socket` as its Wayland connection, `WAYLAND_SOCKET`, which libwayland takes
/// before any other way to connect. mpv is told of no other compositor: `WAYLAND_DISPLAY` names
/// [`NO_COMPOSITOR`], and `DISPLAY` is removed.
fn inherit_wayland_socket(process: &mut Process, socket: &UnixStream) -> io::Result<()> {
    // A descriptor above the standard three, which the spawn sets up for mpv: the socket keeps its
    // number in mpv.
    let inherited: OwnedFd = rustix::io::fcntl_dupfd_cloexec(socket, 3)?;
    let number = inherited.as_raw_fd();
    process
        .env("WAYLAND_SOCKET", number.to_string())
        .env("WAYLAND_DISPLAY", NO_COMPOSITOR)
        .env_remove("DISPLAY");
    let keep_across_exec = move || -> io::Result<()> {
        // The closure owns `inherited`: it stays open in the parent until the spawn is done, and
        // so in the child the fork makes.
        let _ = &inherited;
        // SAFETY: `number` is `inherited`'s, open in this process, as said above, until exec;
        // the borrow ends before.
        let socket = unsafe { BorrowedFd::borrow_raw(number) };
        rustix::io::fcntl_setfd(socket, rustix::io::FdFlags::empty())?;
        Ok(())
    };
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are allowed: it makes one `fcntl` system call, through rustix's raw system calls,
    // which neither allocate nor lock.
    unsafe {
        process.pre_exec(keep_across_exec);
    }
    Ok(())
}

/// Reads mpv's messages, one JSON value a line, and hands them to the engine thread until the
/// connection ends or a message is malformed.
fn read(connection: UnixStream, inbox: &Sender<Message>) {
    let mut reader = BufReader::new(connection);
    let mut line = Vec::new();
    let reason = loop {
        line.clear();
        match (&mut reader)
            .take(LARGEST_MESSAGE + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => break "mpv closed its connection".to_owned(),
            Ok(_) if line.last() != Some(&b'\n') => {
                break if line.len() as u64 > LARGEST_MESSAGE {
                    format!("mpv sent a message larger than {LARGEST_MESSAGE} bytes")
                } else {
                    "mpv closed its connection in the middle of a message".to_owned()
                };
            }
            Ok(_) => match parse(&line) {
                Ok(value) => {
                    if inbox.send(Message::Received(value)).is_err() {
                        return;
                    }
                }
                Err(error) => break format!("mpv sent malformed JSON: {error}"),
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            // mpv ended without reading all it was sent: the same end, told differently.
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {
                break "mpv closed its connection".to_owned();
            }
            Err(error) => break format!("mpv's connection failed: {error}"),
        }
    };
    let _ = inbox.send(Message::Disconnected(reason));
}

/// One message of mpv's. mpv copies the bytes of a file's tags into its JSON as they are, which
/// may not be UTF-8: such bytes are replaced rather than failing the whole connection.
fn parse(line: &[u8]) -> serde_json::Result<Value> {
    match std::str::from_utf8(line) {
        Ok(text) => serde_json::from_str(text),
        Err(_) => serde_json::from_str(&String::from_utf8_lossy(line)),
    }
}

/// What the engine thread waits for an answer to.
enum Pending {
    /// A command of the player's, by its name in [`Event::Refused`].
    Command(&'static str),
    /// The load of an open, which answers with mpv's number for it.
    Load(OpenId),
    /// A factor of the clock's rate, read just before the position of this open.
    Rate { open: OpenId, factor: RateFactor },
    /// The position, for a [`Clock`] of this open.
    Position(OpenId),
    /// A property of the report, read for this open; the last of a reading sends the report.
    Property {
        open: OpenId,
        name: &'static str,
        last: bool,
    },
    /// mpv's sync set for this open: display sync, or audio sync before a probe.
    Sync { open: OpenId, display: bool },
    /// An answer nothing depends on: the setup, a stop.
    Ignored,
}

/// The two factors of a clock's rate, read from mpv.
#[derive(Clone, Copy)]
enum RateFactor {
    /// `speed`: the speed asked of mpv.
    Speed,
    /// `video-speed-correction`: mpv's own change of speed to match the display, 1 unless it
    /// syncs the video to the display.
    Correction,
}

impl RateFactor {
    fn property(self) -> &'static str {
        match self {
            Self::Speed => "speed",
            Self::Correction => "video-speed-correction",
        }
    }
}

/// The engine thread's state.
struct Session {
    connection: UnixStream,
    events: Sender<Event>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    /// Each open by mpv's number for its playlist entry, once mpv answered its load.
    entries: HashMap<u64, OpenId>,
    /// The opens sent that have neither started loading nor ended.
    waiting: BTreeSet<OpenId>,
    /// The open mpv is loading or playing, if mpv's entry is one of Aerith's.
    current: Option<OpenId>,
    /// Whether the current source has shown its first frame.
    started: bool,
    /// mpv's pause, once it said it.
    paused: Option<bool>,
    /// The clock last read for the current source.
    clock: Option<Clock>,
    /// The factors of the rate of the clock being read, each 1 until mpv answers it.
    speed: f64,
    correction: f64,
    /// How often a running clock is read again.
    read_again: Duration,
    /// When the clock is to be read again: set by each reading of a running clock, cleared when it
    /// comes and when the source changes.
    read_at: Option<Instant>,
    /// Whether mpv's playback is stopped: paused, seeking, waiting for data or idle.
    core_idle: bool,
    /// The values of [`REPORTED`] and `track-list` for the current source, by name.
    properties: HashMap<&'static str, Value>,
    /// The report last sent for the current source.
    report: Option<Report>,
    /// Whether the current source's lost picture was reported.
    picture_failed: bool,
    /// How to ask the relay for a probe of the display's rate; `None` without a plane, where mpv
    /// stays under audio sync.
    probes: Option<Box<dyn Fn(u64) + Send>>,
    /// The number of the last probe asked for: the relay's verdict on any other is stale.
    probe: u64,
    /// Whether mpv was set to display sync for the current source. Each source starts under
    /// audio sync, an option of its own load.
    display_sync: bool,
}

impl Session {
    fn new(connection: UnixStream, events: Sender<Event>, read_again: Duration) -> Self {
        Self {
            connection,
            events,
            next_id: 1,
            pending: HashMap::new(),
            entries: HashMap::new(),
            waiting: BTreeSet::new(),
            current: None,
            started: false,
            paused: None,
            clock: None,
            speed: 1.0,
            correction: 1.0,
            read_again,
            read_at: None,
            core_idle: true,
            properties: HashMap::new(),
            report: None,
            picture_failed: false,
            probes: None,
            probe: 0,
            display_sync: false,
        }
    }

    /// Drives mpv until its connection breaks or the player closes it, and returns how mpv ended.
    fn run(
        &mut self,
        messages: &Receiver<Message>,
        child: &mut Child,
        reader: JoinHandle<()>,
    ) -> String {
        for (id, name) in (1..).zip(OBSERVED) {
            self.request(Pending::Ignored, json!(["observe_property", id, name]));
        }
        let ended = loop {
            let Some(message) = self.next_message(messages) else {
                // Unreachable while the player holds its sender; mpv is ended all the same.
                break reap(child);
            };
            match message {
                Message::Command(command) => self.command(command),
                Message::Received(value) => self.received(&value),
                Message::Probes(probes) => self.probes = Some(probes),
                Message::Display(news) => self.display_news(news),
                Message::Disconnected(reason) => {
                    // mpv quits at the end of its connection: close it before waiting, so that an
                    // mpv that still runs is not kept waiting the whole grace.
                    let _ = self.connection.shutdown(Shutdown::Both);
                    let ended = reap(child);
                    self.emit(Event::EngineFailed(format!("{reason}; mpv {ended}")));
                    break ended;
                }
                Message::Close => {
                    let _ = self.connection.shutdown(Shutdown::Both);
                    break reap(child);
                }
            }
        };
        // Ends the reader's wait, whichever way the loop ended.
        let _ = self.connection.shutdown(Shutdown::Both);
        let _ = reader.join();
        ended
    }

    /// The next message, or `None` once the player's sender is gone. A running clock is read again
    /// when its time comes; otherwise the thread sleeps until something arrives.
    fn next_message(&mut self, messages: &Receiver<Message>) -> Option<Message> {
        loop {
            let Some(read_at) = self.read_at else {
                return messages.recv().ok();
            };
            match messages.recv_timeout(read_at.saturating_duration_since(Instant::now())) {
                Ok(message) => return Some(message),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    // Once per reading: a frozen mpv that never answers is not asked again and
                    // again.
                    self.read_at = None;
                    self.ask_position();
                }
            }
        }
    }

    fn command(&mut self, command: Command) {
        let pending = |name| Pending::Command(name);
        match command {
            Command::Open(open, source) => {
                let (target, headers) = match source.kind() {
                    Kind::File(path) => (path.clone(), Vec::new()),
                    Kind::Stream { url, headers } => (url.clone(), headers.clone()),
                };
                if headers.iter().any(|header| header.line().contains('\\')) {
                    self.emit(Event::Ended(
                        open,
                        Ending::Failed(
                            "a header holds a backslash, which mpv's list of headers cannot carry"
                                .to_owned(),
                        ),
                    ));
                    return;
                }
                // The headers are options of this load only: mpv sets them for its file and
                // restores them after, so that no other source ever carries them.
                let mut options = serde_json::Map::new();
                // Each source starts under audio sync, until the relay tells the display's rate;
                // an option of the load, mpv restores it at the source's end, whatever it was
                // set to during it.
                options.insert("video-sync".to_owned(), Value::from("audio"));
                if !headers.is_empty() {
                    options.insert(
                        "http-header-fields".to_owned(),
                        Value::from(header_list(&headers)),
                    );
                }
                self.waiting.insert(open);
                self.request(
                    Pending::Load(open),
                    json!({"name": "loadfile", "url": target, "flags": "replace", "index": -1,
                           "options": options}),
                );
            }
            Command::Play => self.request(pending("play"), json!(["set_property", "pause", false])),
            Command::Pause => {
                self.request(pending("pause"), json!(["set_property", "pause", true]))
            }
            // `exact`, rather than mpv's default precision, which its manual says may change.
            Command::Seek(position) => self.request(
                pending("seek"),
                json!(["seek", position.as_secs_f64(), "absolute+exact"]),
            ),
            Command::Stop => {
                // mpv clears its playlist: an open that has not started loading ends here, as mpv
                // will say nothing of it.
                let waiting: Vec<OpenId> = std::mem::take(&mut self.waiting).into_iter().collect();
                self.entries.retain(|_, open| !waiting.contains(open));
                for open in waiting {
                    self.emit(Event::Ended(open, Ending::Stopped));
                }
                self.request(pending("stop"), json!(["stop"]));
            }
        }
    }

    /// Sends one IPC command. A failed write is not reported here: the reader sees the broken
    /// connection and reports it.
    fn request(&mut self, pending: Pending, command: Value) {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({"command": command, "request_id": id}).to_string();
        line.push('\n');
        if self.connection.write_all(line.as_bytes()).is_ok() {
            self.pending.insert(id, pending);
        }
    }

    /// Asks for the current source's position and its rate, for a [`Clock`], unless a question for
    /// it is already on its way: its answer comes after the change that prompted this one, and is
    /// read with the state then. mpv answers in the order it was asked, so the rate's factors come
    /// before the position.
    fn ask_position(&mut self) {
        let Some(open) = self.current.filter(|_| self.started) else {
            return;
        };
        if self
            .pending
            .values()
            .any(|p| matches!(p, Pending::Position(of) if *of == open))
        {
            return;
        }
        self.speed = 1.0;
        self.correction = 1.0;
        for factor in [RateFactor::Speed, RateFactor::Correction] {
            self.request(
                Pending::Rate { open, factor },
                json!(["get_property", factor.property()]),
            );
        }
        self.request(Pending::Position(open), json!(["get_property", "time-pos"]));
    }

    /// Reads every property the report and the picture's check are made of. mpv does not send
    /// all of them again to an observer when one source replaces another of the same format, so
    /// the report of a source is never built from what was observed for the last one.
    fn read_report(&mut self) {
        let Some(open) = self.current else { return };
        // A reading on its way for this source answers for both; one for an earlier source is
        // dropped when it comes, and does not count.
        if self
            .pending
            .values()
            .any(|p| matches!(p, Pending::Property { open: of, .. } if *of == open))
        {
            return;
        }
        let names: Vec<&'static str> = REPORTED.into_iter().chain(["track-list"]).collect();
        for (index, name) in names.iter().enumerate() {
            let last = index + 1 == names.len();
            self.request(
                Pending::Property { open, name, last },
                json!(["get_property", name]),
            );
        }
    }

    fn emit(&self, event: Event) {
        // A caller that dropped the receiver no longer listens; the player still runs.
        let _ = self.events.send(event);
    }

    fn received(&mut self, message: &Value) {
        if let Some(event) = message["event"].as_str() {
            self.event(event, message);
        } else if let Some(id) = message["request_id"].as_u64() {
            self.answer(id, message);
        }
    }

    fn answer(&mut self, id: u64, answer: &Value) {
        let success = answer["error"] == "success";
        let error = || {
            answer["error"]
                .as_str()
                .unwrap_or("no reason given")
                .to_owned()
        };
        match self.pending.remove(&id) {
            Some(Pending::Command(command)) if !success => self.emit(Event::Refused {
                command,
                reason: error(),
            }),
            // An open already ended (stopped, or replaced) needs no number.
            Some(Pending::Load(open)) if self.waiting.contains(&open) => {
                match (success, answer["data"]["playlist_entry_id"].as_u64()) {
                    (true, Some(entry)) => {
                        self.entries.insert(entry, open);
                    }
                    (true, None) => {
                        self.waiting.remove(&open);
                        let reason = "mpv gave the load no number".to_owned();
                        self.emit(Event::Ended(open, Ending::Failed(reason)));
                    }
                    (false, _) => {
                        self.waiting.remove(&open);
                        self.emit(Event::Ended(open, Ending::Failed(error())));
                    }
                }
            }
            // A factor mpv cannot give, as the correction without a picture, stays 1.
            Some(Pending::Rate { open, factor }) if success && self.current == Some(open) => {
                if let Some(value) = answer["data"].as_f64() {
                    match factor {
                        RateFactor::Speed => self.speed = value,
                        RateFactor::Correction => self.correction = value,
                    }
                }
            }
            Some(Pending::Sync { open, display }) if self.current == Some(open) => {
                if !success {
                    self.display_sync = false;
                    self.emit(Event::Refused {
                        command: "video-sync",
                        reason: error(),
                    });
                    return;
                }
                if !display {
                    self.ask_probe();
                }
                // The clock's rate follows mpv's correction, which display sync changes.
                self.ask_position();
            }
            Some(Pending::Position(open)) if success && self.current == Some(open) => {
                if let Some(seconds) = answer["data"].as_f64() {
                    self.clock_read(open, seconds);
                }
            }
            Some(Pending::Property { open, name, last }) if self.current == Some(open) => {
                // An error is a property mpv does not have now: no such track.
                let value = if success {
                    answer["data"].clone()
                } else {
                    Value::Null
                };
                self.properties.insert(name, value);
                if last {
                    self.send_report();
                    self.check_picture();
                }
            }
            // An answer for a source that has ended, or one nothing depends on.
            _ => {}
        }
    }

    /// Sends the clock read at `seconds` for `open`, unless it says nothing new: a stopped clock
    /// at the same position (a seek's restart after the change that stopped the clock).
    fn clock_read(&mut self, open: OpenId, seconds: f64) {
        // mpv's values are finite and positive; anything else is read as real time.
        let rate = Some(self.speed * self.correction)
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .unwrap_or(1.0);
        let clock = Clock {
            position: Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default(),
            at: Instant::now(),
            running: !self.core_idle,
            rate,
        };
        let repeated = self
            .clock
            .is_some_and(|last| !last.running && !clock.running && last.position == clock.position);
        self.clock = Some(clock);
        self.read_at = clock.running.then(|| clock.at + self.read_again);
        if !repeated {
            self.emit(Event::Clock(open, clock));
        }
    }

    fn event(&mut self, event: &str, message: &Value) {
        let entry = message["playlist_entry_id"].as_u64();
        match event {
            "property-change" => {
                let name = message["name"].as_str().unwrap_or_default();
                let data = message.get("data").cloned().unwrap_or(Value::Null);
                self.property(name, data);
            }
            "start-file" => {
                // An entry that is not one of Aerith's opens (a playlist's) plays as no source:
                // its events are not sent.
                self.current = entry
                    .and_then(|entry| self.entries.get(&entry).copied())
                    .filter(|open| self.waiting.contains(open));
                if let Some(open) = self.current {
                    self.waiting.remove(&open);
                    self.end_superseded(open);
                }
                // Each source gets its own report, clock and check, even ones the same as the
                // last source's.
                self.started = false;
                self.report = None;
                self.clock = None;
                self.read_at = None;
                self.picture_failed = false;
                self.properties.clear();
                self.display_sync = false;
            }
            "playback-restart" => {
                if let Some(open) = self.current.filter(|_| !self.started) {
                    self.started = true;
                    self.emit(Event::Started(open));
                    self.read_report();
                    self.ask_probe();
                }
                self.ask_position();
            }
            "audio-reconfig" | "video-reconfig" if self.started => self.read_report(),
            "end-file" => {
                let Some(open) = entry.and_then(|entry| self.entries.remove(&entry)) else {
                    return;
                };
                self.end_superseded(open);
                let was_current = self.current == Some(open);
                if was_current {
                    self.current = None;
                    self.started = false;
                    self.read_at = None;
                }
                if !(self.waiting.remove(&open) || was_current) {
                    return;
                }
                let ending = match message["reason"].as_str() {
                    Some("eof") => Ending::Finished,
                    Some("stop" | "quit") => Ending::Stopped,
                    Some("error") => Ending::Failed(
                        message["file_error"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_owned(),
                    ),
                    Some("redirect") => {
                        // A playlist, which mpv would replace by its entries, without the
                        // source's headers: the player plays one media an open. Stopped unless a
                        // later open has been sent since, which a stop would end.
                        if !self.waiting.iter().any(|later| *later > open) {
                            self.request(Pending::Ignored, json!(["stop"]));
                        }
                        Ending::Failed("the source is a playlist".to_owned())
                    }
                    other => Ending::Failed(format!("ended for an unknown reason: {other:?}")),
                };
                self.emit(Event::Ended(open, ending));
            }
            _ => {}
        }
    }

    /// Asks the relay for a probe of the display's rate, mpv under audio sync, if there is a
    /// plane.
    fn ask_probe(&mut self) {
        if let Some(probes) = &self.probes {
            self.probe += 1;
            probes(self.probe);
        }
    }

    /// Acts on the relay's news of the display, for the source playing: display sync at a fixed
    /// rate; after a change, audio sync again, and a new probe.
    fn display_news(&mut self, news: DisplayNews) {
        let Some(open) = self.current.filter(|_| self.started) else {
            return;
        };
        match news {
            DisplayNews::Rate {
                probe,
                rate: DisplayRate::Fixed,
            } if probe == self.probe && !self.display_sync => {
                self.set_sync(open, true);
            }
            // Under VRR mpv stays under audio sync; a verdict on an older probe is stale.
            DisplayNews::Rate { .. } => {}
            DisplayNews::Changed if self.display_sync => self.set_sync(open, false),
            DisplayNews::Changed => self.ask_probe(),
        }
    }

    /// Sets mpv's sync for `open`: display sync, or audio sync, after which the relay probes again
    /// once mpv has answered.
    fn set_sync(&mut self, open: OpenId, display: bool) {
        self.display_sync = display;
        let value = if display { "display-resample" } else { "audio" };
        self.request(
            Pending::Sync { open, display },
            json!(["set_property", "video-sync", value]),
        );
    }

    /// Ends the opens sent before `open` that never started loading: mpv dropped them when a
    /// later one replaced them, and says nothing of them.
    fn end_superseded(&mut self, open: OpenId) {
        let earlier: Vec<OpenId> = self.waiting.range(..open).copied().collect();
        for dropped in earlier {
            self.waiting.remove(&dropped);
            self.emit(Event::Ended(dropped, Ending::Stopped));
        }
    }

    fn property(&mut self, name: &str, data: Value) {
        match name {
            "pause" => {
                let Some(paused) = data.as_bool() else { return };
                // mpv's first value is the state at start, not a change.
                if self
                    .paused
                    .replace(paused)
                    .is_some_and(|before| before != paused)
                {
                    self.emit(Event::Paused(paused));
                }
            }
            "core-idle" => {
                let Some(idle) = data.as_bool() else { return };
                let changed = self.core_idle != idle;
                self.core_idle = idle;
                if changed {
                    self.ask_position();
                }
            }
            "track-list" => {
                self.properties.insert("track-list", data);
                self.check_picture();
            }
            _ => {
                // Only the names asked for are kept: the map stays bounded.
                let Some(name) = REPORTED.into_iter().find(|reported| *reported == name) else {
                    return;
                };
                self.properties.insert(name, data);
                self.send_report();
            }
        }
    }

    /// Reports, once a source, a picture that does not play: mpv deselects the video track when
    /// its video output fails, and plays the sound alone. Aerith never deselects it itself.
    fn check_picture(&mut self) {
        let Some(open) = self
            .current
            .filter(|_| self.started && !self.picture_failed)
        else {
            return;
        };
        let Some(tracks) = self.properties.get("track-list").and_then(Value::as_array) else {
            return;
        };
        let pictures: Vec<&Value> = tracks.iter().filter(|t| t["type"] == "video").collect();
        if !pictures.is_empty() && pictures.iter().all(|t| t["selected"] != true) {
            self.picture_failed = true;
            self.emit(Event::PictureFailed(open));
        }
    }

    /// Sends the report if it changed since the last one sent. A report of no track at all is not
    /// sent: mpv clears its properties as a source ends.
    fn send_report(&mut self) {
        let Some(open) = self.current.filter(|_| self.started) else {
            return;
        };
        let report = report(&self.properties);
        if report != Report::default() && self.report.as_ref() != Some(&report) {
            self.report = Some(report.clone());
            self.emit(Event::Report(open, Box::new(report)));
        }
    }
}

/// The headers as mpv's list option reads them: one `Name: value` an item, items separated by
/// commas, a comma inside an item escaped by a backslash. mpv reads no escape for a backslash
/// itself, so a header that holds one is refused before ([`Session::command`]).
fn header_list(headers: &[Header]) -> String {
    headers
        .iter()
        .map(|header| header.line().replace(',', "\\,"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The report, from the properties' values.
fn report(properties: &HashMap<&'static str, Value>) -> Report {
    let text = |name: &str| properties.get(name).and_then(Value::as_str);
    let colour = |params: &Value| -> Option<Colour> {
        Some(Colour {
            primaries: params["primaries"].as_str()?.to_owned(),
            transfer: params["gamma"].as_str()?.to_owned(),
            // mpv says 0 when the file tells no peak.
            peak: params["sig-peak"].as_f64().filter(|peak| *peak > 0.0),
        })
    };
    let audio_format = |params: &Value| -> Option<AudioFormat> {
        Some(AudioFormat {
            format: params["format"].as_str()?.to_owned(),
            sample_rate: u32::try_from(params["samplerate"].as_u64()?).ok()?,
            channels: params["channels"].as_str()?.to_owned(),
        })
    };
    let dimension =
        |params: &Value, name: &str| -> Option<u32> { u32::try_from(params[name].as_u64()?).ok() };
    let video = (|| {
        let params = properties.get("video-params")?;
        Some(VideoReport {
            codec: text("video-format")?.to_owned(),
            // mpv says `no` when it decodes in software.
            hardware_decoder: text("hwdec-current")
                .filter(|decoder| !decoder.is_empty() && *decoder != "no")
                .map(str::to_owned),
            width: dimension(params, "w")?,
            height: dimension(params, "h")?,
            decoded: colour(params)?,
            output: text("current-vo").unwrap_or("none").to_owned(),
            drawn: properties.get("video-target-params").and_then(colour),
        })
    })();
    let audio = (|| {
        Some(AudioReport {
            codec: text("audio-codec-name")?.to_owned(),
            decoded: audio_format(properties.get("audio-params")?)?,
            output: text("current-ao").unwrap_or("none").to_owned(),
            sent: properties.get("audio-out-params").and_then(audio_format),
        })
    })();
    Report { video, audio }
}

/// Waits for mpv to exit, up to [`GRACE`], kills it if it has not, and says how it ended.
fn reap(child: &mut Child) -> String {
    let deadline = Instant::now() + GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return match (status.code(), status.signal()) {
                    (Some(code), _) => format!("exited with code {code}"),
                    (None, Some(signal)) => format!("was killed by signal {signal}"),
                    (None, None) => "ended".to_owned(),
                };
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return "did not exit in time and was killed".to_owned();
            }
            Err(error) => return format!("could not be waited for: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{Header, Source};
    use std::path::PathBuf;
    use std::sync::Arc;

    const WAIT: Duration = Duration::from_secs(5);

    /// The other end of the connection, speaking as mpv would.
    struct FakeMpv {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
    }

    impl FakeMpv {
        fn new(stream: UnixStream) -> Self {
            stream.set_read_timeout(Some(WAIT)).unwrap();
            Self {
                reader: BufReader::new(stream.try_clone().unwrap()),
                writer: stream,
            }
        }

        /// The next command the engine sent.
        fn command(&mut self) -> Value {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("a command in time");
            serde_json::from_str(&line).unwrap()
        }

        fn answer(&mut self, request: &Value, error: &str, data: Value) {
            let id = request["request_id"].clone();
            self.send(json!({"request_id": id, "error": error, "data": data}));
        }

        /// Answers the next command with success, and returns it.
        fn accept(&mut self) -> Value {
            let command = self.command();
            self.answer(&command, "success", Value::Null);
            command
        }

        /// Answers the next command, a load, with mpv's number for its entry, and returns it.
        fn load(&mut self, entry: u64) -> Value {
            let command = self.command();
            assert_eq!(command["command"]["name"], "loadfile", "{command}");
            self.answer(&command, "success", json!({"playlist_entry_id": entry}));
            command
        }

        fn send(&mut self, value: Value) {
            writeln!(self.writer, "{value}").unwrap();
        }

        fn property(&mut self, name: &str, data: Value) {
            self.send(json!({"event": "property-change", "id": 1, "name": name, "data": data}));
        }

        /// Answers the reading of the report's properties with `values`, and null for the others.
        fn report_read(&mut self, values: &Value) {
            for name in REPORTED.into_iter().chain(["track-list"]) {
                let question = self.command();
                assert_eq!(question["command"], json!(["get_property", name]));
                match values.get(name) {
                    Some(value) => self.answer(&question, "success", value.clone()),
                    None => self.answer(&question, "property unavailable", Value::Null),
                }
            }
        }

        /// Answers the question for the position with `seconds`, at mpv's own speed.
        fn position(&mut self, seconds: f64) {
            self.position_at_rate(seconds, json!(1.0), json!(1.0));
        }

        /// Answers the questions for the clock: the speed, the correction (null: mpv cannot give
        /// it) and the position.
        fn position_at_rate(&mut self, seconds: f64, speed: Value, correction: Value) {
            for (name, value) in [("speed", speed), ("video-speed-correction", correction)] {
                let question = self.command();
                assert_eq!(question["command"], json!(["get_property", name]));
                if value.is_null() {
                    self.answer(&question, "property unavailable", Value::Null);
                } else {
                    self.answer(&question, "success", value);
                }
            }
            let question = self.command();
            assert_eq!(question["command"], json!(["get_property", "time-pos"]));
            self.answer(&question, "success", json!(seconds));
        }

        /// The clock's three questions, read and left unanswered.
        fn clock_questions(&mut self) -> [Value; 3] {
            ["speed", "video-speed-correction", "time-pos"].map(|name| {
                let question = self.command();
                assert_eq!(question["command"], json!(["get_property", name]));
                question
            })
        }

        /// Answers the clock's three questions: at mpv's own speed, at `seconds`.
        fn answer_clock(&mut self, questions: &[Value; 3], seconds: f64) {
            self.answer(&questions[0], "success", json!(1.0));
            self.answer(&questions[1], "success", json!(1.0));
            self.answer(&questions[2], "success", json!(seconds));
        }

        /// Fails if the engine sends a command within a moment. An absence is proven only as far
        /// as this wait goes.
        fn silent(&mut self, wait: Duration) {
            self.reader.get_ref().set_read_timeout(Some(wait)).unwrap();
            let mut line = String::new();
            let read = self.reader.read_line(&mut line);
            self.reader.get_ref().set_read_timeout(Some(WAIT)).unwrap();
            assert!(
                matches!(&read, Err(error) if error.kind() == io::ErrorKind::WouldBlock),
                "no command expected, got {read:?} {line}"
            );
        }
    }

    /// An engine over a fake mpv, its setup answered. `child` stands for mpv's process. A running
    /// clock is not read again within a test, but by [`driven_reading_every`].
    fn driven(child: Child) -> (Engine, Receiver<Event>, FakeMpv) {
        driven_reading_every(child, Duration::from_secs(3600))
    }

    /// An engine over a fake mpv that reads a running clock again every `read_again`.
    fn driven_reading_every(
        child: Child,
        read_again: Duration,
    ) -> (Engine, Receiver<Event>, FakeMpv) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (engine, events) = Engine::drive(ours, child, read_again).unwrap();
        let mut mpv = FakeMpv::new(theirs);
        for name in OBSERVED {
            let command = mpv.accept();
            assert_eq!(command["command"][0], "observe_property");
            assert_eq!(command["command"][2], name);
        }
        (engine, events, mpv)
    }

    /// A process that stands for mpv and has exited already, so that dropping the engine reaps it
    /// at once: the fake mpv is the connection, not this process.
    fn exited() -> Child {
        let mut child = Process::new("true").spawn().unwrap();
        while child.try_wait().unwrap().is_none() {
            thread::sleep(Duration::from_millis(1));
        }
        child
    }

    /// A process that stands for an mpv that will not exit.
    fn sleeping() -> Child {
        Process::new("sleep").arg("60").spawn().unwrap()
    }

    fn next(events: &Receiver<Event>) -> Event {
        events.recv_timeout(WAIT).expect("an event in time")
    }

    /// Fails on any event within a moment. An absence is proven only as far as this wait goes.
    fn quiet(events: &Receiver<Event>) {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(300)) {
            panic!("no event expected, got {event:?}");
        }
    }

    fn file(path: &str) -> Source {
        Source::file(path).unwrap()
    }

    /// Opens a file, loads it as mpv's entry `entry`, and brings it to its first frame, playing,
    /// with `report` as mpv's properties. Returns the open's number.
    fn start(
        engine: &Engine,
        mpv: &mut FakeMpv,
        events: &Receiver<Event>,
        entry: u64,
        report: &Value,
    ) -> OpenId {
        let open = engine.open(file("/corpus/clip.mkv")).unwrap();
        mpv.load(entry);
        mpv.send(json!({"event": "start-file", "playlist_entry_id": entry}));
        mpv.property("core-idle", json!(false));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(events), Event::Started(open));
        mpv.report_read(report);
        // A report needs a track: a picture's or a sound's parameters.
        if report.get("video-params").is_some() || report.get("audio-params").is_some() {
            assert!(matches!(next(events), Event::Report(o, _) if o == open));
        }
        mpv.position(0.0);
        assert!(matches!(next(events), Event::Clock(o, c) if o == open && c.running));
        open
    }

    /// mpv's properties for a 4K HDR10 picture on hardware and a TrueHD sound.
    fn hdr_report() -> Value {
        json!({
            "video-format": "hevc",
            "hwdec-current": "vaapi",
            "video-params": {"w": 3840, "h": 2160, "primaries": "bt.2020", "gamma": "pq",
                             "sig-peak": 4.926},
            "current-vo": "gpu-next",
            "video-target-params": {"primaries": "bt.709", "gamma": "gamma2.2", "sig-peak": 0.0},
            "audio-codec-name": "truehd",
            "audio-params": {"format": "s32", "samplerate": 48000, "channels": "7.1"},
            "current-ao": "pipewire",
            "track-list": [{"type": "video", "selected": true}, {"type": "audio", "selected": true}],
        })
    }

    fn settings(video: Video, audio: Audio) -> Settings {
        Settings {
            engine: PathBuf::from("mpv"),
            video,
            audio,
            cache: PathBuf::from("/cache/aerith"),
        }
    }

    /// A plane's settings, for mpv's arguments only: its window is never used.
    fn plane() -> Video {
        use raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle};
        let nowhere = std::ptr::NonNull::dangling();
        // SAFETY: no relay starts from this window, so nothing reaches through its addresses:
        // only mpv's arguments are made from the settings it is in.
        let window = unsafe {
            crate::plane::Window::new(
                WaylandDisplayHandle::new(nowhere),
                WaylandWindowHandle::new(nowhere),
                WaylandWindowHandle::new(nowhere),
            )
        };
        Video::Plane(window)
    }

    #[test]
    fn mpv_runs_without_the_persons_configuration_and_never_on_drm() {
        for video in [Video::Discarded, plane()] {
            let arguments: Vec<String> = arguments(&settings(video, Audio::Discarded))
                .into_iter()
                .map(|a| a.into_string().unwrap())
                .collect();
            for expected in [
                "--no-config",
                "--load-scripts=no",
                "--input-default-bindings=no",
                "--terminal=no",
                "--input-ipc-client=fd://0",
                "--clipboard-backends-clr",
                "--ao=null",
                "--title=Aerith",
                "--gpu-shader-cache-dir=/cache/aerith/shaders",
            ] {
                assert!(arguments.iter().any(|a| a == expected), "{expected}");
            }
            assert!(
                !arguments
                    .iter()
                    .any(|a| a.contains("drm") || a.contains("log-file"))
            );
        }
        let window: Vec<OsString> = arguments(&settings(plane(), Audio::System));
        assert!(window.contains(&"--gpu-context=waylandvk,wayland".into()));
        assert!(window.contains(&"--vo=gpu-next".into()));
        // The window is ready before the first open, and the pointer is the interface's.
        assert!(window.contains(&"--force-window=yes".into()));
        assert!(window.contains(&"--input-cursor-passthrough=yes".into()));
        assert!(
            !window
                .iter()
                .any(|a| a.to_string_lossy().starts_with("--ao"))
        );
        let discarded = arguments(&settings(Video::Discarded, Audio::System));
        assert!(discarded.contains(&"--vo=null".into()));
    }

    #[test]
    fn mpv_inherits_its_wayland_socket_and_no_compositor() {
        let (_relay_end, mpv_end) = UnixStream::pair().unwrap();
        let mut process = Process::new("mpv");
        inherit_wayland_socket(&mut process, &mpv_end).unwrap();
        let environment: HashMap<_, _> = process.get_envs().collect();
        let socket = environment[std::ffi::OsStr::new("WAYLAND_SOCKET")]
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<i32>().ok())
            .expect("a descriptor's number");
        assert!(socket > 2, "above the standard three: {socket}");
        assert_eq!(
            environment.get(std::ffi::OsStr::new("DISPLAY")),
            Some(&None),
            "DISPLAY is removed"
        );
        // A second Wayland connection of mpv's goes to a path that cannot exist, never to the
        // session's compositor: libwayland takes an absolute WAYLAND_DISPLAY as it is.
        let display = environment[std::ffi::OsStr::new("WAYLAND_DISPLAY")]
            .and_then(|value| value.to_str())
            .expect("WAYLAND_DISPLAY is set");
        assert!(display.starts_with('/'), "an absolute path: {display}");
        let refused = UnixStream::connect(display).expect_err("no compositor there");
        assert_eq!(
            refused.raw_os_error(),
            Some(rustix::io::Errno::NOTDIR.raw_os_error()),
            "{refused}"
        );
    }

    #[test]
    fn headers_are_listed_as_mpv_reads_them() {
        let headers = [
            Header::new("Authorization", "Bearer a, b").unwrap(),
            Header::new("Referer", "https://example.org/").unwrap(),
        ];
        assert_eq!(
            header_list(&headers),
            r"Authorization: Bearer a\, b,Referer: https://example.org/"
        );
    }

    #[test]
    fn a_header_with_a_backslash_ends_its_open_unloaded() {
        let (engine, events, _mpv) = driven(exited());
        let header = Header::new("X-Path", r"C:\x").unwrap();
        let source = Source::stream("https://example.org/a.mkv", vec![header]).unwrap();
        let open = engine.open(source).unwrap();
        assert!(matches!(next(&events), Event::Ended(o, Ending::Failed(_)) if o == open));
    }

    #[test]
    fn a_file_loads_without_headers_and_its_first_frame_starts_the_clock() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/corpus/clip.mkv")).unwrap();
        assert_eq!(
            mpv.load(1)["command"],
            json!({"name": "loadfile", "url": "/corpus/clip.mkv", "flags": "replace",
                   "index": -1, "options": {"video-sync": "audio"}})
        );
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        mpv.property("core-idle", json!(false));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started(open));
        mpv.report_read(&json!({}));
        let before = Instant::now();
        mpv.position(12.5);
        let Event::Clock(clock_open, clock) = next(&events) else {
            panic!("a clock")
        };
        assert_eq!(clock_open, open);
        assert_eq!(clock.position, Duration::from_millis(12_500));
        assert!(clock.running);
        assert!(clock.at >= before);
    }

    #[test]
    fn opens_are_numbered_in_order() {
        let (engine, _events, mut mpv) = driven(exited());
        let first = engine.open(file("/a.mkv")).unwrap();
        let second = engine.open(file("/b.mkv")).unwrap();
        assert!(first < second);
        assert_eq!(first, OpenId(1));
        mpv.load(1);
        mpv.load(2);
    }

    #[test]
    fn a_streams_headers_are_options_of_its_own_load() {
        let (engine, _events, mut mpv) = driven(exited());
        let headers = vec![
            Header::new("Authorization", "Bearer token, with a comma").unwrap(),
            Header::new("Referer", "https://example.org/").unwrap(),
        ];
        let source = Source::stream("https://example.org/a.mkv?key=1", headers).unwrap();
        engine.open(source).unwrap();
        assert_eq!(
            mpv.load(1)["command"],
            json!({"name": "loadfile", "url": "https://example.org/a.mkv?key=1",
                   "flags": "replace", "index": -1,
                   "options": {"video-sync": "audio", "http-header-fields":
                       r"Authorization: Bearer token\, with a comma,Referer: https://example.org/"}})
        );
        // No global option is set: nothing else carries them.
        engine.open(file("/b.mkv")).unwrap();
        assert_eq!(
            mpv.load(2)["command"]["options"],
            json!({"video-sync": "audio"})
        );
    }

    #[test]
    fn play_pause_seek_and_stop_send_their_commands() {
        let (engine, events, mut mpv) = driven(exited());
        for (command, expected) in [
            (Command::Play, json!(["set_property", "pause", false])),
            (Command::Pause, json!(["set_property", "pause", true])),
            (
                Command::Seek(Duration::from_millis(83_250)),
                json!(["seek", 83.25, "absolute+exact"]),
            ),
            (Command::Stop, json!(["stop"])),
        ] {
            engine.send(command).unwrap();
            assert_eq!(mpv.accept()["command"], expected);
        }
        quiet(&events);
    }

    #[test]
    fn a_refused_command_is_reported_by_name() {
        let (engine, events, mut mpv) = driven(exited());
        engine.send(Command::Seek(Duration::from_secs(1))).unwrap();
        let seek = mpv.command();
        mpv.answer(&seek, "property unavailable", Value::Null);
        assert_eq!(
            next(&events),
            Event::Refused {
                command: "seek",
                reason: "property unavailable".to_owned()
            }
        );
    }

    #[test]
    fn a_refused_load_ends_its_open() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/a.mkv")).unwrap();
        let load = mpv.command();
        mpv.answer(&load, "invalid parameter", Value::Null);
        assert_eq!(
            next(&events),
            Event::Ended(open, Ending::Failed("invalid parameter".to_owned()))
        );
    }

    #[test]
    fn an_open_replaced_before_it_loads_ends_once() {
        let (engine, events, mut mpv) = driven(exited());
        let first = engine.open(file("/a.mkv")).unwrap();
        let second = engine.open(file("/b.mkv")).unwrap();
        mpv.load(1);
        mpv.load(2);
        // mpv drops the first entry without a word, and starts the second.
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 2}));
        assert_eq!(next(&events), Event::Ended(first, Ending::Stopped));
        // A late end of the first, as mpv may send, ends nothing twice.
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "stop"}));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started(second));
    }

    #[test]
    fn a_source_that_replaces_a_playing_one_gets_its_whole_report() {
        let (engine, events, mut mpv) = driven(exited());
        let first = start(&engine, &mut mpv, &events, 1, &hdr_report());
        // The second source's sound is the same: mpv clears it as the first ends and does not
        // send it again to its observers. The report reads it.
        let second = engine.open(file("/b.mkv")).unwrap();
        mpv.load(2);
        mpv.property("audio-params", Value::Null);
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "stop"}));
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 2}));
        mpv.send(json!({"event": "playback-restart"}));
        let mut ended = false;
        loop {
            match next(&events) {
                Event::Ended(open, Ending::Stopped) if open == first => ended = true,
                Event::Report(open, _) if open == first => {}
                Event::Started(open) => {
                    assert_eq!(open, second);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(ended);
        mpv.report_read(&hdr_report());
        let Event::Report(open, report) = next(&events) else {
            panic!("a report")
        };
        assert_eq!(open, second);
        assert_eq!(report.audio.unwrap().codec, "truehd");
    }

    #[test]
    fn a_stop_ends_the_opens_that_have_not_started() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/a.mkv")).unwrap();
        mpv.load(1);
        engine.send(Command::Stop).unwrap();
        assert_eq!(next(&events), Event::Ended(open, Ending::Stopped));
        assert_eq!(mpv.accept()["command"], json!(["stop"]));
        // mpv cleared its playlist; were it to start the entry all the same, nothing is sent.
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        mpv.send(json!({"event": "playback-restart"}));
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "stop"}));
        quiet(&events);
    }

    #[test]
    fn an_open_stopped_before_mpv_answers_its_load_ends_once() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/a.mkv")).unwrap();
        let load = mpv.command();
        engine.send(Command::Stop).unwrap();
        assert_eq!(next(&events), Event::Ended(open, Ending::Stopped));
        mpv.answer(&load, "success", json!({"playlist_entry_id": 1}));
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        mpv.send(json!({"event": "playback-restart"}));
        quiet(&events);
    }

    #[test]
    fn readings_on_their_way_for_the_last_source_do_not_hold_back_the_next() {
        let (engine, events, mut mpv) = driven(exited());
        let first = engine.open(file("/a.mkv")).unwrap();
        mpv.load(1);
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started(first));
        // The first source's report and clock are asked and left unanswered: the report's
        // properties, the track list, and the clock's three questions.
        for _ in 0..REPORTED.len() + 4 {
            mpv.command();
        }
        let second = engine.open(file("/b.mkv")).unwrap();
        mpv.load(2);
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "stop"}));
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 2}));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Ended(first, Ending::Stopped));
        assert_eq!(next(&events), Event::Started(second));
        mpv.report_read(&hdr_report());
        assert!(matches!(next(&events), Event::Report(o, _) if o == second));
        mpv.position(0.0);
        assert!(matches!(next(&events), Event::Clock(o, _) if o == second));
    }

    #[test]
    fn a_playlists_stop_spares_an_open_sent_after_it() {
        let (engine, events, mut mpv) = driven(exited());
        let list = engine.open(file("/list.m3u")).unwrap();
        mpv.load(1);
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        let next_open = engine.open(file("/b.mkv")).unwrap();
        mpv.load(2);
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "redirect"}));
        assert!(matches!(next(&events), Event::Ended(o, Ending::Failed(_)) if o == list));
        // The next command mpv reads is the person's, not a stop that would end the later open.
        engine.send(Command::Play).unwrap();
        assert_eq!(
            mpv.accept()["command"],
            json!(["set_property", "pause", false])
        );
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 2}));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started(next_open));
    }

    #[test]
    fn opens_from_several_threads_reach_mpv_in_the_order_of_their_numbers() {
        const EACH: u64 = 500;
        let (engine, events, mut mpv) = driven(exited());
        let engine = std::sync::Arc::new(engine);
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let engine = std::sync::Arc::clone(&engine);
                thread::spawn(move || {
                    for _ in 0..EACH {
                        engine.open(file("/a.mkv")).unwrap();
                    }
                })
            })
            .collect();
        for entry in 1..=2 * EACH {
            mpv.load(entry);
        }
        for thread in threads {
            thread.join().unwrap();
        }
        // The entries start in the order mpv received their loads. Were two opens received out of
        // the order of their numbers, the start of the later-numbered one would end the other.
        for entry in 1..=2 * EACH {
            mpv.send(json!({"event": "start-file", "playlist_entry_id": entry}));
        }
        quiet(&events);
    }

    #[test]
    fn a_clock_carries_mpvs_speed_times_its_correction() {
        let (engine, events, mut mpv) = driven(exited());
        let open = start(&engine, &mut mpv, &events, 1, &json!({}));
        mpv.property("core-idle", json!(true));
        mpv.position_at_rate(5.0, json!(1.0), json!(0.99317));
        let Event::Clock(of, clock) = next(&events) else {
            panic!("a clock expected");
        };
        assert_eq!(of, open);
        assert!((clock.rate - 0.99317).abs() < 1e-12, "{clock:?}");
        // Without a picture mpv has no correction, and a speed it cannot give is not trusted: the
        // clock runs at real time.
        mpv.property("core-idle", json!(false));
        mpv.position_at_rate(6.0, json!(-3.0), Value::Null);
        let Event::Clock(_, clock) = next(&events) else {
            panic!("a clock expected");
        };
        assert!(clock.running);
        assert_eq!(clock.rate, 1.0);
    }

    #[test]
    fn a_running_clock_is_read_again_and_a_stopped_one_is_not() {
        let read_again = Duration::from_millis(100);
        let (engine, events, mut mpv) = driven_reading_every(exited(), read_again);
        let open = start(&engine, &mut mpv, &events, 1, &json!({}));
        // The clock runs: after `read_again`, the engine asks again, and sends the new reading.
        mpv.position_at_rate(1.0, json!(1.0), json!(1.001067));
        let Event::Clock(of, clock) = next(&events) else {
            panic!("a clock expected");
        };
        assert_eq!(of, open);
        assert!(clock.running && (clock.rate - 1.001067).abs() < 1e-12);
        // A reading left unanswered, as a frozen mpv would leave it, is not asked again.
        let questions = mpv.clock_questions();
        mpv.silent(read_again * 5);
        mpv.answer_clock(&questions, 2.0);
        assert!(matches!(next(&events), Event::Clock(_, c) if c.running));
        // Paused, the clock stops, and nothing is asked any more.
        mpv.property("core-idle", json!(true));
        mpv.position(2.5);
        assert!(matches!(next(&events), Event::Clock(_, c) if !c.running));
        mpv.silent(read_again * 5);
    }

    #[test]
    fn a_load_mpv_does_not_number_ends_its_open() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/a.mkv")).unwrap();
        let load = mpv.command();
        mpv.answer(&load, "success", Value::Null);
        assert_eq!(
            next(&events),
            Event::Ended(
                open,
                Ending::Failed("mpv gave the load no number".to_owned())
            )
        );
    }

    #[test]
    fn the_events_of_an_entry_that_is_no_open_are_not_sent() {
        let (_engine, events, mut mpv) = driven(exited());
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 7}));
        mpv.send(json!({"event": "playback-restart"}));
        mpv.property("core-idle", json!(true));
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 7, "reason": "eof"}));
        quiet(&events);
    }

    #[test]
    fn a_playlist_ends_its_open_and_is_stopped() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/list.m3u")).unwrap();
        mpv.load(1);
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        mpv.send(
            json!({"event": "end-file", "playlist_entry_id": 1, "reason": "redirect",
                        "playlist_insert_id": 2, "playlist_insert_num_entries": 3}),
        );
        assert_eq!(
            next(&events),
            Event::Ended(open, Ending::Failed("the source is a playlist".to_owned()))
        );
        assert_eq!(mpv.accept()["command"], json!(["stop"]));
    }

    #[test]
    fn the_pause_is_reported_when_it_changes_not_at_start() {
        let (_engine, events, mut mpv) = driven(exited());
        mpv.property("pause", json!(false));
        quiet(&events);
        mpv.property("pause", json!(true));
        assert_eq!(next(&events), Event::Paused(true));
        mpv.property("pause", json!(true));
        quiet(&events);
        mpv.property("pause", json!(false));
        assert_eq!(next(&events), Event::Paused(false));
    }

    #[test]
    fn the_clock_is_read_again_when_playback_stops_or_runs() {
        let (engine, events, mut mpv) = driven(exited());
        let open = start(&engine, &mut mpv, &events, 1, &json!({}));
        mpv.property("core-idle", json!(true));
        mpv.position(3.0);
        assert!(matches!(
            next(&events),
            Event::Clock(o, c) if o == open && !c.running && c.position == Duration::from_secs(3)
        ));
        // The same state again asks nothing.
        mpv.property("core-idle", json!(true));
        quiet(&events);
    }

    #[test]
    fn a_stopped_clock_at_the_same_position_is_sent_once() {
        let (engine, events, mut mpv) = driven(exited());
        start(&engine, &mut mpv, &events, 1, &json!({}));
        mpv.property("core-idle", json!(true));
        mpv.position(5.0);
        assert!(matches!(next(&events), Event::Clock(_, c) if !c.running));
        mpv.send(json!({"event": "playback-restart"}));
        mpv.position(5.0);
        quiet(&events);
    }

    #[test]
    fn one_question_for_the_position_at_a_time() {
        let (engine, events, mut mpv) = driven(exited());
        start(&engine, &mut mpv, &events, 1, &json!({}));
        mpv.property("core-idle", json!(true));
        let questions = mpv.clock_questions();
        // A seek's restart while the question is on its way asks nothing more.
        mpv.send(json!({"event": "playback-restart"}));
        mpv.property("core-idle", json!(false));
        mpv.answer_clock(&questions, 4.0);
        assert!(
            matches!(next(&events), Event::Clock(_, c) if c.running),
            "read with the state when the answer came"
        );
        quiet(&events);
        // The next change asks again.
        mpv.property("core-idle", json!(true));
        assert_eq!(mpv.command()["command"][0], "get_property");
    }

    #[test]
    fn a_position_answered_after_its_source_ended_sends_no_clock() {
        let (engine, events, mut mpv) = driven(exited());
        let open = start(&engine, &mut mpv, &events, 1, &json!({}));
        mpv.property("core-idle", json!(true));
        let questions = mpv.clock_questions();
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "eof"}));
        assert_eq!(next(&events), Event::Ended(open, Ending::Finished));
        mpv.answer_clock(&questions, 9.0);
        quiet(&events);
    }

    #[test]
    fn the_report_is_read_at_the_start_then_sent_at_each_change() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/a.mkv")).unwrap();
        mpv.load(1);
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        // Observed values before the first frame are not sent.
        mpv.property("video-format", json!("hevc"));
        quiet(&events);
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started(open));
        mpv.report_read(&hdr_report());
        let Event::Report(_, report) = next(&events) else {
            panic!("a report")
        };
        let video = report.video.as_ref().unwrap();
        assert_eq!(video.codec, "hevc");
        assert_eq!(video.hardware_decoder.as_deref(), Some("vaapi"));
        assert_eq!((video.width, video.height), (3840, 2160));
        assert_eq!(video.decoded.transfer, "pq");
        assert_eq!(video.decoded.peak, Some(4.926));
        assert_eq!(video.output, "gpu-next");
        let drawn = video.drawn.as_ref().unwrap();
        assert_eq!(drawn.primaries, "bt.709");
        assert_eq!(drawn.peak, None, "mpv's 0 is no peak");
        let audio = report.audio.as_ref().unwrap();
        assert_eq!(audio.codec, "truehd");
        assert_eq!(audio.decoded.channels, "7.1");
        assert_eq!(audio.sent, None, "the output has not opened");
        mpv.position(0.0);
        assert!(matches!(next(&events), Event::Clock(..)));

        mpv.property("hwdec-current", json!("no"));
        let Event::Report(_, report) = next(&events) else {
            panic!("a report")
        };
        assert_eq!(report.video.unwrap().hardware_decoder, None);
        // The same value again sends nothing, nor does a property the report is not made of.
        mpv.property("hwdec-current", json!("no"));
        mpv.property("media-title", json!("anything"));
        quiet(&events);
        // A reconfiguration reads the report again.
        mpv.send(json!({"event": "audio-reconfig"}));
        mpv.report_read(&hdr_report());
        assert!(matches!(next(&events), Event::Report(..)));
        // A report of no track at all, as a source ends, is not sent.
        for name in ["video-params", "audio-params"] {
            mpv.property(name, Value::Null);
        }
        let Event::Report(_, report) = next(&events) else {
            panic!("a report")
        };
        assert!(report.video.is_none() && report.audio.is_some());
        quiet(&events);
    }

    #[test]
    fn a_picture_the_output_could_not_show_is_reported_once() {
        let (engine, events, mut mpv) = driven(exited());
        let open = start(&engine, &mut mpv, &events, 1, &hdr_report());
        // The output fails: mpv drops the picture and plays on.
        let dropped =
            json!([{"type": "video", "selected": false}, {"type": "audio", "selected": true}]);
        mpv.property("track-list", dropped.clone());
        assert_eq!(next(&events), Event::PictureFailed(open));
        mpv.property("track-list", dropped);
        quiet(&events);
    }

    #[test]
    fn a_source_without_picture_is_no_failure() {
        let (engine, events, mut mpv) = driven(exited());
        let report = json!({"track-list": [{"type": "audio", "selected": true}]});
        start(&engine, &mut mpv, &events, 1, &report);
        quiet(&events);
    }

    #[test]
    fn each_end_maps_to_an_ending() {
        let (engine, events, mut mpv) = driven(exited());
        for (entry, end, expected) in [
            (1, json!({"reason": "eof"}), Ending::Finished),
            (2, json!({"reason": "stop"}), Ending::Stopped),
            (
                3,
                json!({"reason": "error", "file_error": "unrecognized file format"}),
                Ending::Failed("unrecognized file format".to_owned()),
            ),
        ] {
            let open = engine.open(file("/a.mkv")).unwrap();
            mpv.load(entry);
            mpv.send(json!({"event": "start-file", "playlist_entry_id": entry}));
            let mut message = end;
            message["event"] = json!("end-file");
            message["playlist_entry_id"] = json!(entry);
            mpv.send(message);
            assert_eq!(next(&events), Event::Ended(open, expected));
        }
    }

    #[test]
    fn a_crash_of_mpv_is_reported_with_how_it_ended() {
        // A process that dies of a segmentation fault, as a crashing decoder would; dead before
        // its connection closes, however loaded the machine.
        let mut crashed = Process::new("sh")
            .args(["-c", "kill -SEGV $$"])
            .spawn()
            .unwrap();
        while crashed.try_wait().unwrap().is_none() {
            thread::sleep(Duration::from_millis(1));
        }
        let (engine, events, mpv) = driven(crashed);
        drop(mpv);
        let Event::EngineFailed(reason) = next(&events) else {
            panic!("a failure")
        };
        assert!(reason.contains("mpv closed its connection"), "{reason}");
        assert!(reason.contains("killed by signal 11"), "{reason}");
        // The thread ends: commands then fail, and nothing waits on them.
        let deadline = Instant::now() + WAIT;
        while engine.send(Command::Play).is_ok() {
            assert!(Instant::now() < deadline, "commands fail once mpv is gone");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_relay_that_ends_mpvs_wayland_connection_ends_mpv_and_the_engine() {
        // An mpv that would go on without its window, as mpv may.
        let (engine, events, _mpv) = driven(sleeping());
        let failed = relay_failure(engine.inbox.clone());
        failed("the relay ended mpv's Wayland connection: a test".to_owned());
        let Event::EngineFailed(reason) = next(&events) else {
            panic!("a failure")
        };
        assert!(
            reason.starts_with("the relay ended mpv's Wayland connection: a test"),
            "{reason}"
        );
        assert!(
            reason.ends_with("did not exit in time and was killed"),
            "an mpv that does not quit is ended: {reason}"
        );
        // A second call, once the engine is gone, is no fault.
        failed("again".to_owned());
    }

    #[test]
    fn an_mpv_that_ends_without_reading_what_it_was_sent_closed_its_connection() {
        // The fake leaves the setup unread and closes: its end drops what it holds, a reset.
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (_engine, events) = Engine::drive(ours, exited(), READ_AGAIN).unwrap();
        thread::sleep(Duration::from_millis(100));
        drop(theirs);
        let Event::EngineFailed(reason) = next(&events) else {
            panic!("a failure")
        };
        assert!(reason.starts_with("mpv closed its connection"), "{reason}");
    }

    #[test]
    fn a_message_larger_than_the_limit_ends_the_engine_and_mpv() {
        let (_engine, events, mpv) = driven(sleeping());
        let mut writer = mpv.writer;
        let flood = thread::spawn(move || {
            let chunk = vec![b'x'; 1 << 16];
            let mut sent = 0;
            while sent <= LARGEST_MESSAGE + (1 << 16) && writer.write_all(&chunk).is_ok() {
                sent += chunk.len() as u64;
            }
        });
        let Event::EngineFailed(reason) = next(&events) else {
            panic!("a failure")
        };
        assert!(reason.contains("larger than"), "{reason}");
        assert!(
            reason.contains("did not exit in time and was killed"),
            "an mpv that does not quit is ended: {reason}"
        );
        flood.join().unwrap();
    }

    #[test]
    fn on_malformed_input_mpv_quits_at_once_as_its_connection_closes() {
        // cat reads its standard input, the connection, as mpv does, and exits at its end; the
        // test writes to the engine through another handle on the same end.
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut writer = theirs.try_clone().unwrap();
        let child = Process::new("cat")
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let (_engine, events) = Engine::drive(ours, child, READ_AGAIN).unwrap();
        writer.write_all(b"{\"event\": nope}\n").unwrap();
        let Event::EngineFailed(reason) = next(&events) else {
            panic!("a failure")
        };
        assert!(reason.contains("malformed"), "{reason}");
        assert!(reason.ends_with("mpv exited with code 0"), "{reason}");
    }

    #[test]
    fn bytes_that_are_not_utf8_are_replaced() {
        let (engine, events, mut mpv) = driven(exited());
        let open = engine.open(file("/a.mkv")).unwrap();
        mpv.load(1);
        mpv.writer
            .write_all(
                b"{\"event\":\"end-file\",\"playlist_entry_id\":1,\"reason\":\"error\",\"file_error\":\"\xff\"}\n",
            )
            .unwrap();
        assert_eq!(
            next(&events),
            Event::Ended(open, Ending::Failed("\u{fffd}".to_owned()))
        );
    }

    #[test]
    fn a_program_that_exits_at_once_is_reported() {
        let settings = Settings {
            engine: PathBuf::from("false"),
            ..settings(Video::Discarded, Audio::Discarded)
        };
        let (_engine, events) = Engine::start(&settings).unwrap();
        let Event::EngineFailed(reason) = next(&events) else {
            panic!("a failure")
        };
        assert!(reason.contains("exited with code 1"), "{reason}");
    }

    #[test]
    fn a_program_that_does_not_exist_fails_to_start() {
        let settings = Settings {
            engine: PathBuf::from("/nonexistent/mpv"),
            ..settings(Video::Discarded, Audio::Discarded)
        };
        assert!(Engine::start(&settings).is_err());
    }

    #[test]
    fn closing_the_player_ends_the_connection_and_the_engine_exits() {
        // cat reads its standard input, the connection, as mpv does, and exits at its end.
        let (ours, theirs) = UnixStream::pair().unwrap();
        let child = Process::new("cat")
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let (mut engine, events) = Engine::drive(ours, child, READ_AGAIN).unwrap();
        assert_eq!(
            engine.close().as_deref(),
            Some("exited with code 0"),
            "it exited as the connection closed, not killed after the grace"
        );
        // Closing is no failure: the events end without one.
        assert_eq!(events.recv_timeout(WAIT).ok(), None);
    }

    /// The probes the engine asked the relay for, by number, standing for the relay.
    type Probes = Arc<Mutex<Vec<u64>>>;

    /// Gives the engine a relay that records the probes it is asked for, as a plane's would.
    fn with_probes(engine: &Engine) -> Probes {
        let probes = Probes::default();
        let asked = Arc::clone(&probes);
        engine
            .inbox
            .send(Message::Probes(Box::new(move |number| {
                asked.lock().unwrap().push(number);
            })))
            .unwrap();
        probes
    }

    /// Tells the engine what the relay would.
    fn tell(engine: &Engine, news: DisplayNews) {
        engine.inbox.send(Message::Display(news)).unwrap();
    }

    fn fixed(probe: u64) -> DisplayNews {
        DisplayNews::Rate {
            probe,
            rate: DisplayRate::Fixed,
        }
    }

    /// Waits until the engine asked for `count` probes, and returns them.
    fn probes_asked(probes: &Probes, count: usize) -> Vec<u64> {
        let deadline = Instant::now() + WAIT;
        loop {
            let asked = probes.lock().unwrap().clone();
            if asked.len() >= count {
                return asked;
            }
            assert!(Instant::now() < deadline, "probes asked: {asked:?}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Answers the next command, which must set `video-sync` to `value`, as `error` says.
    fn sync_set(mpv: &mut FakeMpv, value: &str, error: &str) {
        let command = mpv.command();
        assert_eq!(
            command["command"],
            json!(["set_property", "video-sync", value])
        );
        mpv.answer(&command, error, Value::Null);
    }

    #[test]
    fn a_fixed_rate_switches_mpv_to_display_sync_and_reads_the_clock_again() {
        let (engine, events, mut mpv) = driven(exited());
        let probes = with_probes(&engine);
        let open = start(&engine, &mut mpv, &events, 1, &json!({}));
        assert_eq!(probes_asked(&probes, 1), [1], "a probe at the first frame");
        tell(&engine, fixed(1));
        sync_set(&mut mpv, "display-resample", "success");
        mpv.position_at_rate(3.0, json!(1.0), json!(1.001));
        let Event::Clock(clock_open, clock) = next(&events) else {
            panic!("a clock")
        };
        assert_eq!((clock_open, clock.rate), (open, 1.001));
        // Seeking brings no new probe; the next source does.
        mpv.send(json!({"event": "playback-restart"}));
        mpv.position(4.0);
        assert!(matches!(next(&events), Event::Clock(..)));
        assert_eq!(probes.lock().unwrap().len(), 1);
    }

    #[test]
    fn under_vrr_and_on_a_stale_verdict_mpv_stays_under_audio_sync() {
        let (engine, events, mut mpv) = driven(exited());
        let probes = with_probes(&engine);
        start(&engine, &mut mpv, &events, 1, &json!({}));
        probes_asked(&probes, 1);
        tell(
            &engine,
            DisplayNews::Rate {
                probe: 1,
                rate: DisplayRate::Variable,
            },
        );
        // A verdict on another probe than the last asked says nothing of the display now.
        tell(&engine, fixed(0));
        tell(&engine, fixed(2));
        mpv.silent(Duration::from_millis(300));
        quiet(&events);
    }

    #[test]
    fn a_change_under_display_sync_brings_audio_sync_then_a_new_probe() {
        let (engine, events, mut mpv) = driven(exited());
        let probes = with_probes(&engine);
        start(&engine, &mut mpv, &events, 1, &json!({}));
        probes_asked(&probes, 1);
        tell(&engine, fixed(1));
        sync_set(&mut mpv, "display-resample", "success");
        mpv.position(1.0);
        assert!(matches!(next(&events), Event::Clock(..)));
        tell(&engine, DisplayNews::Changed);
        let questions = {
            let command = mpv.command();
            assert_eq!(
                command["command"],
                json!(["set_property", "video-sync", "audio"])
            );
            // The new probe waits for mpv's answer: mpv may still present under display sync.
            thread::sleep(Duration::from_millis(100));
            assert_eq!(probes.lock().unwrap().len(), 1);
            mpv.answer(&command, "success", Value::Null);
            mpv.clock_questions()
        };
        assert_eq!(probes_asked(&probes, 2), [1, 2]);
        mpv.answer_clock(&questions, 2.0);
        assert!(matches!(next(&events), Event::Clock(..)));
        // The display is fixed still: display sync again.
        tell(&engine, fixed(2));
        sync_set(&mut mpv, "display-resample", "success");
    }

    #[test]
    fn a_change_under_audio_sync_brings_a_new_probe_alone() {
        let (engine, events, mut mpv) = driven(exited());
        let probes = with_probes(&engine);
        start(&engine, &mut mpv, &events, 1, &json!({}));
        probes_asked(&probes, 1);
        tell(&engine, DisplayNews::Changed);
        assert_eq!(probes_asked(&probes, 2), [1, 2]);
        mpv.silent(Duration::from_millis(300));
        // The verdict on the first probe came late: stale.
        tell(&engine, fixed(1));
        mpv.silent(Duration::from_millis(300));
    }

    #[test]
    fn news_without_a_source_playing_are_dropped() {
        let (engine, events, mut mpv) = driven(exited());
        let probes = with_probes(&engine);
        tell(&engine, DisplayNews::Changed);
        tell(&engine, fixed(0));
        mpv.silent(Duration::from_millis(300));
        assert!(probes.lock().unwrap().is_empty());
        let open = start(&engine, &mut mpv, &events, 1, &json!({}));
        probes_asked(&probes, 1);
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "eof"}));
        assert_eq!(next(&events), Event::Ended(open, Ending::Finished));
        tell(&engine, fixed(1));
        tell(&engine, DisplayNews::Changed);
        mpv.silent(Duration::from_millis(300));
        assert_eq!(probes.lock().unwrap().len(), 1);
    }

    #[test]
    fn each_source_starts_under_audio_sync_and_probes_again() {
        let (engine, events, mut mpv) = driven(exited());
        let probes = with_probes(&engine);
        start(&engine, &mut mpv, &events, 1, &json!({}));
        probes_asked(&probes, 1);
        tell(&engine, fixed(1));
        sync_set(&mut mpv, "display-resample", "success");
        mpv.position(1.0);
        assert!(matches!(next(&events), Event::Clock(..)));
        // The next source: its load asks for audio sync, mpv restoring it at the first's end.
        let open = engine.open(file("/corpus/next.mkv")).unwrap();
        assert_eq!(mpv.load(2)["command"]["options"]["video-sync"], "audio");
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "stop"}));
        assert!(matches!(next(&events), Event::Ended(_, Ending::Stopped)));
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 2}));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started(open));
        assert_eq!(probes_asked(&probes, 2), [1, 2]);
        // A change before the new verdict asks for no audio sync: mpv is under it already.
        tell(&engine, DisplayNews::Changed);
        mpv.report_read(&json!({}));
        mpv.position(0.0);
        assert!(matches!(next(&events), Event::Clock(..)));
        assert_eq!(probes_asked(&probes, 3), [1, 2, 3]);
        mpv.silent(Duration::from_millis(300));
    }

    #[test]
    fn a_refused_sync_is_reported_and_mpv_counted_under_audio_sync() {
        let (engine, events, mut mpv) = driven(exited());
        let probes = with_probes(&engine);
        start(&engine, &mut mpv, &events, 1, &json!({}));
        probes_asked(&probes, 1);
        tell(&engine, fixed(1));
        sync_set(&mut mpv, "display-resample", "property unavailable");
        assert_eq!(
            next(&events),
            Event::Refused {
                command: "video-sync",
                reason: "property unavailable".to_owned()
            }
        );
        // Under audio sync still, a change needs a new probe only.
        tell(&engine, DisplayNews::Changed);
        assert_eq!(probes_asked(&probes, 2), [1, 2]);
        mpv.silent(Duration::from_millis(300));
    }

    #[test]
    fn without_a_plane_no_probe_is_asked() {
        let (engine, events, mut mpv) = driven(exited());
        start(&engine, &mut mpv, &events, 1, &json!({}));
        mpv.silent(Duration::from_millis(300));
    }
}
