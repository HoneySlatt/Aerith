// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The statistics `--stats` prints: what is read, and the JSON line it makes. The crate's
//! documentation gives the format.

use std::fmt::Write as _;
use std::fs;
use std::time::Duration;

use aerith_player::{DisplayRate, Presentation};

/// A thread's context switches, from `/proc/self/task/<tid>/status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    /// The thread gave up the CPU to wait: each one is a sleep, and so a wake-up after it.
    pub voluntary: u64,
    /// The thread was preempted.
    pub involuntary: u64,
}

/// The switches of this process's thread `tid`, or `None` if it cannot be read: the thread ended.
pub fn thread_switches(tid: u32) -> Option<Switches> {
    parse_switches(&fs::read_to_string(format!("/proc/self/task/{tid}/status")).ok()?)
}

/// The switches a `status` file gives.
fn parse_switches(status: &str) -> Option<Switches> {
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|value| value.trim().parse().ok())
    };
    Some(Switches {
        voluntary: field("voluntary_ctxt_switches")?,
        involuntary: field("nonvoluntary_ctxt_switches")?,
    })
}

/// The id of this process's thread named `name`, if one runs. Linux keeps the first 15 bytes of a
/// thread's name. A scan of `/proc/self/task` may miss a thread while others start or end: the
/// caller looks again later rather than take an absence as final.
pub fn thread_named(name: &str) -> Option<u32> {
    fs::read_dir("/proc/self/task")
        .ok()?
        .flatten()
        .find(|task| {
            fs::read_to_string(task.path().join("comm")).is_ok_and(|comm| comm.trim_end() == name)
        })
        .and_then(|task| task.file_name().to_str()?.parse().ok())
}

/// The player's clock as a line reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockReading {
    /// The position at the moment of the line, in seconds.
    pub position: f64,
    pub running: bool,
    pub rate: f64,
}

/// What one line reports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub since_open: Duration,
    pub presentation: Presentation,
    pub main_thread: Option<Switches>,
    pub relay_thread: Option<Switches>,
    pub clock: Option<ClockReading>,
    pub ui_frames: u64,
    pub ui_frames_late: u64,
}

/// The line printed each second.
pub fn line(sample: &Sample) -> String {
    format!("{{{}}}", fields(sample))
}

/// The line printed at the end, with the time from the open to the first frame.
pub fn summary(sample: &Sample, first_frame: Option<Duration>) -> String {
    let first_frame = first_frame.map_or("null".to_owned(), |first| {
        format!("{:.1}", first.as_secs_f64() * 1000.0)
    });
    format!(
        "{{\"summary\":true,\"first_frame_ms\":{first_frame},{}}}",
        fields(sample)
    )
}

