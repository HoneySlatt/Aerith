// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The player on every clip of the test corpus (ADR-0012), with the picture and the sound
//! discarded. The corpus is 11 GB that `scripts/corpus.sh` makes, so CI does not hold it, and the
//! test is ignored unless asked for:
//!
//! ```text
//! AERITH_CORPUS=~/.cache/aerith-bench/corpus cargo test -p aerith-player --test corpus -- --ignored --nocapture
//! ```
//!
//! It prints, for each clip, the time to the first frame and what mpv reported. A second test
//! times the first frame through the player against mpv alone.

#![cfg(target_os = "linux")]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use aerith_player::{Ending, Event, Header, OpenId, Player, Report, Source};
use common::{ClipServer, TestDir, wait_for};

fn corpus() -> PathBuf {
    let corpus = PathBuf::from(
        std::env::var_os("AERITH_CORPUS").expect("AERITH_CORPUS names the corpus's folder"),
    );
    assert!(
        corpus.join("sources").is_dir(),
        "{} holds no corpus",
        corpus.display()
    );
    corpus
}

fn clips(corpus: &Path) -> Vec<PathBuf> {
    let mut clips: Vec<PathBuf> = std::fs::read_dir(corpus)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "mkv" || e == "ts"))
        .collect();
    clips.sort();
    assert!(clips.len() >= 20, "the whole corpus: {clips:?}");
    clips
}

/// What mpv reported, in one line.
fn summary(report: &Report) -> String {
    let video = report.video.as_ref().map_or("no picture".to_owned(), |v| {
        format!(
            "{} {}x{} {}/{}{}",
            v.codec,
            v.width,
            v.height,
            v.decoded.primaries,
            v.decoded.transfer,
            v.hardware_decoder
                .as_ref()
                .map_or(String::new(), |d| format!(" on {d}"))
        )
    });
    let audio = report.audio.as_ref().map_or("no sound".to_owned(), |a| {
        format!(
            "{} {} {} Hz",
            a.codec, a.decoded.channels, a.decoded.sample_rate
        )
    });
    format!("{video}; {audio}")
}

/// Keeps `latest` in `report` unless it tells of fewer tracks: mpv clears what it reports as each
/// track ends, and a short clip ends during the test.
fn keep(report: &mut Option<Report>, latest: Report) {
    let tracks = |r: &Report| usize::from(r.video.is_some()) + usize::from(r.audio.is_some());
    if report
        .as_ref()
        .is_none_or(|kept| tracks(&latest) >= tracks(kept))
    {
        *report = Some(latest);
    }
}

/// Waits for the first event `wanted` accepts, as [`wait_for`] does, keeping the reports of `open`
/// that come in between in `report`.
fn wait_keeping_reports(
    events: &Receiver<Event>,
    open: OpenId,
    report: &mut Option<Report>,
    wanted: impl Fn(&Event) -> bool,
) -> Event {
    loop {
        let event = wait_for(events, |e| wanted(e) || matches!(e, Event::Report(..)));
        match event {
            Event::Report(of, latest) if !wanted(&Event::Report(of, latest.clone())) => {
                if of == open {
                    keep(report, *latest);
                }
            }
            event => return event,
        }
    }
}

