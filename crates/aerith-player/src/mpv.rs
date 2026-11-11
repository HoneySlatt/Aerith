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
//! **Secrets.** A stream's URL and headers travel over the connection only, never on mpv's command
//! line, and mpv writes no log.
//!
//! [ADR-0003]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0003-mpv-in-its-own-process.md

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::event::{AudioFormat, AudioReport, Clock, Colour, Ending, Event, Report, VideoReport};
use crate::player::{Audio, Command, EngineGone, Settings, Video};
use crate::source::Kind;

/// The largest message accepted from mpv, in bytes. Its messages are a few hundred bytes; a larger
/// one is a fault, not something to buffer.
const LARGEST_MESSAGE: u64 = 1 << 20;
/// How long mpv may take to exit once asked, or once its connection broke, before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// The properties whose changes mpv sends: the pause, whether the clock runs, the tracks and the
/// one selected (mpv drops the picture when its video output fails), and what the report is built
/// from.
const OBSERVED: [&str; 13] = [
    "pause",
    "core-idle",
    "track-list",
    "vid",
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
    /// Our end of the connection, to close it on drop even if the engine thread is stuck writing.
    connection: UnixStream,
    /// The engine thread, which returns how mpv ended.
    thread: Option<JoinHandle<String>>,
}

impl Engine {
    /// Starts mpv as `settings` say and the threads that drive it.
    pub(crate) fn start(settings: &Settings) -> io::Result<(Self, Receiver<Event>)> {
        let (ours, theirs) = UnixStream::pair()?;
        let child = Process::new(&settings.engine)
            .args(arguments(settings))
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
                connection,
                thread: Some(thread),
            },
            receiver,
        ))
    }

    pub(crate) fn send(&self, command: Command) -> Result<(), EngineGone> {
        self.inbox
            .send(Message::Command(command))
            .map_err(|_| EngineGone)
    }
}

impl Engine {
    /// Asks mpv to quit, waits for it as [`reap`] does, and says how it ended; `None` once closed.
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
    /// The position, for a [`Clock`].
    Position,
    /// The setup, which mpv does not refuse.
    Setup,
}

/// The engine thread's state.
struct Session {
    connection: UnixStream,
    events: Sender<Event>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    /// Whether the current source has shown its first frame.
    started: bool,
    /// mpv's pause, once it said it.
    paused: Option<bool>,
    /// The clock last read for the current source.
    clock: Option<Clock>,
    /// Whether mpv's playback is stopped: paused, seeking, waiting for data or idle.
    core_idle: bool,
    /// The observed properties' last values, by name.
    properties: HashMap<String, Value>,
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
            self.request(Pending::Setup, json!(["observe_property", id, name]));
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
                    // A connection that broke while mpv runs leaves mpv without orders: end it.
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
            Command::Open(source) => {
                let (target, headers) = match source.kind() {
                    Kind::File(path) => (path.clone(), Vec::new()),
                    Kind::Stream { url, headers } => {
                        (url.clone(), headers.iter().map(|h| h.line()).collect())
                    }
                };
                // Set before each load, emptied for a source without headers: the option outlives
                // the source it was set for.
                self.request(
                    pending("open"),
                    json!(["set_property", "http-header-fields", headers]),
                );
                self.request(pending("open"), json!(["loadfile", target]));
            }
            Command::Play => self.request(pending("play"), json!(["set_property", "pause", false])),
            Command::Pause => {
                self.request(pending("pause"), json!(["set_property", "pause", true]))
            }
            Command::Seek(position) => self.request(
                pending("seek"),
                json!(["seek", position.as_secs_f64(), "absolute"]),
            ),
            Command::Stop => self.request(pending("stop"), json!(["stop"])),
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