/// The fields every line holds, without the braces.
fn fields(sample: &Sample) -> String {
    let presentation = &sample.presentation;
    let gaps: Vec<String> = presentation.gaps.iter().map(u64::to_string).collect();
    let switches = |switches: Option<Switches>| {
        switches.map_or("null".to_owned(), |s| {
            format!(
                "{{\"voluntary\":{},\"involuntary\":{}}}",
                s.voluntary, s.involuntary
            )
        })
    };
    let clock = sample.clock.map_or("null".to_owned(), |clock| {
        format!(
            "{{\"position\":{:.3},\"running\":{},\"rate\":{:.6}}}",
            clock.position, clock.running, clock.rate
        )
    });
    let display_rate = match presentation.display_rate {
        None => "null",
        Some(DisplayRate::Fixed) => "\"fixed\"",
        Some(DisplayRate::Variable) => "\"variable\"",
    };
    let last_probe = presentation.last_probe.map_or("null".to_owned(), |probe| {
        format!("{:.1}", probe.as_secs_f64() * 1000.0)
    });
    let mut text = String::new();
    // Writing to a String cannot fail.
    let _ = write!(
        text,
        "\"since_open\":{:.3},\"presentation\":{{\"committed\":{},\"presented\":{},\
         \"discarded\":{},\"zero_copy\":{},\"gaps\":[{}],\"longer_gaps\":{},\"probes_ended\":{},\
         \"display_rate\":{},\"last_probe_ms\":{}}},\
         \"main_thread\":{},\"relay_thread\":{},\"clock\":{},\"ui_frames\":{},\
         \"ui_frames_late\":{}",
        sample.since_open.as_secs_f64(),
        presentation.committed,
        presentation.presented,
        presentation.discarded,
        presentation.zero_copy,
        gaps.join(","),
        presentation.longer_gaps,
        presentation.probes_ended,
        display_rate,
        last_probe,
        switches(sample.main_thread),
        switches(sample.relay_thread),
        clock,
        sample.ui_frames,
        sample.ui_frames_late,
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Sample {
        let mut gaps = [0; aerith_player::COUNTED_GAPS];
        gaps[10] = 12;
        gaps[11] = 11;
        Sample {
            since_open: Duration::from_millis(3000),
            presentation: Presentation {
                committed: 72,
                presented: 70,
                discarded: 2,
                zero_copy: 5,
                gaps,
                longer_gaps: 1,
                probes_ended: 1,
                display_rate: Some(DisplayRate::Fixed),
                last_probe: Some(Duration::from_micros(1_402_340)),
            },
            main_thread: Some(Switches {
                voluntary: 310,
                involuntary: 4,
            }),
            relay_thread: None,
            clock: Some(ClockReading {
                position: 2.5,
                running: true,
                rate: 0.99317,
            }),
            ui_frames: 3,
            ui_frames_late: 0,
        }
    }

    const FIELDS: &str = "\"since_open\":3.000,\"presentation\":{\"committed\":72,\
        \"presented\":70,\"discarded\":2,\"zero_copy\":5,\
        \"gaps\":[0,0,0,0,0,0,0,0,0,0,12,11,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],\
        \"longer_gaps\":1,\"probes_ended\":1,\"display_rate\":\"fixed\",\"last_probe_ms\":1402.3},\
        \"main_thread\":{\"voluntary\":310,\"involuntary\":4},\
        \"relay_thread\":null,\"clock\":{\"position\":2.500,\"running\":true,\
        \"rate\":0.993170},\"ui_frames\":3,\"ui_frames_late\":0";

    #[test]
    fn a_line_is_one_json_object_with_every_field() {
        assert_eq!(line(&sample()), format!("{{{FIELDS}}}"));
    }

    #[test]
    fn the_summary_adds_the_first_frame() {
        assert_eq!(
            summary(&sample(), Some(Duration::from_micros(812_345))),
            format!("{{\"summary\":true,\"first_frame_ms\":812.3,{FIELDS}}}")
        );
        let without = Sample {
            clock: None,
            ..sample()
        };
        let text = summary(&without, None);
        assert!(text.starts_with("{\"summary\":true,\"first_frame_ms\":null,"));
        assert!(text.contains(",\"clock\":null,"));
    }

    #[test]
    fn switches_are_read_from_a_status_file() {
        let status = "Name:\taerith-relay\nState:\tS (sleeping)\n\
            voluntary_ctxt_switches:\t1234\nnonvoluntary_ctxt_switches:\t56\n";
        assert_eq!(
            parse_switches(status),
            Some(Switches {
                voluntary: 1234,
                involuntary: 56,
            })
        );
        assert_eq!(parse_switches("voluntary_ctxt_switches:\t1\n"), None);
        assert_eq!(parse_switches(""), None);
    }

    #[test]
    fn a_threads_switches_are_found_by_its_name() {
        // The thread is looked for once `spawn` has returned and the thread runs, and waits until
        // it was found: its name is in place by then, whichever thread sets it.
        let (running, started) = std::sync::mpsc::channel();
        let (found, done) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("aerith-stats-tst".to_owned())
            .spawn(move || {
                running.send(()).unwrap();
                done.recv().unwrap();
            })
            .unwrap();
        started.recv().unwrap();
        // Linux keeps 15 bytes of the name. Other tests' threads start and end meanwhile, and a
        // scan may miss this one: looked for a few times, as the stats loop looks each second.
        let tid = (0..100)
            .find_map(|_| thread_named("aerith-stats-ts"))
            .expect("the thread by its name");
        assert!(thread_switches(tid).is_some(), "its switches");
        found.send(()).unwrap();
        thread.join().unwrap();
        assert_eq!(thread_named("no-such-thread"), None);
    }
}
