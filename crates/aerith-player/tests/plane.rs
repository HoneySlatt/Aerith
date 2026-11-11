// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The real mpv through the relay, into a fake compositor's window (`tests/common/host.rs`): its
//! frames reach the plane, mpv stops drawing while the plane is hidden, mpv quits once the plane
//! is closed, and a crash of mpv leaves the window's connection whole.
//!
//! mpv draws with lavapipe, Mesa's Vulkan driver on the CPU, which the development shell does not
//! hold ([`common::lavapipe`]): these tests run only when `AERITH_TEST_WAYLAND=1` is set at build
//! time (`build.rs`), and are ignored otherwise, as in CI.

#![cfg(target_os = "linux")]

mod common;

use std::time::{Duration, Instant};

use aerith_player::{Event, Player, Source, Window};
use common::host::{FakeHost, Interface};
use common::{TestDir, wait_for};

/// A player showing its picture in a plane of `interface`'s window, placed at 320x240 and shown.
fn player(dir: &TestDir, interface: &Interface) -> (Player, std::sync::mpsc::Receiver<Event>) {
    assert!(
        common::lavapipe().is_some(),
        "these tests need lavapipe: name its JSON file in AERITH_TEST_LAVAPIPE"
    );
    let (display, parent, below) = interface.handles();
    // SAFETY: the interface's display and surfaces outlive the player: each test drops the player
    // first.
    let window = unsafe { Window::new(display, parent, below) };
    let (player, events) = Player::start(&dir.plane_settings(window, &[])).unwrap();
    let plane = player.plane().expect("a plane");
    plane.place(0, 0, 320, 240).unwrap();
    plane.show();
    interface.commit();
    (player, events)
}

/// How many frames mpv committed on the plane's surface so far.
fn frames(host: &FakeHost, interface: &Interface) -> u32 {
    host.with(|recorded| {
        interface
            .plane(recorded)
            .map_or(0, |surface| surface.buffer_commits)
    })
}

/// Waits until mpv committed `count` frames on the plane.
fn wait_for_frames(host: &FakeHost, interface: &Interface, count: u32) {
    host.wait(&format!("{count} frames of mpv's"), |recorded| {
        interface
            .plane(recorded)
            .filter(|surface| surface.buffer_commits >= count)
            .map(|_| ())
    });
}

#[test]
#[cfg_attr(
    not(aerith_test_wayland),
    ignore = "needs lavapipe: build with AERITH_TEST_WAYLAND=1 (CONTRIBUTING.md)"
)]
fn mpvs_frames_reach_the_plane_and_stop_while_it_is_hidden() {
    let dir = TestDir::new("plane");
    let clip = dir.clip(6);
    let host = FakeHost::start();
    let mut interface = host.connect(true);
    let (player, events) = player(&dir, &interface);
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    // mpv reports its video output drawing: its window and output were made at start.
    wait_for(&events, |e| {
        matches!(e, Event::Report(o, r)
            if *o == open && r.video.as_ref().is_some_and(|v| v.output == "gpu-next" && v.drawn.is_some()))
    });
    wait_for_frames(&host, &interface, 10);
    host.with(|recorded| {
        let surface = interface.plane(recorded).unwrap();
        let buffer = &recorded.buffers[surface.buffer.as_ref().unwrap()];
        assert!(!buffer.dmabuf, "lavapipe draws in shared memory");
        assert_eq!(surface.destination, Some((320, 240)));
        assert!(recorded.errors.is_empty(), "{:?}", recorded.errors);
    });
    // The relay counts each frame as the compositor's answer reaches it, a moment later.
    let plane = player.plane().unwrap();
    let deadline = Instant::now() + common::WAIT;
    while plane.presentation().presented < 10 {
        assert!(Instant::now() < deadline, "{:?}", plane.presentation());
        std::thread::sleep(Duration::from_millis(10));
    }

    // The relay counts each frame mpv commits: no fewer than the compositor has seen, as it sees
    // them after the relay.
    let seen = frames(&host, &interface);
    assert!(
        plane.presentation().committed >= u64::from(seen),
        "{seen} frames seen, {:?}",
        plane.presentation()
    );
    // Hidden, mpv is suspended and draws nothing: the relay counts what mpv commits, shown or
    // not. A frame it was drawing may still come.
    plane.hide();
    interface.commit();
    std::thread::sleep(Duration::from_millis(500));
    let hidden = frames(&host, &interface);
    let drawn = plane.presentation().committed;
    std::thread::sleep(Duration::from_secs(1));
    let drawn_while_hidden = plane.presentation().committed - drawn;
    assert!(
        drawn_while_hidden <= 1,
        "mpv drew {drawn_while_hidden} frames while hidden"
    );
    assert_eq!(
        frames(&host, &interface),
        hidden,
        "no frame shown while hidden"
    );
    plane.show();
    interface.commit();
    wait_for_frames(&host, &interface, hidden + 10);
    drop(player);
    interface
        .roundtrip()
        .expect("the window's connection lives on");
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
    dir.assert_home_untouched(&[]);
}

#[test]
#[cfg_attr(
    not(aerith_test_wayland),
    ignore = "needs lavapipe: build with AERITH_TEST_WAYLAND=1 (CONTRIBUTING.md)"
)]
fn mpv_quits_once_the_plane_is_closed() {
    let dir = TestDir::new("plane-close");
    let clip = dir.clip(6);
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (player, events) = player(&dir, &interface);
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    wait_for_frames(&host, &interface, 5);
    player.plane().unwrap().close();
    interface
        .roundtrip()
        .expect("the window's connection lives on");
    // mpv sees its Wayland connection end, takes it as its window closed, and quits by itself, in
    // the middle of the clip: the player reports the engine's end, as for any.
    let reason = common::engine_failure(&events);
    assert!(
        reason.contains("mpv closed its connection") && reason.contains("exited with code 0"),
        "{reason}"
    );
    assert!(common::mpv_of(&dir).is_empty(), "mpv is gone");
    drop(player);
    interface.roundtrip().unwrap();
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
    dir.assert_home_untouched(&[]);
}

#[test]
#[cfg_attr(
    not(aerith_test_wayland),
    ignore = "needs lavapipe: build with AERITH_TEST_WAYLAND=1 (CONTRIBUTING.md)"
)]
fn a_crash_of_mpv_leaves_the_window_whole() {
    let dir = TestDir::new("plane-crash");
    let clip = dir.clip(6);
    let host = FakeHost::start();
    let mut interface = host.connect(false);
    let (player, events) = player(&dir, &interface);
    let open = player.open(&Source::file(&clip).unwrap()).unwrap();
    wait_for(&events, |e| *e == Event::Started(open));
    wait_for_frames(&host, &interface, 5);
    let mpv = common::mpv_of(&dir);
    assert_eq!(mpv.len(), 1, "one mpv: {mpv:?}");
    let killed = std::process::Command::new("kill")
        .args(["-SEGV", &mpv[0].to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let reason = common::engine_failure(&events);
    assert!(reason.contains("mpv closed its connection"), "{reason}");
    interface
        .roundtrip()
        .expect("the window's connection lives on");
    // The plane closes with the player; the window goes on.
    drop(player);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.with(|recorded| interface.plane(recorded).is_some()) {
        assert!(
            Instant::now() < deadline,
            "the plane's surface is destroyed"
        );
        interface.roundtrip().unwrap();
    }
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
    dir.assert_home_untouched(&[]);
}
