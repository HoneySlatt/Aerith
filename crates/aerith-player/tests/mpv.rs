// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The player against the real mpv, on clips mpv makes itself, with the picture and the sound
//! discarded: what CI runs on every change. `tests/corpus.rs` plays the test corpus the same way.
//!
//! The clips are FFV1 and FLAC at 24 frames a second: a frame lasts 42 ms.

#![cfg(target_os = "linux")]

mod common;

use std::time::{Duration, Instant};

use aerith_player::{Ending, EngineGone, Event, Header, OpenId, Player, Report, Source};
use common::{ClipServer, TestDir, wait_for};

/// Whether `position` is within a frame and a half of `expected`.
fn near(position: Duration, expected: Duration) -> bool {
    position.abs_diff(expected) <= Duration::from_millis(63)
}

/// Whether a running clock read at `position` started from `expected`. mpv answers once it has
/// played a few frames, without sound to wait for: two in these tests, more on a loaded machine.
fn just_after(position: Duration, expected: Duration) -> bool {
    position >= expected && position - expected <= Duration::from_millis(500)
}

/// Whether `report` tells of both tracks and both outputs, as it does once mpv has said all.
fn complete(report: &Report) -> bool {
    report.video.as_ref().is_some_and(|v| v.output != "none")
        && report.audio.as_ref().is_some_and(|a| a.sent.is_some())
}

/// Waits for the report of `open` that tells of both tracks, and returns it.
fn complete_report(events: &std::sync::mpsc::Receiver<Event>, open: OpenId) -> Report {
    let Event::Report(_, report) = wait_for(
        events,
        |e| matches!(e, Event::Report(o, r) if *o == open && complete(r)),
    ) else {
        unreachable!()
    };
    *report
}

