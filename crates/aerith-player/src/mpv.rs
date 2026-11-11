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
//! it is read when its relation to time changes, and sent as a [`Clock`].
//!
//! **Each open is numbered**, and mpv numbers the playlist entry each load makes: the events of a
//! source carry the number of the open it came from, and each open ends with exactly one `Ended`,
//! even when a later open replaces it before it loads.
//!
//! **Secrets.** A stream's URL and headers travel over the connection only, never on mpv's command
//! line, in its title or in a log; the headers are options of their own load, which no other
//! source carries.
//!
//! [ADR-0003]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0003-mpv-in-its-own-process.md

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command as Process, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::event::{
    AudioFormat, AudioReport, Clock, Colour, Ending, Event, OpenId, Report, VideoReport,
};
use crate::player::{Audio, Command, EngineGone, Settings, Video};
use crate::source::{Header, Kind, Source};

/// The largest message accepted from mpv, in bytes. Its messages are a few hundred bytes; a larger
/// one is a fault, not something to buffer.
const LARGEST_MESSAGE: u64 = 1 << 20;
/// How long mpv may take to exit once asked, or once its connection broke, before it is killed.
const GRACE: Duration = Duration::from_secs(2);

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
        Video::Window => arguments.extend(
            [
                "--vo=gpu-next",
                // Wayland only: never mpv's DRM or direct-display contexts, which would take the
                // screen from the compositor when Wayland fails.
                "--gpu-context=waylandvk,wayland",
                "--hwdec=auto-safe",
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
    /// The player is dropped: mpv is to quit.
    Close,
}

/// The running engine, as the player holds it.
pub(crate) struct Engine {
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
    /// Starts mpv as `settings` say and the threads that drive it.
    pub(crate) fn start(settings: &Settings) -> io::Result<(Self, Receiver<Event>)> {
        let (ours, theirs) = UnixStream::pair()?;
        // The GPU driver's own shader caches go to Aerith's folder too, not the person's home.
        let driver_cache = settings.cache.join("driver");
        let child = Process::new(&settings.engine)
            .args(arguments(settings))
            .env("MESA_SHADER_CACHE_DIR", &driver_cache)
            .env("__GL_SHADER_DISK_CACHE_PATH", &driver_cache)
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // The spawn closed our copy of mpv's end: when mpv ends, the reader sees the end at once.
        Self::drive(ours, child)
    }

    /// Starts the threads that drive the engine `child` over `connection`. On failure, mpv is
    /// ended: closing the connection makes it quit.
    fn drive(connection: UnixStream, mut child: Child) -> io::Result<(Self, Receiver<Event>)> {
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
        let mut session = Session::new(writer, events);
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
    /// The position, for a [`Clock`] of this open.
    Position(OpenId),
    /// A property of the report, read for this open; the last of a reading sends the report.
    Property {
        open: OpenId,
        name: &'static str,
        last: bool,
    },
    /// An answer nothing depends on: the setup, a stop.
    Ignored,
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
    /// Whether mpv's playback is stopped: paused, seeking, waiting for data or idle.
    core_idle: bool,
    /// The values of [`REPORTED`] and `track-list` for the current source, by name.
    properties: HashMap<&'static str, Value>,
    /// The report last sent for the current source.
    report: Option<Report>,
    /// Whether the current source's lost picture was reported.
    picture_failed: bool,
}

impl Session {
    fn new(connection: UnixStream, events: Sender<Event>) -> Self {
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
            core_idle: true,
            properties: HashMap::new(),
            report: None,
            picture_failed: false,
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
            let Ok(message) = messages.recv() else {
                // Unreachable while the player holds its sender; mpv is ended all the same.
                break reap(child);
            };
            match message {
                Message::Command(command) => self.command(command),
                Message::Received(value) => self.received(&value),
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

    /// Asks for the current source's position, for a [`Clock`], unless a question for it is already
    /// on its way: its answer comes after the change that prompted this one, and is read with the
    /// state then.
    fn ask_position(&mut self) {
        let Some(open) = self.current.filter(|_| self.started) else {
            return;
        };
        if !self
            .pending
            .values()
            .any(|p| matches!(p, Pending::Position(of) if *of == open))
        {
            self.request(Pending::Position(open), json!(["get_property", "time-pos"]));
        }
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
        let clock = Clock {
            position: Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default(),
            at: Instant::now(),
            running: !self.core_idle,
        };
        let repeated = self
            .clock
            .is_some_and(|last| !last.running && !clock.running && last.position == clock.position);
        self.clock = Some(clock);
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
                self.picture_failed = false;
                self.properties.clear();
            }
            "playback-restart" => {
                if let Some(open) = self.current.filter(|_| !self.started) {
                    self.started = true;
                    self.emit(Event::Started(open));
                    self.read_report();
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

        /// Answers the question for the position with `seconds`.
        fn position(&mut self, seconds: f64) {
            let question = self.command();
            assert_eq!(question["command"], json!(["get_property", "time-pos"]));
            self.answer(&question, "success", json!(seconds));
        }
    }

    /// An engine over a fake mpv, its setup answered. `child` stands for mpv's process.
    fn driven(child: Child) -> (Engine, Receiver<Event>, FakeMpv) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (engine, events) = Engine::drive(ours, child).unwrap();
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

    #[test]
    fn mpv_runs_without_the_persons_configuration_and_never_on_drm() {
        for video in [Video::Discarded, Video::Window] {
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
        let window: Vec<OsString> = arguments(&settings(Video::Window, Audio::System));
        assert!(window.contains(&"--gpu-context=waylandvk,wayland".into()));
        assert!(window.contains(&"--vo=gpu-next".into()));
        assert!(
            !window
                .iter()
                .any(|a| a.to_string_lossy().starts_with("--ao"))
        );
        let discarded = arguments(&settings(Video::Discarded, Audio::System));
        assert!(discarded.contains(&"--vo=null".into()));
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
                   "index": -1, "options": {}})
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
                   "options": {"http-header-fields":
                       r"Authorization: Bearer token\, with a comma,Referer: https://example.org/"}})
        );
        // No global option is set: nothing else carries them.
        engine.open(file("/b.mkv")).unwrap();
        assert_eq!(mpv.load(2)["command"]["options"], json!({}));
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
        // The first source's report and position are asked and left unanswered.
        for _ in 0..=REPORTED.len() + 1 {
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
        let question = mpv.command();
        // A seek's restart while the question is on its way asks nothing more.
        mpv.send(json!({"event": "playback-restart"}));
        mpv.property("core-idle", json!(false));
        mpv.answer(&question, "success", json!(4.0));
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
        let question = mpv.command();
        mpv.send(json!({"event": "end-file", "playlist_entry_id": 1, "reason": "eof"}));
        assert_eq!(next(&events), Event::Ended(open, Ending::Finished));
        mpv.answer(&question, "success", json!(9.0));
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
    fn an_mpv_that_ends_without_reading_what_it_was_sent_closed_its_connection() {
        // The fake leaves the setup unread and closes: its end drops what it holds, a reset.
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (_engine, events) = Engine::drive(ours, exited()).unwrap();
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
        let (_engine, events) = Engine::drive(ours, child).unwrap();
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
        let (mut engine, events) = Engine::drive(ours, child).unwrap();
        assert_eq!(
            engine.close().as_deref(),
            Some("exited with code 0"),
            "it exited as the connection closed, not killed after the grace"
        );
        // Closing is no failure: the events end without one.
        assert_eq!(events.recv_timeout(WAIT).ok(), None);
    }
}
