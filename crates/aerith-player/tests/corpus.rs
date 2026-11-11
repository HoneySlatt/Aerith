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
//! It prints, for each clip, the time to the first frame and what mpv reported.

#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use aerith_player::{Ending, Event, Header, Player, Report, Source};
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

/// Waits for the first event `wanted` accepts, as [`wait_for`] does, keeping the reports that
/// come in between in `report`.
fn wait_keeping_reports(
    events: &Receiver<Event>,
    report: &mut Option<Report>,
    wanted: impl Fn(&Event) -> bool,
) -> Event {
    loop {
        match wait_for(events, |e| wanted(e) || matches!(e, Event::Report(_))) {
            Event::Report(latest) if !wanted(&Event::Report(latest.clone())) => {
                keep(report, *latest);
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
    player.open(source).unwrap();
    let first_frame = match wait_keeping_reports(events, &mut report, |e| {
        matches!(e, Event::Started | Event::Ended(_))
    }) {
        Event::Started => opened.elapsed(),
        other => panic!("{source:?} ended before its first frame: {other:?}"),
    };
    // The clocks of the start come first; the next one is the seek's. Some clips' pictures last
    // two frames: the seek stops on the last one, short of 0.2 s.
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(event) = events.try_recv() {
        if let Event::Report(latest) = event {
            keep(&mut report, *latest);
        }
    }
    player.seek(Duration::from_millis(200)).unwrap();
    let clock = match wait_keeping_reports(events, &mut report, |e| {
        matches!(e, Event::Clock(_) | Event::Ended(_))
    }) {
        Event::Clock(clock) => clock,
        // mpv ends a short MPEG-TS clip when it seeks into it (dv7-1080p): reported as it is.
        Event::Ended(Ending::Finished) => {
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
    wait_keeping_reports(events, &mut report, |e| *e == Event::Paused(false));
    std::thread::sleep(Duration::from_millis(300));
    while let Ok(event) = events.try_recv() {
        match event {
            Event::Report(latest) => keep(&mut report, *latest),
            Event::Ended(_) => break,
            _ => {}
        }
    }
    player.stop().unwrap();
    // A clip shorter than the wait has ended by itself, and its stop ends nothing.
    let deadline = Instant::now() + Duration::from_secs(2);
    while let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        match event {
            Event::Ended(Ending::Stopped | Ending::Finished) => break,
            Event::Ended(failed) => panic!("{source:?}: {failed:?}"),
            Event::EngineFailed(reason) => panic!("the engine failed: {reason}"),
            _ => {}
        }
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

/// The time from `open` to the first frame, as the bench times mpv alone (ADR-0013): the player
/// started and idle beforehand, the bench's clip from a file and from HTTP on 127.0.0.1, the
/// picture and the sound discarded. Run alone (`--test-threads=1`) on an idle machine.
#[test]
#[ignore = "a measure on the corpus: AERITH_CORPUS=<folder> cargo test --release -p aerith-player --test corpus -- --ignored --test-threads=1 --nocapture"]
fn time_to_the_first_frame() {
    const RUNS: usize = 5;
    let corpus = corpus();
    let clip = corpus.join("hdr10-2160p-hevc10-eac3.mkv");
    let server = ClipServer::start(clip.clone());
    let dir = TestDir::new("first-frame");
    let (player, events) = Player::start(&dir.settings(&[&corpus])).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    for (from, source) in [
        ("file", Source::file(&clip).unwrap()),
        ("stream", Source::stream(server.url(""), vec![]).unwrap()),
    ] {
        let mut times: Vec<f64> = (0..RUNS)
            .map(|_| {
                let opened = Instant::now();
                player.open(&source).unwrap();
                wait_for(&events, |e| *e == Event::Started);
                let time = opened.elapsed().as_secs_f64() * 1000.0;
                player.stop().unwrap();
                wait_for(&events, |e| matches!(e, Event::Ended(_)));
                time
            })
            .collect();
        let shown: Vec<String> = times.iter().map(|t| format!("{t:.0}")).collect();
        times.sort_by(f64::total_cmp);
        eprintln!(
            "{from}: {} ms, median {:.0} ms",
            shown.join(", "),
            times[RUNS / 2]
        );
    }
    drop(player);
    dir.assert_home_untouched(&[&corpus]);
}
