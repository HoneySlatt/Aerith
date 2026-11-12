// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How the bench talks to the engine inside each rival, once the rival runs ([`crate::rivals`]).
//!
//! - mpv, alone or embedded as libmpv in Stremio and Jellyfin Desktop, over its JSON IPC: the bench
//!   loads the clip, times the first frame by mpv's `playback-restart` event, and reads mpv's own
//!   counters of dropped, delayed and mistimed frames and its audio-video difference.
//! - Kodi's own player, over JSON-RPC on TCP: the first frame is Kodi's `Player.OnAVStart`
//!   notification; Kodi publishes no frame counter and no sync figure there, so those come from its
//!   log or stay unmeasured ([`Engine::counters`]).

use std::io;
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::json_stream::{JsonStream, Protocol, Timed};

/// How long one request may wait for its answer.
const ANSWER: Duration = Duration::from_secs(5);
/// How long a clip may take to show its first frame before the run fails.
const FIRST_FRAME: Duration = Duration::from_secs(60);

/// mpv's counters and figures that the bench reads every second, by property name.
pub const MPV_COUNTERS: [&str; 7] = [
    "frame-drop-count",
    "decoder-frame-drop-count",
    "vo-delayed-frame-count",
    "mistimed-frame-count",
    "avsync",
    "time-pos",
    "estimated-display-fps",
];

/// What mpv reports once the clip plays: the engine's own view of what it decodes and outputs.
const MPV_REPORT: [&str; 17] = [
    "mpv-version",
    "ffmpeg-version",
    "current-vo",
    "current-gpu-context",
    "hwdec-current",
    "video-params",
    "video-out-params",
    "video-target-params",
    "display-names",
    "display-fps",
    "display-sync-active",
    "video-sync",
    "current-ao",
    "audio-codec-name",
    "audio-params",
    "audio-out-params",
    "audio-device",
];

/// Kodi's own labels for what it decodes and outputs, read through `XBMC.GetInfoLabels`.
const KODI_REPORT: [&str; 9] = [
    "System.BuildVersion",
    "Player.Process(videodecoder)",
    "Player.Process(videohwdecoder)",
    "Player.Process(pixformat)",
    "Player.Process(videofps)",
    "VideoPlayer.HdrType",
    "Player.Process(audiodecoder)",
    "Player.Process(audiochannels)",
    "System.ScreenResolution",
];

/// The engine of a running rival.
pub enum Engine {
    /// mpv's IPC, on its Unix socket.
    Mpv(JsonStream<UnixStream>),
    /// Kodi's JSON-RPC on TCP, and its log, which holds the frames it dropped.
    Kodi {
        rpc: JsonStream<TcpStream>,
        log: PathBuf,
    },
}

/// One second's reading of an engine.
#[derive(Clone, Debug, Default)]
pub struct Counters {
    /// Each counter by name, `None` where the engine has no such figure now.
    pub values: Vec<(String, Option<f64>)>,
    /// The engine reached the end of the clip, or stopped.
    pub ended: bool,
}