#[test]
fn a_file_plays_pauses_seeks_and_stops() {
    let dir = TestDir::new("file");
    let clip = dir.clip(4);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));

    let report = complete_report(&events, open);
    let video = report.video.as_ref().expect("a picture");
    assert_eq!(video.codec, "ffv1");
    assert_eq!((video.width, video.height), (320, 240));
    assert_eq!(video.hardware_decoder, None);
    assert_eq!(video.output, "null");
    let audio = report.audio.as_ref().expect("a sound");
    assert_eq!(audio.codec, "flac");
    assert_eq!(audio.output, "null");

    // Clocks come as mpv's state changes, in an order that depends on the machine's load: each
    // wait below is for the clock a state must bring, not for the next clock.
    player.pause().unwrap();
    wait_for(&events, |e| *e == Event::Paused(true));

    player.seek(Duration::from_secs(2)).unwrap();
    wait_for(&events, |e| {
        matches!(e, Event::Clock(o, c)
            if *o == open && !c.running && near(c.position, Duration::from_secs(2)))
    });

    player.play().unwrap();
    wait_for(&events, |e| *e == Event::Paused(false));
    let Event::Clock(_, clock) = wait_for(
        &events,
        |e| matches!(e, Event::Clock(o, c) if *o == open && c.running),
    ) else {
        unreachable!()
    };
    assert!(
        just_after(clock.position, Duration::from_secs(2)),
        "{clock:?}"
    );

    player.stop().unwrap();
    wait_for(&events, |e| *e == Event::Ended(open, Ending::Stopped));
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_file_plays_to_its_end_in_real_time() {
    let dir = TestDir::new("end");
    let clip = dir.clip(1);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    let started = Instant::now();
    wait_for(&events, |e| *e == Event::Ended(open, Ending::Finished));
    assert!(
        started.elapsed() > Duration::from_millis(800),
        "it played, not raced"
    );
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_seek_past_the_end_ends_a_playing_source_and_keeps_a_paused_one_on_its_last_frame() {
    let dir = TestDir::new("past-end");
    let clip = dir.clip(3);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let playing = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(playing));
    player.seek(Duration::from_secs(10)).unwrap();
    wait_for(&events, |e| *e == Event::Ended(playing, Ending::Finished));

    player.pause().unwrap();
    let paused = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(paused));
    player.seek(Duration::from_secs(10)).unwrap();
    let Event::Clock(_, clock) = wait_for(
        &events,
        |e| matches!(e, Event::Clock(o, c) if *o == paused && c.position > Duration::from_secs(2)),
    ) else {
        unreachable!()
    };
    assert!(!clock.running, "{clock:?}");
    assert!(
        clock.position <= Duration::from_secs(3),
        "the last frame: {clock:?}"
    );
    // It stays there: no end comes.
    let deadline = Instant::now() + Duration::from_secs(1);
    while let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        assert!(!matches!(event, Event::Ended(..)), "{event:?}");
    }
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_file_that_cannot_be_opened_is_reported_and_the_player_goes_on() {
    let dir = TestDir::new("missing");
    let clip = dir.clip(2);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let missing = player
        .open(&Source::file(dir.root.join("clips/missing.mkv")).unwrap())
        .unwrap();
    wait_for(
        &events,
        |e| matches!(e, Event::Ended(o, Ending::Failed(_)) if *o == missing),
    );

    let not_a_clip = dir.root.join("clips/text.mkv");
    std::fs::write(&not_a_clip, "not a clip\n").unwrap();
    let text = player.open(&Source::file(&not_a_clip).unwrap()).unwrap();
    wait_for(
        &events,
        |e| matches!(e, Event::Ended(o, Ending::Failed(_)) if *o == text),
    );

    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_seek_before_any_source_is_refused() {
    let dir = TestDir::new("refused");
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.seek(Duration::from_secs(1)).unwrap();
    let refused = wait_for(&events, |e| matches!(e, Event::Refused { .. }));
    assert!(
        matches!(
            refused,
            Event::Refused {
                command: "seek",
                ..
            }
        ),
        "{refused:?}"
    );
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_source_that_replaces_a_playing_one_gets_its_whole_report() {
    let dir = TestDir::new("replace");
    let clips = [dir.clip(3), dir.clip(2)];
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let mut last = None;
    for round in 0..4 {
        let open = player
            .open(&Source::file(&clips[round % 2]).unwrap())
            .unwrap();
        if let Some(replaced) = last {
            wait_for(&events, |e| *e == Event::Ended(replaced, Ending::Stopped));
        }
        wait_for(&events, |e| *e == Event::Started(open));
        // The same sound as the source it replaced, which mpv does not send again by itself.
        let report = complete_report(&events, open);
        assert_eq!(report.audio.unwrap().codec, "flac", "round {round}");
        last = Some(open);
    }
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn every_open_ends_once_and_starts_at_most_once() {
    let dir = TestDir::new("opens");
    let clip = dir.clip(2);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let source = Source::file(&clip).unwrap();
    // Opens in quick succession: mpv drops those a later one replaces before they load.
    let opens: Vec<OpenId> = (0..20).map(|_| player.open(&source).unwrap()).collect();
    let last = *opens.last().unwrap();
    let mut started = std::collections::HashMap::new();
    let mut ended = std::collections::HashMap::new();
    let mut stopped = false;
    let deadline = Instant::now() + common::WAIT;
    while !ended.contains_key(&last) {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("every open is answered in time");
        match event {
            Event::Started(open) => {
                *started.entry(open).or_insert(0) += 1;
                if open == last && !stopped {
                    player.stop().unwrap();
                    stopped = true;
                }
            }
            Event::Ended(open, _) => *ended.entry(open).or_insert(0) += 1,
            _ => {}
        }
    }
    for open in &opens {
        assert_eq!(ended.get(open), Some(&1), "{open:?} ends once");
        assert!(
            started.get(open).copied().unwrap_or(0) <= 1,
            "{open:?} starts at most once"
        );
    }
    assert_eq!(started.get(&last), Some(&1), "the last one plays");
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn an_open_stopped_at_once_ends_once() {
    let dir = TestDir::new("open-stop");
    let clip = dir.clip(2);
    let source = Source::file(&clip).unwrap();
    for round in 0..10 {
        // A fresh player each round: the open and the stop reach mpv as it starts.
        let (player, events) = Player::start(&dir.settings(&[])).unwrap();
        let open = player.open(&source).unwrap();
        player.stop().unwrap();
        wait_for(&events, |e| matches!(e, Event::Ended(o, _) if *o == open));
        let deadline = Instant::now() + Duration::from_millis(500);
        while let Ok(event) =
            events.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            assert!(
                !matches!(event, Event::Ended(o, _) | Event::Started(o) if o == open),
                "round {round}: {event:?} after the end"
            );
        }
    }
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_stream_carries_its_headers_and_no_secret_reaches_a_command_line() {
    let dir = TestDir::new("stream");
    let clip = dir.clip(3);
    let server = ClipServer::start(clip);
    // A marker no environment holds by chance.
    let marker = format!(
        "marker{}x{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let headers = vec![
        Header::new("Authorization", format!("Bearer {marker}, with a comma")).unwrap(),
        Header::new("X-Emby-Token", format!("{marker}, second")).unwrap(),
    ];
    let source = Source::stream(server.url(&format!("?api_key={marker}")), headers).unwrap();
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let open = player.open(&source).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));

    let heads = server.heads();
    assert!(!heads.is_empty());
    for head in &heads {
        assert!(
            head.starts_with(&format!("GET /clip.mkv?api_key={marker} ")),
            "{head}"
        );
        assert!(
            head.contains(&format!(
                "\r\nAuthorization: Bearer {marker}, with a comma\r\n"
            )),
            "{head}"
        );
        assert!(
            head.contains(&format!("\r\nX-Emby-Token: {marker}, second\r\n")),
            "{head}"
        );
    }
    let processes = common::mpv_of(&dir);
    assert!(!processes.is_empty(), "mpv runs");
    for pid in common::descendants() {
        let seen = common::command_line(pid) + &common::environment(pid);
        assert!(!seen.contains(&marker), "process {pid}: {seen}");
    }

    // The next source, a file, then a stream without headers, carry none of the first's.
    let file = dir.clip(2);
    let next = player.open(&Source::file(&file).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Ended(open, Ending::Stopped));
    wait_for(&events, |e| *e == Event::Started(next));
    let before = server.heads().len();
    let bare = player
        .open(&Source::stream(server.url(""), vec![]).unwrap())
        .unwrap();
    wait_for(&events, |e| *e == Event::Started(bare));
    let later = server.heads();
    assert!(later.len() > before);
    assert!(later[before..].iter().all(|head| !head.contains(&marker)));
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_crash_of_mpv_is_reported_and_aerith_goes_on() {
    let dir = TestDir::new("crash");
    let clip = dir.clip(4);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));

    let mpv = common::mpv_of(&dir);
    assert_eq!(mpv.len(), 1, "one mpv: {mpv:?}");
    // A segmentation fault, as a decoder fed a hostile file would end.
    let killed = std::process::Command::new("kill")
        .args(["-SEGV", &mpv[0].to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let reason = common::engine_failure(&events);
    assert!(reason.contains("mpv closed its connection"), "{reason}");
    // The engine's thread has ended: a command fails instead of waiting.
    let deadline = Instant::now() + common::WAIT;
    while player.play().is_ok() {
        assert!(Instant::now() < deadline, "commands fail once mpv is gone");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(player.play(), Err(EngineGone));
    drop(player);

    // A new player plays again.
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn dropping_the_player_ends_mpv() {
    let dir = TestDir::new("drop");
    let clip = dir.clip(4);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    let mpv = common::mpv_of(&dir);
    assert_eq!(mpv.len(), 1);
    drop(player);
    assert!(common::command_line(mpv[0]).is_empty(), "mpv is gone");
    dir.assert_home_untouched(&[]);
}
