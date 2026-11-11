// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The quality bar's floor at rest (docs/ROADMAP.md): a paused or stopped player wakes none of
//! Aerith's threads. Alone in its test binary, so that no other player's threads are counted.
//!
//! A thread's wake-ups are its context switches, from `/proc/self/task/<tid>/status`. mpv's own are
//! printed beside them: they are mpv's, not Aerith's, and the bench compares them with mpv alone.

#![cfg(target_os = "linux")]

mod common;

use std::fs;
use std::thread;
use std::time::Duration;

use aerith_player::{Ending, Event, Player, Source};
use common::{TestDir, wait_for};

/// How long each state is watched: a wake-up once a second, the slowest a timer would plausibly
/// take, shows at least once.
const WATCHED: Duration = Duration::from_secs(2);

/// The context switches of every thread of `pid` whose name `wanted` accepts, summed.
fn switches(pid: &str, wanted: impl Fn(&str) -> bool) -> u64 {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return 0;
    };
    tasks
        .flatten()
        .filter(|task| {
            fs::read_to_string(task.path().join("comm")).is_ok_and(|name| wanted(name.trim()))
        })
        .filter_map(|task| fs::read_to_string(task.path().join("status")).ok())
        .map(|status| {
            status
                .lines()
                .filter(|line| line.contains("ctxt_switches:"))
                .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
                .sum::<u64>()
        })
        .sum()
}

/// The wake-ups of the player's threads, and of mpv's, during [`WATCHED`].
fn watch(mpv: u32) -> (u64, u64) {
    let ours = |name: &str| name.starts_with("aerith-mpv");
    let (player_before, mpv_before) =
        (switches("self", ours), switches(&mpv.to_string(), |_| true));
    thread::sleep(WATCHED);
    let (player_after, mpv_after) = (switches("self", ours), switches(&mpv.to_string(), |_| true));
    (player_after - player_before, mpv_after - mpv_before)
}

#[test]
fn a_paused_or_stopped_player_wakes_none_of_aeriths_threads() {
    let dir = TestDir::new("rest");
    let clip = dir.clip(20);
    let (player, events) = Player::start(&dir.settings(&[])).unwrap();
    player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started);
    // Counted once they run: a thread takes its name as it starts.
    let found = fs::read_dir("/proc/self/task")
        .unwrap()
        .flatten()
        .filter(|task| {
            fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|name| name.trim().starts_with("aerith-mpv"))
        })
        .count();
    assert_eq!(found, 2, "the player's two threads are found by name");
    let mpv = common::mpv_of(&dir)[0];
    thread::sleep(Duration::from_millis(300));
    let (player_playing, mpv_playing) = watch(mpv);
    let seconds = WATCHED.as_secs_f64();
    eprintln!(
        "playing: the player's threads woke {:.1} times a second, mpv's {:.1}",
        player_playing as f64 / seconds,
        mpv_playing as f64 / seconds
    );

    player.pause().unwrap();
    wait_for(&events, |e| *e == Event::Paused(true));
    thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_ok() {}
    let (player_paused, mpv_paused) = watch(mpv);
    eprintln!(
        "paused: the player's threads woke {player_paused} times in {seconds} s, mpv's {mpv_paused}"
    );
    assert_eq!(player_paused, 0, "no wake-up of Aerith's own while paused");
    assert!(events.try_recv().is_err(), "and no event");

    player.stop().unwrap();
    wait_for(&events, |e| *e == Event::Ended(Ending::Stopped));
    thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_ok() {}
    let (player_stopped, mpv_stopped) = watch(mpv);
    eprintln!(
        "stopped: the player's threads woke {player_stopped} times in {seconds} s, mpv's {mpv_stopped}"
    );
    assert_eq!(
        player_stopped, 0,
        "no wake-up of Aerith's own while stopped"
    );
    assert!(events.try_recv().is_err(), "and no event");
    drop(player);
    dir.assert_home_untouched(&[]);
}
