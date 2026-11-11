// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The run's timeline, on a thread of its own, `aerith-testbed-schedule`, so that the window's
//! thread sleeps between what the person does: it tells the window's loop when to open the source,
//! pause, show or hide the controls and quit, and prints the statistics each second.

use std::io::{self, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aerith_player::{Clock, Player};
use winit::event_loop::EventLoopProxy;

use crate::stats::{self, ClockReading, Sample};

/// How long after the player starts the source is opened: mpv's start is then over, and the time
/// to the first frame leaves it out, as Aerith's player is started before the person presses Play.
pub const OPEN_DELAY: Duration = Duration::from_secs(1);
/// How long to wait for the first frame before giving up.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(30);
/// When `--toggle-controls` first shows the controls after the first frame, and how long each
/// show and each hide lasts.
const TOGGLE_START: Duration = Duration::from_secs(1);
const TOGGLE_STEP: Duration = Duration::from_secs(1);
const STATS_EVERY: Duration = Duration::from_secs(1);

/// What the schedule asks of the window's loop.
#[derive(Debug)]
pub enum Cue {
    Open,
    Pause,
    /// Show (`true`) or hide the controls.
    Controls(bool),
    /// Quit: the run is over, or failed.
    Quit(Result<(), String>),
}

/// What the window's loop tells the schedule.
#[derive(Clone, Copy, Debug)]
pub enum Note {
    Opened(Instant),
    FirstFrame(Instant),
}

/// The run's length and what it does along the way, from the command line.
#[derive(Clone, Copy, Debug)]
pub struct Timeline {
    pub seconds: Duration,
    pub pause_after: Option<Duration>,
    pub toggle_controls: u32,
}

/// What the window's loop shares with the statistics.
#[derive(Debug, Default)]
pub struct Shared {
    /// The player's last clock.
    pub clock: Mutex<Option<Clock>>,
    pub ui_frames: AtomicU64,
    pub ui_frames_late: AtomicU64,
}

/// What the statistics read.
pub struct StatsSource {
    pub player: Arc<Player>,
    pub shared: Arc<Shared>,
    /// The window's thread.
    pub main_thread: u32,
}

/// The schedule's thread. Dropping it stops the thread, which prints the summary first.
pub struct Schedule {
    notes: Option<Sender<Note>>,
    thread: Option<JoinHandle<()>>,
}

impl Schedule {
    /// Starts the timeline at `started`, the player's start. `send` hands a cue to the window's
    /// loop; with `stats`, the statistics are printed too.
    pub fn start<T: From<Cue> + Send + 'static>(
        started: Instant,
        timeline: Timeline,
        stats: Option<StatsSource>,
        loop_proxy: EventLoopProxy<T>,
    ) -> io::Result<Self> {
        let (notes, received) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("aerith-testbed-schedule".to_owned())
            .spawn(move || {
                let send = |cue: Cue| {
                    // The loop is gone: the run is ending anyway.
                    let _ = loop_proxy.send_event(T::from(cue));
                };
                run(started, timeline, stats, &received, &send);
            })?;
        Ok(Self {
            notes: Some(notes),
            thread: Some(thread),
        })
    }

    pub fn note(&self, note: Note) {
        if let Some(notes) = &self.notes {
            // The thread ended: it has nothing left to do.
            let _ = notes.send(note);
        }
    }
}