impl Engine {
    /// Connects to mpv's IPC socket, waiting up to `timeout` for the player to open it.
    pub fn connect_mpv(socket: &Path, timeout: Duration) -> io::Result<Self> {
        let deadline = Instant::now() + timeout;
        loop {
            match UnixStream::connect(socket) {
                Ok(stream) => {
                    let reader = stream.try_clone()?;
                    return Ok(Engine::Mpv(JsonStream::new(stream, reader, Protocol::Mpv)));
                }
                Err(error) if Instant::now() >= deadline => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("mpv's IPC at {} did not open: {error}", socket.display()),
                    ));
                }
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    /// Connects to Kodi's JSON-RPC on `127.0.0.1:port`, waiting up to `timeout` for Kodi to start
    /// and answer a ping.
    pub fn connect_kodi(port: u16, log: PathBuf, timeout: Duration) -> io::Result<Self> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
                let reader = stream.try_clone()?;
                let mut rpc = JsonStream::new(stream, reader, Protocol::JsonRpc);
                if rpc
                    .request(json!({"jsonrpc": "2.0", "method": "JSONRPC.Ping"}), ANSWER)
                    .is_ok_and(|answer| answer["result"] == "pong")
                {
                    return Ok(Engine::Kodi { rpc, log });
                }
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Kodi's JSON-RPC on port {port} did not answer"),
                ));
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    /// Asks the engine to play `clip`, a path or a URL, and returns the time until its first frame
    /// was shown, by the engine's own marker.
    pub fn load(&mut self, clip: &str) -> io::Result<Duration> {
        let started = Instant::now();
        let deadline = started + FIRST_FRAME;
        match self {
            Engine::Mpv(ipc) => {
                checked(ipc.request(json!({"command": ["loadfile", clip]}), ANSWER)?)?;
                wait_for(ipc, "playback-restart", deadline)?;
                // mpv plays the sound alone when its video output fails: that is no run.
                let video = ipc.request(
                    json!({"command": ["get_property", "vo-configured"]}),
                    ANSWER,
                )?;
                if video["data"] != true {
                    return Err(io::Error::other(
                        "mpv shows no video: its video output failed (see mpv.log)",
                    ));
                }
            }
            Engine::Kodi { rpc, .. } => {
                let answer = rpc.request(
                    json!({"jsonrpc": "2.0", "method": "Player.Open",
                           "params": {"item": {"file": clip}}}),
                    ANSWER,
                )?;
                if answer.get("error").is_some() {
                    return Err(io::Error::other(format!("Kodi refused the clip: {answer}")));
                }
                wait_for(rpc, "Player.OnAVStart", deadline)?;
            }
        }
        Ok(started.elapsed())
    }

    /// Waits for the clip that the player opened by itself (Aerith's testbed opens its own) to
    /// show its first frame, and returns the time from mpv's `start-file` event to its
    /// `playback-restart`. mpv opens the bench's socket at its start, before the player opens the
    /// clip, so the bench sees both events.
    pub fn wait_started(&mut self) -> io::Result<Duration> {
        let Engine::Mpv(ipc) = self else {
            return Err(io::Error::other("only an mpv engine is observed"));
        };
        let deadline = Instant::now() + FIRST_FRAME;
        wait_for(ipc, "start-file", deadline)?;
        let started = Instant::now();
        wait_for(ipc, "playback-restart", deadline)?;
        let elapsed = started.elapsed();
        let video = ipc.request(
            json!({"command": ["get_property", "vo-configured"]}),
            ANSWER,
        )?;
        if video["data"] != true {
            return Err(io::Error::other(
                "mpv shows no video: its video output failed (see mpv.log)",
            ));
        }
        Ok(elapsed)
    }

    /// Shows the frame at `at` seconds, paused, and has mpv write it to `path` as it draws it in
    /// its window (`screenshot-to-file ... window`): scaled to the window, as on screen.
    pub fn capture(&mut self, at: f64, path: &Path) -> io::Result<()> {
        let Engine::Mpv(ipc) = self else {
            return Err(io::Error::other("only mpv can capture its frames"));
        };
        checked(ipc.request(json!({"command": ["set_property", "pause", true]}), ANSWER)?)?;
        drain(ipc, "playback-restart")?;
        checked(ipc.request(json!({"command": ["seek", at, "absolute+exact"]}), ANSWER)?)?;
        wait_for(ipc, "playback-restart", Instant::now() + FIRST_FRAME)?;
        checked(ipc.request(
            json!({"command": ["screenshot-to-file", path.display().to_string(), "window"]}),
            FIRST_FRAME,
        )?)?;
        Ok(())
    }

    /// Reads the engine's counters. The events that came since the last reading are read too, to
    /// see whether the clip ended.
    pub fn counters(&mut self) -> io::Result<Counters> {
        match self {
            Engine::Mpv(ipc) => {
                let mut counters = Counters::default();
                for name in MPV_COUNTERS {
                    let answer = ipc.request(json!({"command": ["get_property", name]}), ANSWER)?;
                    let value = (answer["error"] == "success").then(|| answer["data"].as_f64());
                    counters.values.push((name.to_owned(), value.flatten()));
                }
                let idle =
                    ipc.request(json!({"command": ["get_property", "idle-active"]}), ANSWER)?;
                counters.ended = idle["data"] == true || drain(ipc, "end-file")?;
                Ok(counters)
            }
            Engine::Kodi { rpc, .. } => {
                // Kodi numbers its players by kind; a stream may play in another than a file.
                let players = rpc.request(
                    json!({"jsonrpc": "2.0", "method": "Player.GetActivePlayers"}),
                    ANSWER,
                )?;
                let Some(player) = players["result"].as_array().and_then(|active| {
                    active.iter().find_map(|player| player["playerid"].as_u64())
                }) else {
                    return Ok(Counters {
                        values: vec![("time-pos".into(), None)],
                        ended: true,
                    });
                };
                let answer = rpc.request(
                    json!({"jsonrpc": "2.0", "method": "Player.GetProperties",
                           "params": {"playerid": player, "properties": ["time", "speed"]}}),
                    ANSWER,
                )?;
                let time = &answer["result"]["time"];
                let seconds = [
                    ("hours", 3600.0),
                    ("minutes", 60.0),
                    ("seconds", 1.0),
                    ("milliseconds", 0.001),
                ]
                .iter()
                .map(|(unit, scale)| time[unit].as_f64().map(|v| v * scale))
                .sum::<Option<f64>>();
                let ended = answer.get("error").is_some() || drain(rpc, "Player.OnStop")?;
                Ok(Counters {
                    values: vec![("time-pos".into(), seconds)],
                    ended,
                })
            }
        }
    }

    /// What the engine says it decodes and outputs, as it reports it.
    pub fn report(&mut self) -> io::Result<Value> {
        match self {
            Engine::Mpv(ipc) => {
                let mut report = serde_json::Map::new();
                for name in MPV_REPORT {
                    let answer = ipc.request(json!({"command": ["get_property", name]}), ANSWER)?;
                    let value = if answer["error"] == "success" {
                        answer["data"].clone()
                    } else {
                        Value::Null
                    };
                    report.insert(name.to_owned(), value);
                }
                Ok(Value::Object(report))
            }
            Engine::Kodi { rpc, .. } => {
                let answer = rpc.request(
                    json!({"jsonrpc": "2.0", "method": "XBMC.GetInfoLabels",
                           "params": {"labels": KODI_REPORT}}),
                    ANSWER,
                )?;
                Ok(answer["result"].clone())
            }
        }
    }

    /// The frames Kodi's log says it dropped at output. A lower bound: Kodi logs only these;
    /// the frames its decoder drops, and those its renderer skips, reach only its on-screen
    /// overlay. `None` for mpv, whose counters are exact.
    pub fn logged_drops(&self) -> Option<u64> {
        match self {
            Engine::Mpv(_) => None,
            Engine::Kodi { log, .. } => std::fs::read_to_string(log)
                .ok()
                .map(|text| text.matches("- dropped in output").count() as u64),
        }
    }

    /// The audio-video differences, in seconds, that Kodi's audio engine logs when it measures
    /// them (`ActiveAE::SyncStream - average error ...`, in milliseconds): only when it corrects
    /// or checks the sync, not at a fixed rate. `None` for mpv, read over its IPC instead.
    pub fn logged_sync(&self) -> Option<Vec<f64>> {
        match self {
            Engine::Mpv(_) => None,
            Engine::Kodi { log, .. } => std::fs::read_to_string(log)
                .ok()
                .map(|text| kodi_sync(&text)),
        }
    }

    /// Asks the player to quit; the caller stops what does not.
    pub fn quit(&mut self) {
        let _ = match self {
            Engine::Mpv(ipc) => ipc.request(json!({"command": ["quit"]}), Duration::from_secs(2)),
            Engine::Kodi { rpc, .. } => rpc.request(
                json!({"jsonrpc": "2.0", "method": "Application.Quit"}),
                Duration::from_secs(2),
            ),
        };
    }
}