    /// Asks for the position, for a [`Clock`], unless a question is already on its way: its
    /// answer comes after the change that prompted this one, and is read with the state then.
    fn ask_position(&mut self) {
        if !self
            .pending
            .values()
            .any(|p| matches!(p, Pending::Position))
        {
            self.request(Pending::Position, json!(["get_property", "time-pos"]));
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
        match self.pending.remove(&id) {
            Some(Pending::Command(command)) if !success => self.emit(Event::Refused {
                command,
                reason: answer["error"]
                    .as_str()
                    .unwrap_or("no reason given")
                    .to_owned(),
            }),
            Some(Pending::Position) if success => {
                if let Some(seconds) = answer["data"].as_f64() {
                    let clock = Clock {
                        position: Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default(),
                        at: Instant::now(),
                        running: !self.core_idle,
                    };
                    // A stopped clock at the same position says nothing new (a seek's restart
                    // after the change that stopped the clock).
                    let repeated = self.clock.is_some_and(|last| {
                        !last.running && !clock.running && last.position == clock.position
                    });
                    self.clock = Some(clock);
                    if !repeated {
                        self.emit(Event::Clock(clock));
                    }
                }
            }
            // A position mpv no longer has (the source ended in between) needs no clock.
            _ => {}
        }
    }

    fn event(&mut self, event: &str, message: &Value) {
        match event {
            "property-change" => {
                let name = message["name"].as_str().unwrap_or_default();
                let data = message.get("data").cloned().unwrap_or(Value::Null);
                self.property(name, data);
            }
            "start-file" => {
                // Each source gets its own report and clock, even ones the same as the last
                // source's.
                self.started = false;
                self.report = None;
                self.clock = None;
                self.picture_failed = false;
            }
            "playback-restart" => {
                if !self.started {
                    self.started = true;
                    self.emit(Event::Started);
                    self.send_report();
                    self.check_picture();
                }
                self.ask_position();
            }
            "end-file" => {
                self.started = false;
                let ending = match message["reason"].as_str() {
                    Some("eof") => Ending::Finished,
                    Some("stop" | "quit") => Ending::Stopped,
                    Some("error") => Ending::Failed(
                        message["file_error"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_owned(),
                    ),
                    // A playlist replaced by its entries, which play next.
                    Some("redirect") => return,
                    other => Ending::Failed(format!("ended for an unknown reason: {other:?}")),
                };
                self.emit(Event::Ended(ending));
            }
            _ => {}
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
                if changed && self.started {
                    self.ask_position();
                }
            }
            "track-list" | "vid" => {
                self.properties.insert(name.to_owned(), data);
                self.check_picture();
            }
            _ => {
                self.properties.insert(name.to_owned(), data);
                if self.started {
                    self.send_report();
                }
            }
        }
    }

    /// Reports, once a source, a picture that does not play: mpv deselects the video track when
    /// its video output fails, and plays the sound alone. Aerith never deselects it itself.
    fn check_picture(&mut self) {
        if !self.started || self.picture_failed {
            return;
        }
        let has_picture = self
            .properties
            .get("track-list")
            .and_then(Value::as_array)
            .is_some_and(|tracks| tracks.iter().any(|track| track["type"] == "video"));
        let selected = self
            .properties
            .get("vid")
            .is_some_and(|vid| *vid != false && *vid != "no");
        if has_picture && !selected {
            self.picture_failed = true;
            self.emit(Event::PictureFailed);
        }
    }

    /// Sends the report if it changed since the last one sent. A report of no track at all is not
    /// sent: mpv clears its properties as a source ends, and fills them as it starts, in any order
    /// against the events that say so.
    fn send_report(&mut self) {
        let report = report(&self.properties);
        if report != Report::default() && self.report.as_ref() != Some(&report) {
            self.report = Some(report.clone());
            self.emit(Event::Report(Box::new(report)));
        }
    }
}

/// The report, from the observed properties' values.
fn report(properties: &HashMap<String, Value>) -> Report {
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

        fn send(&mut self, value: Value) {
            writeln!(self.writer, "{value}").unwrap();
        }

        fn property(&mut self, name: &str, data: Value) {
            self.send(json!({"event": "property-change", "id": 1, "name": name, "data": data}));
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

    fn quiet(events: &Receiver<Event>) {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(100)) {
            panic!("no event expected, got {event:?}");
        }
    }

    /// Brings the fake to a started source, playing.
    fn start(mpv: &mut FakeMpv, events: &Receiver<Event>) {
        mpv.property("pause", json!(false));
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        mpv.property("core-idle", json!(false));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(events), Event::Started);
        let question = mpv.command();
        assert_eq!(question["command"], json!(["get_property", "time-pos"]));
        mpv.answer(&question, "success", json!(0.0));
        assert!(matches!(next(events), Event::Clock(_)));
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
    fn a_file_opens_without_headers_and_its_first_frame_starts_the_clock() {
        let (engine, events, mut mpv) = driven(exited());
        engine
            .send(Command::Open(Source::file("/corpus/clip.mkv").unwrap()))
            .unwrap();
        let headers = mpv.accept();
        assert_eq!(
            headers["command"],
            json!(["set_property", "http-header-fields", []])
        );
        assert_eq!(
            mpv.accept()["command"],
            json!(["loadfile", "/corpus/clip.mkv"])
        );
        mpv.property("pause", json!(false));
        mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
        mpv.property("core-idle", json!(false));
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started);
        let question = mpv.command();
        assert_eq!(question["command"], json!(["get_property", "time-pos"]));
        let before = Instant::now();
        mpv.answer(&question, "success", json!(12.5));
        let Event::Clock(clock) = next(&events) else {
            panic!("a clock")
        };
        assert_eq!(clock.position, Duration::from_millis(12_500));
        assert!(clock.running);
        assert!(clock.at >= before);
    }