impl Drop for Schedule {
    fn drop(&mut self) {
        // A closed channel stops the thread.
        self.notes = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What is due and when.
struct Progress {
    open_sent: bool,
    opened: Option<Instant>,
    first_frame: Option<Instant>,
    pause_sent: bool,
    toggles_sent: u32,
    quit_sent: bool,
    next_stats: Option<Instant>,
}

fn run(
    started: Instant,
    timeline: Timeline,
    stats: Option<StatsSource>,
    notes: &Receiver<Note>,
    send: &dyn Fn(Cue),
) {
    let mut progress = Progress {
        open_sent: false,
        opened: None,
        first_frame: None,
        pause_sent: false,
        toggles_sent: 0,
        quit_sent: false,
        next_stats: None,
    };
    let mut relay_thread = None;
    loop {
        let now = Instant::now();
        let mut deadlines = Vec::new();
        let mut due = |at: Instant| {
            deadlines.push(at);
            now >= at
        };
        if !progress.open_sent && due(started + OPEN_DELAY) {
            send(Cue::Open);
            progress.open_sent = true;
        }
        if let (Some(opened), None) = (progress.opened, progress.first_frame)
            && !progress.quit_sent
            && due(opened + FIRST_FRAME_WAIT)
        {
            send(Cue::Quit(Err(format!(
                "no first frame {} s after the open",
                FIRST_FRAME_WAIT.as_secs()
            ))));
            progress.quit_sent = true;
        }
        if let Some(first) = progress.first_frame {
            if let Some(pause_after) = timeline.pause_after
                && !progress.pause_sent
                && due(first + pause_after)
            {
                send(Cue::Pause);
                progress.pause_sent = true;
            }
            while progress.toggles_sent < timeline.toggle_controls.saturating_mul(2)
                && due(first + TOGGLE_START + TOGGLE_STEP * progress.toggles_sent)
            {
                send(Cue::Controls(progress.toggles_sent.is_multiple_of(2)));
                progress.toggles_sent += 1;
            }
            if !progress.quit_sent && due(first + timeline.seconds) {
                send(Cue::Quit(Ok(())));
                progress.quit_sent = true;
            }
        }
        if let (Some(source), Some(next), Some(opened)) =
            (&stats, progress.next_stats, progress.opened)
            && due(next)
        {
            // Looked for until found: a scan of the process's threads can miss one while others
            // start or end.
            if relay_thread.is_none() {
                relay_thread = stats::thread_named("aerith-relay");
            }
            let relay = relay_thread;
            print_line(&stats::line(&sample(source, opened, relay, now)));
            // A late tick is skipped, not caught up.
            let mut following = next + STATS_EVERY;
            while following <= now {
                following += STATS_EVERY;
            }
            progress.next_stats = Some(following);
        }
        deadlines.extend(progress.next_stats);
        let deadline = deadlines.into_iter().filter(|at| *at > now).min();
        let note = match deadline {
            Some(at) => notes.recv_timeout(at - now),
            None => notes.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match note {
            Ok(Note::Opened(at)) => {
                progress.opened = Some(at);
                if stats.is_some() {
                    progress.next_stats = Some(at + STATS_EVERY);
                }
            }
            Ok(Note::FirstFrame(at)) => progress.first_frame = Some(at),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if let (Some(source), Some(opened)) = (&stats, progress.opened) {
        let relay = relay_thread.or_else(|| stats::thread_named("aerith-relay"));
        let first_frame = progress.first_frame.map(|first| first - opened);
        let sample = sample(source, opened, relay, Instant::now());
        print_line(&stats::summary(&sample, first_frame));
    }
}

/// What the statistics read at `now`.
fn sample(
    source: &StatsSource,
    opened: Instant,
    relay_thread: Option<u32>,
    now: Instant,
) -> Sample {
    let clock = *source
        .shared
        .clock
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    Sample {
        since_open: now.saturating_duration_since(opened),
        presentation: source
            .player
            .plane()
            .map(|plane| plane.presentation())
            .unwrap_or_default(),
        main_thread: stats::thread_switches(source.main_thread),
        relay_thread: relay_thread.and_then(stats::thread_switches),
        clock: clock.map(|clock| ClockReading {
            position: clock.position_at(now).as_secs_f64(),
            running: clock.running,
            rate: clock.rate,
        }),
        ui_frames: source.shared.ui_frames.load(Ordering::Relaxed),
        ui_frames_late: source.shared.ui_frames_late.load(Ordering::Relaxed),
    }
}

/// One line on stdout, sent at once so that a reader sees each second as it comes.
fn print_line(line: &str) {
    let mut out = io::stdout().lock();
    // stdout closed (a reader that quit): the lines are lost, the run goes on.
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}