/// The differences in Kodi's `average error` lines, converted to seconds, from two seconds after
/// its audio and video started (`OnAVStarted`), as mpv's readings are: the first seconds settle
/// the clock after the start.
fn kodi_sync(log: &str) -> Vec<f64> {
    // A line starts with its date and time: `2026-11-11 03:42:43.093 T:...`.
    let seconds_of_day = |line: &str| -> Option<f64> {
        let time = line.split_whitespace().nth(1)?;
        let mut parts = time.split(':');
        let (hours, minutes, seconds) = (parts.next()?, parts.next()?, parts.next()?);
        Some(
            hours.parse::<f64>().ok()? * 3600.0
                + minutes.parse::<f64>().ok()? * 60.0
                + seconds.parse::<f64>().ok()?,
        )
    };
    // Seconds since the log's first day, counting the days a run crosses at midnight.
    let mut day = 0.0;
    let mut previous: Option<f64> = None;
    let timed: Vec<(f64, &str)> = log
        .lines()
        .filter_map(|line| {
            let at = seconds_of_day(line)?;
            if previous.is_some_and(|before| at + 43_200.0 < before) {
                day += 86_400.0;
            }
            previous = Some(at);
            Some((day + at, line))
        })
        .collect();
    let Some(started) = timed
        .iter()
        .find(|(_, line)| line.contains("OnAVStarted"))
        .map(|(at, _)| *at)
    else {
        return Vec::new();
    };
    timed
        .iter()
        .filter(|(at, _)| *at >= started + 2.0)
        .filter_map(|(_, line)| {
            line.split_once("ActiveAE::SyncStream - average error")
                .map(|(_, rest)| rest)
        })
        .filter_map(|rest| {
            let number = rest.trim_start().trim_start_matches("of").trim_start();
            let end = number
                .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
                .unwrap_or(number.len());
            number[..end].parse::<f64>().ok()
        })
        .map(|milliseconds| milliseconds / 1000.0)
        .collect()
}