/// Opens `source` paused, so that a clip shorter than a second still runs every command, then
/// seeks, plays and stops it. Returns the time to the first frame and the fullest report.
fn play(player: &Player, events: &Receiver<Event>, source: &Source) -> (Duration, Report) {
    let mut report = None;
    player.pause().unwrap();
    let opened = Instant::now();
    let open = player.open(source).unwrap();
    let first_frame = match wait_keeping_reports(
        events,
        open,
        &mut report,
        |e| matches!(e, Event::Started(o) | Event::Ended(o, _) if *o == open),
    ) {
        Event::Started(_) => opened.elapsed(),
        other => panic!("{source:?} ended before its first frame: {other:?}"),
    };
    // The clocks of the start come first; the next one is the seek's. Some clips' pictures last
    // two frames: the seek stops on the last one, short of 0.2 s.
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(event) = events.try_recv() {
        if let Event::Report(of, latest) = event
            && of == open
        {
            keep(&mut report, *latest);
        }
    }
    player.seek(Duration::from_millis(200)).unwrap();
    let clock = match wait_keeping_reports(
        events,
        open,
        &mut report,
        |e| matches!(e, Event::Clock(o, _) | Event::Ended(o, _) if *o == open),
    ) {
        Event::Clock(_, clock) => clock,
        // mpv ends a short MPEG-TS clip when it seeks into it (dv7-1080p): reported as it is.
        Event::Ended(_, Ending::Finished) => {
            eprintln!("{source:?}: ended at the seek");
            return (first_frame, report.unwrap_or_default());
        }
        other => panic!("{source:?}: {other:?}"),
    };
    assert!(
        !clock.running && clock.position > Duration::ZERO,
        "{source:?}: {clock:?}"
    );
    assert!(
        clock.position <= Duration::from_millis(250),
        "{source:?}: {clock:?}"
    );
    player.play().unwrap();
    // A clip shorter than its seek's position plus a moment ends as it plays.
    if let Event::Ended(_, ending) = wait_keeping_reports(events, open, &mut report, |e| {
        *e == Event::Paused(false) || matches!(e, Event::Ended(o, _) if *o == open)
    }) {
        assert_eq!(ending, Ending::Finished, "{source:?}");
        return (first_frame, report.unwrap_or_default());
    }
    std::thread::sleep(Duration::from_millis(300));
    player.stop().unwrap();
    // A clip shorter than the wait has ended by itself, and its stop ends nothing more.
    match wait_keeping_reports(
        events,
        open,
        &mut report,
        |e| matches!(e, Event::Ended(o, _) if *o == open),
    ) {
        Event::Ended(_, Ending::Stopped | Ending::Finished) => {}
        other => panic!("{source:?}: {other:?}"),
    }
    (first_frame, report.unwrap_or_default())
}

#[test]
#[ignore = "needs the corpus: AERITH_CORPUS=<folder> cargo test -p aerith-player --test corpus -- --ignored"]
fn every_clip_of_the_corpus_opens_seeks_plays_and_stops() {
    let corpus = corpus();
    let dir = TestDir::new("corpus");
    let (player, events) = Player::start(&dir.settings(&[&corpus])).unwrap();
    for clip in clips(&corpus) {
        let source = Source::file(&clip).unwrap();
        let (first_frame, report) = play(&player, &events, &source);
        let name = clip.file_name().unwrap().to_string_lossy();
        eprintln!(
            "{name:40} {:6.0} ms  {}",
            first_frame.as_secs_f64() * 1000.0,
            summary(&report)
        );
        assert!(report.video.is_some(), "{name}: every clip has a picture");
    }

    // The bench's clip, as a stream with a header.
    let clip = corpus.join("hdr10-2160p-hevc10-eac3.mkv");
    let server = ClipServer::start(clip);
    let header = Header::new("Authorization", "Bearer corpus").unwrap();
    let source = Source::stream(server.url(""), vec![header]).unwrap();
    let (first_frame, report) = play(&player, &events, &source);
    eprintln!(
        "{:40} {:6.0} ms  {}",
        "hdr10-2160p-hevc10-eac3.mkv, streamed",
        first_frame.as_secs_f64() * 1000.0,
        summary(&report)
    );
    assert!(
        server
            .heads()
            .iter()
            .all(|h| h.contains("Authorization: Bearer corpus"))
    );
    drop(player);
    dir.assert_home_untouched(&[&corpus]);
}

/// The options `src/mpv.rs` gives mpv, but its IPC and its caches, written again here: the
/// player keeps them private. Keep the two lists in step.
const PLAYER_OPTIONS: [&str; 19] = [
    "--resume-playback=no",
    "--save-position-on-quit=no",
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
    "--title=Aerith",
];