    #[test]
    fn a_streams_url_and_headers_go_over_the_connection() {
        let (engine, _events, mut mpv) = driven(exited());
        let headers = vec![
            Header::new("Authorization", "Bearer token, with a comma").unwrap(),
            Header::new("Referer", "https://example.org/").unwrap(),
        ];
        let source = Source::stream("https://example.org/a.mkv?key=1", headers).unwrap();
        engine.send(Command::Open(source)).unwrap();
        assert_eq!(
            mpv.accept()["command"],
            json!([
                "set_property",
                "http-header-fields",
                [
                    "Authorization: Bearer token, with a comma",
                    "Referer: https://example.org/"
                ]
            ])
        );
        assert_eq!(
            mpv.accept()["command"],
            json!(["loadfile", "https://example.org/a.mkv?key=1"])
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
                json!(["seek", 83.25, "absolute"]),
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
        let (_engine, events, mut mpv) = driven(exited());
        start(&mut mpv, &events);
        mpv.property("core-idle", json!(true));
        let question = mpv.command();
        mpv.answer(&question, "success", json!(3.0));
        let Event::Clock(clock) = next(&events) else {
            panic!("a clock")
        };
        assert!(!clock.running);
        assert_eq!(clock.position, Duration::from_secs(3));
        // The same state again asks nothing.
        mpv.property("core-idle", json!(true));
        quiet(&events);
    }

    #[test]
    fn a_stopped_clock_at_the_same_position_is_sent_once() {
        let (_engine, events, mut mpv) = driven(exited());
        start(&mut mpv, &events);
        mpv.property("core-idle", json!(true));
        let question = mpv.command();
        mpv.answer(&question, "success", json!(5.0));
        assert!(matches!(next(&events), Event::Clock(c) if !c.running));
        mpv.send(json!({"event": "playback-restart"}));
        let question = mpv.command();
        mpv.answer(&question, "success", json!(5.0));
        quiet(&events);
    }

    #[test]
    fn one_question_for_the_position_at_a_time() {
        let (_engine, events, mut mpv) = driven(exited());
        start(&mut mpv, &events);
        mpv.property("core-idle", json!(true));
        let question = mpv.command();
        // A seek's restart while the question is on its way asks nothing more.
        mpv.send(json!({"event": "playback-restart"}));
        mpv.property("core-idle", json!(false));
        mpv.answer(&question, "success", json!(4.0));
        let Event::Clock(clock) = next(&events) else {
            panic!("a clock")
        };
        assert!(clock.running, "read with the state when the answer came");
        quiet(&events);
        // The next change asks again.
        mpv.property("core-idle", json!(true));
        assert_eq!(mpv.command()["command"][0], "get_property");
    }

    #[test]
    fn the_report_comes_after_the_start_then_at_each_change() {
        let (_engine, events, mut mpv) = driven(exited());
        mpv.property("video-format", json!("hevc"));
        mpv.property("hwdec-current", json!("vaapi"));
        mpv.property(
            "video-params",
            json!({"w": 3840, "h": 2160, "primaries": "bt.2020", "gamma": "pq",
                   "sig-peak": 4.926}),
        );
        mpv.property("current-vo", json!("gpu-next"));
        mpv.property(
            "video-target-params",
            json!({"primaries": "bt.709", "gamma": "gamma2.2", "sig-peak": 0.0}),
        );
        mpv.property("audio-codec-name", json!("truehd"));
        mpv.property(
            "audio-params",
            json!({"format": "s32", "samplerate": 48000, "channels": "7.1"}),
        );
        mpv.property("current-ao", json!("pipewire"));
        quiet(&events);
        mpv.send(json!({"event": "playback-restart"}));
        assert_eq!(next(&events), Event::Started);
        let Event::Report(report) = next(&events) else {
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

        let question = mpv.command();
        mpv.answer(&question, "success", json!(0.0));
        assert!(matches!(next(&events), Event::Clock(_)));
        mpv.property("hwdec-current", json!("no"));
        let Event::Report(report) = next(&events) else {
            panic!("a report")
        };
        assert_eq!(report.video.unwrap().hardware_decoder, None);
        // The same value again sends nothing.
        mpv.property("hwdec-current", json!("no"));
        quiet(&events);
        // Nor does a report of no track at all, as a source ends.
        for name in ["video-params", "audio-params"] {
            mpv.property(name, Value::Null);
        }
        let Event::Report(report) = next(&events) else {
            panic!("a report")
        };
        assert!(report.video.is_none() && report.audio.is_some());
        quiet(&events);
    }

    #[test]
    fn each_source_gets_its_report_even_when_it_is_the_same_as_the_last() {
        let (_engine, events, mut mpv) = driven(exited());
        mpv.property("audio-codec-name", json!("aac"));
        mpv.property(
            "audio-params",
            json!({"format": "floatp", "samplerate": 48000, "channels": "stereo"}),
        );
        for _ in 0..2 {
            mpv.send(json!({"event": "start-file", "playlist_entry_id": 1}));
            mpv.send(json!({"event": "playback-restart"}));
            assert_eq!(next(&events), Event::Started);
            assert!(matches!(next(&events), Event::Report(_)), "a report");
            let question = mpv.command();
            mpv.answer(&question, "success", json!(0.0));
            assert!(matches!(next(&events), Event::Clock(_)));
            mpv.send(json!({"event": "end-file", "reason": "stop"}));
            assert_eq!(next(&events), Event::Ended(Ending::Stopped));
        }
    }

    #[test]
    fn a_picture_the_output_could_not_show_is_reported_once() {
        let (_engine, events, mut mpv) = driven(exited());
        let tracks = json!([{"id": 1, "type": "video"}, {"id": 1, "type": "audio"}]);
        mpv.property("track-list", tracks.clone());
        mpv.property("vid", json!(1));
        start(&mut mpv, &events);
        // The output fails: mpv drops the picture and plays on.
        mpv.property("vid", json!(false));
        assert_eq!(next(&events), Event::PictureFailed);
        mpv.property("track-list", tracks);
        quiet(&events);
    }

    #[test]
    fn a_source_without_picture_is_no_failure() {
        let (_engine, events, mut mpv) = driven(exited());
        mpv.property("track-list", json!([{"id": 1, "type": "audio"}]));
        mpv.property("vid", json!(false));
        start(&mut mpv, &events);
        quiet(&events);
    }

    #[test]
    fn each_end_maps_to_an_ending() {
        let (_engine, events, mut mpv) = driven(exited());
        for (end, expected) in [
            (json!({"reason": "eof"}), Some(Ending::Finished)),
            (json!({"reason": "stop"}), Some(Ending::Stopped)),
            (
                json!({"reason": "error", "file_error": "unrecognized file format"}),
                Some(Ending::Failed("unrecognized file format".to_owned())),
            ),
            (json!({"reason": "redirect"}), None),
        ] {
            let mut message = end;
            message["event"] = json!("end-file");
            mpv.send(message);
            match expected {
                Some(ending) => assert_eq!(next(&events), Event::Ended(ending)),
                None => quiet(&events),
            }
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
        // The thread has ended: commands now fail, and nothing waits on them.
        thread::sleep(Duration::from_millis(100));
        assert_eq!(engine.send(Command::Play), Err(EngineGone));
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
            "mpv was ended: {reason}"
        );
        flood.join().unwrap();
    }

    #[test]
    fn malformed_json_ends_the_engine_and_bytes_that_are_not_utf8_do_not() {
        let (_engine, events, mut mpv) = driven(exited());
        mpv.writer
            .write_all(b"{\"event\":\"end-file\",\"reason\":\"error\",\"file_error\":\"\xff\"}\n")
            .unwrap();
        assert_eq!(
            next(&events),
            Event::Ended(Ending::Failed("\u{fffd}".to_owned()))
        );
        mpv.writer.write_all(b"{\"event\": nope}\n").unwrap();
        let Event::EngineFailed(reason) = next(&events) else {
            panic!("a failure")
        };
        assert!(reason.contains("malformed"), "{reason}");
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
    fn dropping_the_player_closes_the_connection_and_the_engine_exits() {
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
