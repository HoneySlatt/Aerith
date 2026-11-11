// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The quality bar's floor at rest (docs/ROADMAP.md): a paused or stopped player wakes none of
//! Aerith's threads. Alone in its test binary, so that no other player's threads are counted.
//!
//! The player shows its picture in a plane of a fake compositor's window (`tests/common/host.rs`),
//! so that the relay's thread is counted too. mpv draws there with lavapipe where the machine has
//! it ([`common::lavapipe`]); elsewhere, as in CI, its video output fails and the relay serves
//! nothing, and the counts are the engine's threads' alone in all but name.
//!
//! A thread's wake-ups are its context switches, from `/proc/self/task/<tid>/status`. mpv's own are
//! printed beside them: they are mpv's, not Aerith's, and the bench compares them with mpv alone.

#![cfg(target_os = "linux")]

mod common;

use std::fs;
use std::thread;
use std::time::Duration;

use aerith_player::{Ending, Event, Player, Source, Window};
use common::host::FakeHost;
use common::{TestDir, wait_for};

/// How long the events a change brings are given to arrive, on a loaded machine, before the
/// watch: one that came during it would count as a wake-up.
const SETTLE: Duration = Duration::from_secs(1);

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
    let ours = |name: &str| name.starts_with("aerith-");
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
    let host = FakeHost::start();
    let interface = host.connect(false);
    let (display, parent, below) = interface.handles();
    // SAFETY: the interface's display and surface outlive the player, dropped below first.
    let window = unsafe { Window::new(display, parent, below) };
    let (player, events) = Player::start(&dir.plane_settings(window, &[])).unwrap();
    let plane = player.plane().unwrap();
    plane.place(0, 0, 320, 240).unwrap();
    plane.show();
    interface.commit();
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    // Counted once they run: a thread takes its name as it starts.
    let found = fs::read_dir("/proc/self/task")
        .unwrap()
        .flatten()
        .filter(|task| {
            fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|name| name.trim().starts_with("aerith-"))
        })
        .count();
    assert_eq!(
        found, 3,
        "the player's three threads, the relay's among them, are found by name"
    );
    let mpv = common::mpv_of(&dir)[0];
    thread::sleep(SETTLE);
    let (player_playing, mpv_playing) = watch(mpv);
    let seconds = WATCHED.as_secs_f64();
    eprintln!(
        "playing: the player's threads woke {:.1} times a second, mpv's {:.1}; the compositor \
         presented {} of mpv's frames (lavapipe {})",
        player_playing as f64 / seconds,
        mpv_playing as f64 / seconds,
        plane.presentation().presented,
        if common::lavapipe().is_some() {
            "found"
        } else {
            "absent: no picture"
        },
    );

    player.pause().unwrap();
    wait_for(&events, |e| *e == Event::Paused(true));
    thread::sleep(SETTLE);
    while events.try_recv().is_ok() {}
    let (player_paused, mpv_paused) = watch(mpv);
    eprintln!(
        "paused: the player's threads woke {player_paused} times in {seconds} s, mpv's {mpv_paused}"
    );
    assert_eq!(player_paused, 0, "no wake-up of Aerith's own while paused");
    assert!(events.try_recv().is_err(), "and no event");

    player.stop().unwrap();
    wait_for(&events, |e| *e == Event::Ended(open, Ending::Stopped));
    thread::sleep(SETTLE);
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