/// mpv alone, driven as the bench drives it: `--no-config`, the same discarded outputs, its IPC
/// on a socket file, timed from `loadfile` to `playback-restart` over one connection.
struct MpvAlone {
    child: Child,
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl MpvAlone {
    /// Starts mpv in `dir`'s sandbox, with `options` beyond the bench's.
    fn start(dir: &TestDir, corpus: &Path, options: &[&str]) -> Self {
        let socket = dir.root.join("alone.sock");
        let child = Command::new(dir.mpv(&[corpus]))
            .args([
                "--no-config",
                "--idle=yes",
                "--terminal=no",
                "--vo=null",
                "--ao=null",
            ])
            .args(options)
            .arg(format!("--input-ipc-server={}", socket.display()))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + common::WAIT;
        let stream = loop {
            match UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(error) => {
                    assert!(Instant::now() < deadline, "mpv's socket: {error}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        Self {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
            child,
        }
    }

    /// Sends `command`, then reads mpv's messages until one names `event`.
    fn until(&mut self, command: &str, event: &str) {
        writeln!(self.writer, "{command}").unwrap();
        let marker = format!("\"event\":\"{event}\"");
        let mut line = String::new();
        loop {
            line.clear();
            assert!(self.reader.read_line(&mut line).unwrap() > 0, "mpv ended");
            if line.contains(&marker) {
                return;
            }
        }
    }

    /// The time from `loadfile` to the first frame of `target`; then stops it.
    fn first_frame(&mut self, target: &str) -> Duration {
        let opened = Instant::now();
        let load = serde_json::json!({"command": ["loadfile", target]}).to_string();
        self.until(&load, "playback-restart");
        let time = opened.elapsed();
        self.until(r#"{"command": ["stop"]}"#, "end-file");
        time
    }
}

impl Drop for MpvAlone {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The time from `open` to the first frame, against mpv alone driven the same way (ADR-0013),
/// with its own defaults as the bench runs it, and with the player's options: their difference is
/// mpv's, the rest the player's. Each is started and idle beforehand; the bench's clip comes from
/// a file and from HTTP on 127.0.0.1, the picture and the sound discarded. A first run of each is
/// left out, as it warms the caches; then the three alternate, run by run. Run alone
/// (`--test-threads=1`) on an idle machine.
#[test]
#[ignore = "a measure on the corpus: AERITH_CORPUS=<folder> cargo test --release -p aerith-player --test corpus -- --ignored --test-threads=1 --nocapture"]
fn time_to_the_first_frame() {
    const RUNS: usize = 7;
    let corpus = corpus();
    let clip = corpus.join("hdr10-2160p-hevc10-eac3.mkv");
    let server = ClipServer::start(clip.clone());
    let dir = TestDir::new("first-frame");
    // Each mpv alone has a home of its own: with its defaults, its scripts write a font cache.
    let (defaults_dir, options_dir) = (TestDir::new("alone"), TestDir::new("alone-options"));
    let (player, events) = Player::start(&dir.settings(&[&corpus])).unwrap();
    let mut defaults = MpvAlone::start(&defaults_dir, &corpus, &[]);
    let mut with_options = MpvAlone::start(&options_dir, &corpus, &PLAYER_OPTIONS);
    std::thread::sleep(Duration::from_secs(1));
    let through_player = |source: &Source| -> Duration {
        let opened = Instant::now();
        let open = player.open(source).unwrap();
        wait_for(&events, |e| *e == Event::Started(open));
        let time = opened.elapsed();
        player.stop().unwrap();
        wait_for(&events, |e| matches!(e, Event::Ended(o, _) if *o == open));
        time
    };
    let summary = |times: &[Duration]| -> String {
        let mut sorted: Vec<f64> = times.iter().map(|t| t.as_secs_f64() * 1000.0).collect();
        let shown: Vec<String> = sorted.iter().map(|t| format!("{t:.0}")).collect();
        sorted.sort_by(f64::total_cmp);
        format!(
            "{} ms, median {:.0} ms",
            shown.join(", "),
            sorted[sorted.len() / 2]
        )
    };
    for (from, target) in [
        ("file", clip.display().to_string()),
        ("stream", server.url("")),
    ] {
        let source = if from == "file" {
            Source::file(&clip).unwrap()
        } else {
            Source::stream(target.clone(), vec![]).unwrap()
        };
        through_player(&source);
        defaults.first_frame(&target);
        with_options.first_frame(&target);
        let mut times: [Vec<Duration>; 3] = Default::default();
        for _ in 0..RUNS {
            times[0].push(through_player(&source));
            times[1].push(defaults.first_frame(&target));
            times[2].push(with_options.first_frame(&target));
        }
        eprintln!("{from}, through the player: {}", summary(&times[0]));
        eprintln!("{from}, mpv alone, its defaults: {}", summary(&times[1]));
        eprintln!(
            "{from}, mpv alone, the player's options: {}",
            summary(&times[2])
        );
    }
    drop((defaults, with_options, player));
    dir.assert_home_untouched(&[&corpus]);
    options_dir.assert_home_untouched(&[&corpus]);
}
