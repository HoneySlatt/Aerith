// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The player against the real mpv, on clips mpv makes itself, with the picture and the sound
//! discarded: what CI runs on every change. `tests/corpus.rs` plays the test corpus the same way.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use aerith_player::{Ending, EngineGone, Event, Header, Player, Source};
use common::{ClipServer, TestDir, wait_for};

/// Whether `position` is within a frame and a half of `expected`, at 24 frames a second.
fn near(position: Duration, expected: Duration) -> bool {
    position.abs_diff(expected) <= Duration::from_millis(63)
}

/// Whether a running clock read at `position` started from `expected`: mpv has played a few frames
/// by the time it answers, two in these tests, without sound to wait for.
fn just_after(position: Duration, expected: Duration) -> bool {
    position >= expected && position - expected <= Duration::from_millis(150)
}

#[test]
fn a_file_plays_pauses_seeks_and_stops() {
    let dir = TestDir::new("file");
    let clip = dir.clip(4);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started);

    // What mpv says comes property by property: the report fills in over a few events.
    let complete = |r: &aerith_player::Report| {
        r.video.as_ref().is_some_and(|v| v.output != "none")
            && r.audio.as_ref().is_some_and(|a| a.sent.is_some())
    };
    let Event::Report(report) = wait_for(&events, |e| matches!(e, Event::Report(r) if complete(r)))
    else {
        unreachable!()
    };
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
    wait_for(
        &events,
        |e| matches!(e, Event::Clock(c) if !c.running && near(c.position, Duration::from_secs(2))),
    );

    player.play().unwrap();
    wait_for(&events, |e| *e == Event::Paused(false));
    let Event::Clock(clock) = wait_for(&events, |e| matches!(e, Event::Clock(c) if c.running))
    else {
        unreachable!()
    };
    assert!(
        just_after(clock.position, Duration::from_secs(2)),
        "{clock:?}"
    );

    player.stop().unwrap();
    wait_for(&events, |e| *e == Event::Ended(Ending::Stopped));
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_file_plays_to_its_end_in_real_time() {
    let dir = TestDir::new("end");
    let clip = dir.clip(1);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started);
    let started = std::time::Instant::now();
    wait_for(&events, |e| *e == Event::Ended(Ending::Finished));
    assert!(
        started.elapsed() > Duration::from_millis(800),
        "it played, not raced"
    );
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_file_that_cannot_be_opened_is_reported_and_the_player_goes_on() {
    let dir = TestDir::new("missing");
    let clip = dir.clip(2);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player
        .open(&Source::file(dir.root.join("clips/missing.mkv")).unwrap())
        .unwrap();
    let ended = wait_for(&events, |e| matches!(e, Event::Ended(_)));
    assert!(
        matches!(ended, Event::Ended(Ending::Failed(_))),
        "{ended:?}"
    );

    let not_a_clip = dir.root.join("clips/text.mkv");
    std::fs::write(&not_a_clip, "not a clip\n").unwrap();
    player.open(&Source::file(&not_a_clip).unwrap()).unwrap();
    let ended = wait_for(&events, |e| matches!(e, Event::Ended(_)));
    assert!(
        matches!(ended, Event::Ended(Ending::Failed(_))),
        "{ended:?}"
    );

    player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started);
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
fn a_stream_carries_its_headers_and_no_secret_reaches_a_command_line() {
    let dir = TestDir::new("stream");
    let clip = dir.clip(3);
    let server = ClipServer::start(clip);
    let headers = vec![
        Header::new("Authorization", "Bearer header-secret, with a comma").unwrap(),
        Header::new("X-Emby-Token", "second-secret").unwrap(),
    ];
    let source = Source::stream(server.url("?api_key=query-secret"), headers).unwrap();
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.open(&source).unwrap();
    wait_for(&events, |e| *e == Event::Started);

    let heads = server.heads();
    assert!(!heads.is_empty());
    for head in &heads {
        assert!(
            head.starts_with("GET /clip.mkv?api_key=query-secret "),
            "{head}"
        );
        assert!(
            head.contains("\r\nAuthorization: Bearer header-secret, with a comma\r\n"),
            "{head}"
        );
        assert!(
            head.contains("\r\nX-Emby-Token: second-secret\r\n"),
            "{head}"
        );
    }
    let processes = common::mpv_of(&dir);
    assert!(!processes.is_empty(), "mpv runs");
    for pid in common::descendants() {
        let seen = common::command_line(pid) + &common::environment(pid);
        assert!(!seen.contains("secret"), "process {pid}: {seen}");
    }

    // The next source, a file, is opened without the stream's headers.
    let file = dir.clip(2);
    player.open(&Source::file(&file).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Ended(Ending::Stopped));
    wait_for(&events, |e| *e == Event::Started);
    let before = server.heads().len();
    player
        .open(&Source::stream(server.url(""), vec![]).unwrap())
        .unwrap();
    wait_for(&events, |e| *e == Event::Started);
    let later = server.heads();
    assert!(later.len() > before);
    assert!(later[before..].iter().all(|head| !head.contains("secret")));
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn a_crash_of_mpv_is_reported_and_aerith_goes_on() {
    let dir = TestDir::new("crash");
    let clip = dir.clip(4);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started);

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
    let deadline = std::time::Instant::now() + common::WAIT;
    while player.play().is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "commands fail once mpv is gone"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(player.play(), Err(EngineGone));
    drop(player);

    // A new player plays again.
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started);
    drop(player);
    dir.assert_home_untouched(&[]);
}

#[test]
fn dropping_the_player_ends_mpv() {
    let dir = TestDir::new("drop");
    let clip = dir.clip(4);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started);
    let mpv = common::mpv_of(&dir);
    assert_eq!(mpv.len(), 1);
    drop(player);
    assert!(common::command_line(mpv[0]).is_empty(), "mpv is gone");
    dir.assert_home_untouched(&[]);
}