/// An mpv answer that is not a success is an error.
fn checked(answer: Value) -> io::Result<Value> {
    if answer["error"] == "success" {
        Ok(answer)
    } else {
        Err(io::Error::other(format!("mpv refused: {answer}")))
    }
}

/// Reads the events and notifications already received; true if `name` was among them.
fn drain<S: Timed>(stream: &mut JsonStream<S>, name: &str) -> io::Result<bool> {
    let mut seen = false;
    while let Some(value) = stream.next_unsolicited(Instant::now() + Duration::from_millis(1))? {
        seen |= is_named(&value, name);
    }
    Ok(seen)
}

/// An mpv event (`"event"`) or a JSON-RPC notification (`"method"`) by that name.
fn is_named(value: &Value, name: &str) -> bool {
    value["event"] == name || value["method"] == name
}

/// Waits for the event or notification `name`; an mpv `end-file` with an error ends the wait.
fn wait_for<S: Timed>(stream: &mut JsonStream<S>, name: &str, deadline: Instant) -> io::Result<()> {
    loop {
        match stream.next_unsolicited(deadline)? {
            Some(value) if is_named(&value, name) => return Ok(()),
            Some(value) if is_named(&value, "Player.OnStop") => {
                return Err(io::Error::other(
                    "the clip ended before the player reported its first frame",
                ));
            }
            Some(value) if is_named(&value, "end-file") && value["reason"] == "error" => {
                return Err(io::Error::other(format!(
                    "the player failed to open the clip: {value}"
                )));
            }
            Some(_) => {}
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no {name} within {} s", FIRST_FRAME.as_secs()),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// A fake mpv: answers every request with `data`, and sends `playback-restart` after the
    /// `loadfile`.
    fn fake_mpv(socket: PathBuf) -> thread::JoinHandle<Vec<Value>> {
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut seen = Vec::new();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let request: Value = serde_json::from_str(&line).unwrap();
                let id = request["request_id"].clone();
                let command = request["command"].clone();
                seen.push(command.clone());
                let data = match command[1].as_str() {
                    Some("frame-drop-count") => json!(2),
                    Some("idle-active") => json!(false),
                    Some("vo-configured") => json!(true),
                    Some("avsync") => json!(-0.003),
                    _ => json!(null),
                };
                let error = if command[1] == "estimated-display-fps" {
                    "property unavailable"
                } else {
                    "success"
                };
                writeln!(
                    writer,
                    "{}",
                    json!({"request_id": id, "error": error, "data": data})
                )
                .unwrap();
                if command[0] == "loadfile" {
                    writeln!(writer, "{}", json!({"event": "file-loaded"})).unwrap();
                    writeln!(writer, "{}", json!({"event": "playback-restart"})).unwrap();
                }
                if command[0] == "quit" {
                    break;
                }
            }
            seen
        })
    }

    #[test]
    fn mpv_is_loaded_timed_and_read() {
        let directory =
            std::env::temp_dir().join(format!("aerith-bench-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("mpv.sock");
        let _ = std::fs::remove_file(&socket);
        let player = fake_mpv(socket.clone());
        let mut engine = Engine::connect_mpv(&socket, Duration::from_secs(5)).unwrap();
        let first_frame = engine.load("/corpus/clip.mkv").unwrap();
        assert!(first_frame < Duration::from_secs(5));
        let counters = engine.counters().unwrap();
        assert!(!counters.ended);
        let value = |name: &str| counters.values.iter().find(|(n, _)| n == name).unwrap().1;
        assert_eq!(value("frame-drop-count"), Some(2.0));
        assert_eq!(value("avsync"), Some(-0.003));
        assert_eq!(
            value("estimated-display-fps"),
            None,
            "an unavailable property reads as None"
        );
        assert_eq!(engine.logged_drops(), None);
        engine.quit();
        let seen = player.join().unwrap();
        assert_eq!(seen[0], json!(["loadfile", "/corpus/clip.mkv"]));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn kodi_sync_is_read_from_its_log_in_seconds_after_the_start() {
        let log = "2026-11-11 03:42:42.956 T:1 debug <general>: OnAVStarted: CApplication::OnAVStarted
2026-11-11 03:42:43.093 T:2 debug <general>: ActiveAE::SyncStream - average error of -100.731369, start adjusting
2026-11-11 03:42:45.093 T:2 debug <general>: ActiveAE::SyncStream - average error of 31.5, start adjusting
2026-11-11 03:42:46.000 T:2 debug <general>: ActiveAE::SyncStream - average error -0.731369 below threshold of 30.000000
2026-11-11 03:42:47.000 T:2 debug <general>: something else";
        assert_eq!(kodi_sync(log), [0.0315, -0.000731369]);
        assert!(kodi_sync("").is_empty());
        let across_midnight = "2026-11-11 23:59:59.000 T:1 debug <general>: OnAVStarted: x
2026-11-12 00:00:05.000 T:2 debug <general>: ActiveAE::SyncStream - average error of 12.0, start adjusting";
        assert_eq!(kodi_sync(across_midnight), [0.012]);
    }

    #[test]
    fn an_mpv_that_never_opens_its_socket_is_an_error() {
        let socket = std::env::temp_dir().join("aerith-bench-no-such-socket");
        assert!(Engine::connect_mpv(&socket, Duration::from_millis(200)).is_err());
    }

    /// A fake Kodi on TCP: answers the bench's JSON-RPC as Kodi does, with `notifications` sent
    /// after `Player.Open`.
    fn fake_kodi(notifications: Vec<Value>) -> (u16, thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "JSONRPC.Ping" => json!("pong"),
                    "Player.Open" => json!("OK"),
                    "Player.GetActivePlayers" => json!([{"playerid": 1, "type": "video"}]),
                    "Player.GetProperties" => {
                        json!({"time": {"hours": 0, "minutes": 1, "seconds": 2, "milliseconds": 500}})
                    }
                    "XBMC.GetInfoLabels" => json!({"VideoPlayer.HdrType": "hdr10"}),
                    _ => json!(null),
                };
                // Kodi ends its values with no new line.
                write!(
                    writer,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": request["id"], "result": result})
                )
                .unwrap();
                if request["method"] == "Player.Open" {
                    for notification in &notifications {
                        write!(writer, "{notification}").unwrap();
                    }
                }
            }
        });
        (port, server)
    }

    #[test]
    fn kodi_is_loaded_timed_and_read() {
        let (port, server) = fake_kodi(vec![
            json!({"jsonrpc": "2.0", "method": "Player.OnAVStart"}),
        ]);
        let log = std::env::temp_dir().join("aerith-bench-no-kodi-log");
        let mut engine = Engine::connect_kodi(port, log, Duration::from_secs(5)).unwrap();
        assert!(engine.load("/corpus/clip.mkv").unwrap() < Duration::from_secs(5));
        let counters = engine.counters().unwrap();
        assert_eq!(counters.values, [("time-pos".to_owned(), Some(62.5))]);
        assert!(!counters.ended);
        assert_eq!(
            engine.report().unwrap(),
            json!({"VideoPlayer.HdrType": "hdr10"})
        );
        assert_eq!(engine.logged_drops(), None, "no log, no figure");
        drop(engine);
        server.join().unwrap();
    }

    #[test]
    fn a_kodi_clip_that_ends_before_its_first_frame_is_an_error() {
        let (port, server) = fake_kodi(vec![json!({"jsonrpc": "2.0", "method": "Player.OnStop"})]);
        let log = std::env::temp_dir().join("aerith-bench-no-kodi-log");
        let mut engine = Engine::connect_kodi(port, log, Duration::from_secs(5)).unwrap();
        let error = engine.load("/corpus/short.mkv").unwrap_err();
        assert!(error.to_string().contains("ended before"), "{error}");
        drop(engine);
        server.join().unwrap();
    }

    #[test]
    fn a_kodi_that_never_answers_is_an_error() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let log = std::env::temp_dir().join("aerith-bench-no-kodi-log");
        assert!(Engine::connect_kodi(port, log, Duration::from_millis(300)).is_err());
    }
}
