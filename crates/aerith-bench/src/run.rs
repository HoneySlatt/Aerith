// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One run: a rival plays one clip, and the bench writes what it measured to `result.json` in the
//! run's own folder, with the player's logs beside it.
//!
//! The order of a run: the GPU's idle power is read, ten seconds unless asked otherwise; the silent sound server
//! starts; the clip's first 256 MiB are read once, so that every rival reads it from the page
//! cache;
//! the rival starts with a fresh profile, so its shader caches are empty: the first frame
//! includes compiling them, as on a first start; the bench loads the clip and times its first frame;
//! for the run's length it reads, each second, the engine's counters, the tree's memory and CPU
//! and the GPU's power; it asks the engine what it decodes and outputs at the first frame and again two seconds in; then it
//! stops everything it started.
//!
//! Aerith's run differs in one way: its testbed opens the clip itself, as Aerith would, and the
//! bench only watches its mpv; the testbed quits by itself once the run's length has played.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::audio::{self, SilentAudio};
use crate::engine::Counters;
use crate::figures;
use crate::presentation;
use crate::process;
use crate::rivals::{self, Rival, Testbed};
use crate::sampler::{self, Sampler};
use crate::sandbox::Sandbox;
use crate::serve::ClipServer;

/// The version of `result.json`'s layout; a change to its fields raises it.
pub const RESULT_FORMAT: u32 = 2;
/// When the engine is asked what it decodes and outputs, after the first frame: once it settled.
const REPORT_AFTER: Duration = Duration::from_secs(2);
/// The first seconds of frames presented that the figures leave out: the start settles.
const SETTLE: f64 = 2.0;
/// How long Aerith's testbed plays past the bench's last reading before it quits by itself, and
/// how long the bench then waits for it.
const TESTBED_MARGIN: f64 = 2.0;
const TESTBED_EXIT: Duration = Duration::from_secs(15);
/// How much of the clip is read before the run, to warm the cache.
const WARM_BYTES: u64 = 256 << 20;

/// What a run is asked to do.
#[derive(Debug, PartialEq)]
pub struct RunOptions {
    /// The rival to start.
    pub rival: Rival,
    /// The clip to play.
    pub clip: PathBuf,
    /// How long to play, at most; the clip's end stops the run sooner.
    pub seconds: f64,
    /// Where the run's folder is made.
    pub out: PathBuf,
    /// What the owner says of the display (`DP-3 3840x2160@240`): the bench cannot read it.
    pub display: String,
    /// Options added to mpv's command line, as `name=value`: mpv alone's, or Aerith's mpv's.
    pub mpv_options: Vec<String>,
    /// For Aerith: the order of the surfaces, and how many times the controls show and hide.
    pub order: Option<String>,
    pub toggle_controls: Option<u32>,
    /// The times, in seconds, of the frames to capture instead of measuring the playback, for
    /// mpv alone and Aerith: each is written to `frame-<time>.png` in the run's folder.
    pub frames: Vec<f64>,
    /// Serve the clip over HTTP on the loopback interface and play it as a stream.
    pub stream: bool,
    /// How long the GPU's idle power is read before the rival starts; zero skips it.
    pub idle: Duration,
}

/// Runs once and returns the path of `result.json`.
pub fn run(options: &RunOptions) -> io::Result<PathBuf> {
    let clip = fs::canonicalize(&options.clip)?;
    let clip_name = clip
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    fs::create_dir_all(&options.out)?;
    let directory = fs::canonicalize(&options.out)?.join(format!(
        "{}-{}-{}{}",
        figures::iso_utc(started_at).replace(':', ""),
        options.rival.name(),
        clip_name,
        if options.stream { "-stream" } else { "" }
    ));
    fs::create_dir_all(&directory)?;
    let binary = process::resolve(options.rival.program());
    let mut notes = Vec::new();

    let sensor = sampler::gpu_power_sensor();
    if sensor.is_none() {
        notes.push("no GPU power sensor found: power is not measured".to_owned());
    }
    let idle = match &sensor {
        Some(sensor) if !options.idle.is_zero() => {
            eprintln!(
                "reading the GPU's idle power for {} s",
                options.idle.as_secs()
            );
            sampler::idle_gpu_microwatts(sensor, options.idle)
        }
        _ => None,
    };

    // Sockets live apart, under a short path: a socket's path is limited to 108 bytes.
    let sockets =
        std::env::temp_dir().join(format!("aerith-bench-{}-{started_at}", std::process::id()));
    fs::create_dir_all(&sockets)?;
    let measured = measure(options, &clip, &directory, &sockets, sensor.clone());
    let _ = fs::remove_dir_all(&sockets);
    let Measured {
        played,
        samples,
        logged_drops,
        logged_sync,
        wayland_log,
    } = measured?;

    if let Some(device) = &played.sound_device {
        // The run is void: write nothing that looks like a figure.
        return Err(io::Error::other(SoundDeviceOpened(device.clone())));
    }
    let playing: Vec<_> = played
        .first_frame_at
        .map(|at| samples.iter().filter(|s| s.at >= at).cloned().collect())
        .unwrap_or_default();
    let last = |name: &str| -> Value {
        played
            .readings
            .iter()
            .rev()
            .find_map(|counters| value_of(counters, name))
            .map_or(Value::Null, Value::from)
    };
    let avsync: Vec<f64> = played
        .readings
        .iter()
        // The first seconds settle the clock after the start; mpv's own figure is read after.
        .skip(2)
        .filter_map(|counters| value_of(counters, "avsync"))
        .collect();
    let display_sync = figures::display_sync_figures(
        &played
            .readings
            .iter()
            .skip(2)
            .filter_map(|counters| value_of(counters, "display-sync-active"))
            .collect::<Vec<_>>(),
    );
    let frames = if options.rival == Rival::Kodi {
        notes.push(
            "Kodi publishes no frame counter: `dropped` counts the frames its debug log says it \
             dropped at output, a lower bound; its decoder's drops and its renderer's skips are \
             shown only on its overlay. Its sync is the average error its audio engine logs when \
             it checks or corrects the sync, not a reading each second"
                .to_owned(),
        );
        json!({"dropped": logged_drops, "decoder_dropped": null, "delayed": null, "mistimed": null,
               "source": "Kodi's debug log, lines `dropped in output`"})
    } else {
        json!({"dropped": last("frame-drop-count"), "decoder_dropped": last("decoder-frame-drop-count"),
               "delayed": last("vo-delayed-frame-count"), "mistimed": last("mistimed-frame-count"),
               "source": "mpv's properties, read over its IPC"})
    };
    let machine = figures::machine_figures(&playing, idle);
    // The frames' rate on screen: the clip's, times mpv's correction under display sync.
    let frame_rate = played.report["container-fps"]
        .as_f64()
        .map(|rate| rate * last("video-speed-correction").as_f64().unwrap_or(1.0));
    let presentation = match (&wayland_log, played.wayland_window) {
        (Some(log), Some(window)) => presentation::figures(
            &presentation::parse(&read_window(log, window)?),
            SETTLE,
            frame_rate,
        ),
        _ => Value::Null,
    };
    let ideal_gap = presentation["refresh_ms"]
        .as_f64()
        .zip(frame_rate)
        .map(|(refresh, rate)| 1000.0 / refresh / rate);
    let testbed = if options.rival == Rival::Aerith {
        testbed_figures(
            &fs::read_to_string(directory.join("player.log")).unwrap_or_default(),
            ideal_gap,
        )
    } else {
        Value::Null
    };
    let result = json!({
        "format": RESULT_FORMAT,
        "date": figures::iso_utc(started_at),
        "rival": options.rival.name(),
        "binary": binary.map(|path| path.display().to_string()),
        "mpv_options": options.mpv_options,
        "order": options.order,
        "frames_captured": played.captured,
        "toggle_controls": options.toggle_controls,
        "clip": clip_name,
        "clip_path": clip.display().to_string(),
        "source": if options.stream { "HTTP on 127.0.0.1, served by the bench" } else { "local file" },
        "machine": machine_description(),
        "display": options.display,
        "audio": format!("the bench's silent PipeWire, sink {}", audio::SINK),
        "seconds_asked": options.seconds,
        "seconds_played": played.seconds_played.map(|s| figures::round(s, 1)),
        "first_frame_ms": played.first_frame.map(|d| figures::round(d.as_secs_f64() * 1000.0, 1)),
        "first_frame_marker": match options.rival {
            Rival::Kodi => "Kodi's Player.OnAVStart notification after Player.Open",
            Rival::Aerith => "mpv's playback-restart event after its start-file, the testbed opening the clip",
            _ => "mpv's playback-restart event after loadfile",
        },
        "frames": frames,
        "presentation": presentation,
        "presentation_source": wayland_log.as_ref().map(|_| "the compositor's presentation feedback to mpv, from mpv's WAYLAND_DEBUG log, the first 2 s left out"),
        "testbed": testbed,
        "sync": figures::sync_figures(logged_sync.as_deref().unwrap_or(&avsync)),
        "display_sync": display_sync,
        "memory_mib": machine["memory_mib"],
        "cpu": machine["cpu"],
        "gpu_power_w": machine["gpu_power_w"],
        "gpu_power_sensor": sensor.map(|path| path.display().to_string()),
        "engine": played.report,
        "readings": played.readings.iter().map(|counters| {
            counters.values.iter().map(|(name, value)| (name.clone(), json!(value))).collect::<serde_json::Map<_, _>>()
        }).collect::<Vec<_>>(),
        "samples": samples.iter().map(|s| json!({"at": figures::round(s.at, 2), "pss_kib": s.pss_kib,
            "cpu_ticks": s.cpu_ticks, "machine_busy_ticks": s.machine_busy_ticks, "gpu_microwatts": s.gpu_microwatts, "processes": s.processes})).collect::<Vec<_>>(),
        "errors": played.error.map(|error| vec![error]).unwrap_or_default(),
        "notes": notes,
    });
    let path = directory.join("result.json");
    let partial = directory.join("result.json.partial");
    fs::write(
        &partial,
        serde_json::to_vec_pretty(&result).map_err(io::Error::other)?,
    )?;
    fs::rename(&partial, &path)?;
    Ok(path)
}

/// What the rival's run gave, before it becomes `result.json`.
struct Measured {
    played: Played,
    samples: Vec<sampler::Sample>,
    logged_drops: Option<u64>,
    logged_sync: Option<Vec<f64>>,
    wayland_log: Option<PathBuf>,
}

/// Starts the silent sound server and the rival, plays the clip, and stops both, whatever happened.
fn measure(
    options: &RunOptions,
    clip: &Path,
    directory: &Path,
    sockets: &Path,
    sensor: Option<PathBuf>,
) -> io::Result<Measured> {
    let home = directory.join("home");
    fs::create_dir_all(&home)?;
    let sandbox = Sandbox {
        home,
        writable: vec![directory.to_owned(), sockets.to_owned()],
        readable: clip.parent().map(Path::to_owned).into_iter().collect(),
        hidden: Vec::new(),
        own_processes: true,
    };
    let audio = SilentAudio::start(directory, sockets.join("audio"), &sandbox)?;
    warm(clip)?;
    // Before the rival starts: Aerith's testbed opens the clip at its start.
    let server = if options.stream {
        Some(ClipServer::start(clip.to_owned())?)
    } else {
        None
    };
    let target = server
        .as_ref()
        .map_or_else(|| clip.display().to_string(), ClipServer::url);
    eprintln!("starting {}", options.rival.name());
    let testbed = Testbed {
        clip: target.clone(),
        order: options.order.clone(),
        toggle_controls: options.toggle_controls,
        seconds: options.seconds + TESTBED_MARGIN,
    };
    let mut running = rivals::launch(
        options.rival,
        directory,
        &sockets.join("mpv.sock"),
        &sandbox,
        &audio.environment(),
        &options.mpv_options,
        &testbed,
    )?;
    let sampler = Sampler::start(running.child.id(), audio.pids(), sensor);
    let outcome = play(
        &mut running.engine,
        options,
        &target,
        directory,
        &sampler,
        running.wayland_log.as_deref(),
    );
    let logged_drops = running.engine.logged_drops();
    let logged_sync = running.engine.logged_sync();
    if options.rival == Rival::Aerith {
        // The testbed quits by itself, and prints its summary then; mpv quits with it.
        wait_exit(&mut running.child, TESTBED_EXIT);
    } else {
        running.engine.quit();
    }
    // Read the guard once more: a device opened after the last second's reading counts too.
    let late_sound_device = sampler.sound_device_opened();
    let samples = sampler.finish();
    rivals::stop_tree(running.child);
    drop(audio);
    let mut played = outcome?;
    played.sound_device = played.sound_device.or(late_sound_device);
    Ok(Measured {
        played,
        samples,
        logged_drops,
        logged_sync,
        wayland_log: running.wayland_log,
    })
}

/// A process of the run held a sound device open: the run is void, and the set must stop, for a
/// player may be playing aloud on the owner's machine. `main` gives it its own exit code.
#[derive(Debug)]
pub struct SoundDeviceOpened(pub String);

impl std::fmt::Display for SoundDeviceOpened {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "stopped: {}; nothing was measured", self.0)
    }
}

impl std::error::Error for SoundDeviceOpened {}

/// What playing the clip gave.
struct Played {
    first_frame: Option<Duration>,
    /// When the first frame came, in the sampler's seconds.
    first_frame_at: Option<f64>,
    seconds_played: Option<f64>,
    readings: Vec<Counters>,
    report: Value,
    sound_device: Option<String>,
    /// An error after the first frame: the run keeps what it measured until then.
    error: Option<String>,
    /// The part of the Wayland log written while the clip played: its length in bytes at the
    /// first frame and at the last reading.
    wayland_window: Option<(u64, u64)>,
    /// The files of the frames captured.
    captured: Vec<String>,
}

/// Loads the clip, or for Aerith waits for its testbed to open it, and reads the engine each
/// second until the run's seconds have played or the clip ends; or captures the frames asked.
fn play(
    engine: &mut crate::engine::Engine,
    options: &RunOptions,
    clip: &str,
    directory: &Path,
    sampler: &Sampler,
    wayland_log: Option<&Path>,
) -> io::Result<Played> {
    let seconds = options.seconds;
    let first_frame = if options.rival == Rival::Aerith {
        engine.wait_started()?
    } else {
        engine.load(clip)?
    };
    let playing_since = Instant::now();
    let log_length = || {
        wayland_log
            .and_then(|log| fs::metadata(log).ok())
            .map(|meta| meta.len())
    };
    let log_start = log_length();
    let mut played = Played {
        first_frame: Some(first_frame),
        first_frame_at: Some(sampler.started().elapsed().as_secs_f64()),
        seconds_played: None,
        readings: Vec::new(),
        report: Value::Null,
        sound_device: None,
        error: None,
        wayland_window: None,
        captured: Vec::new(),
    };
    if !options.frames.is_empty() {
        for at in &options.frames {
            let name = format!("frame-{at}.png");
            engine.capture(*at, &directory.join(&name))?;
            played.captured.push(name);
        }
        played.seconds_played = Some(playing_since.elapsed().as_secs_f64());
        return Ok(played);
    }
    // At once, for a clip shorter than REPORT_AFTER; again once it settled, below.
    played.report = engine
        .report()
        .unwrap_or_else(|error| json!({"error": error.to_string()}));
    let mut settled_report = false;
    eprintln!(
        "first frame after {:.1} ms; playing for up to {seconds} s",
        first_frame.as_secs_f64() * 1000.0
    );
    let mut next = playing_since;
    while playing_since.elapsed().as_secs_f64() < seconds {
        if let Some(device) = sampler.sound_device_opened() {
            played.sound_device = Some(device);
            break;
        }
        next += sampler::PERIOD;
        thread::sleep(next.saturating_duration_since(Instant::now()));
        match engine.counters() {
            Ok(counters) => {
                let ended = counters.ended;
                played.readings.push(counters);
                if ended {
                    break;
                }
            }
            Err(error) => {
                played.error = Some(format!("the engine stopped answering: {error}"));
                break;
            }
        }
        if !settled_report && playing_since.elapsed() >= REPORT_AFTER {
            settled_report = true;
            played.report = engine
                .report()
                .unwrap_or_else(|error| json!({"error": error.to_string()}));
        }
    }
    played.seconds_played = Some(playing_since.elapsed().as_secs_f64());
    played.wayland_window = log_start.zip(log_length());
    if played.report.is_null() && played.error.is_none() {
        played.report = engine
            .report()
            .unwrap_or_else(|error| json!({"error": error.to_string()}));
    }
    Ok(played)
}

/// The bytes of `log` between the two lengths of `window`, as text.
fn read_window(log: &Path, (start, end): (u64, u64)) -> io::Result<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(log)?;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(end.saturating_sub(start))
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Waits up to `timeout` for `child` to exit by itself.
fn wait_exit(child: &mut std::process::Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// What Aerith's testbed printed with `--stats` (its crate's documentation gives the format):
/// its summary, and between the line 2 s after the open and the last one, the wake-ups a second
/// of the window's thread and of the relay's, and the relay's count of what the compositor
/// presented, with feedback on every frame, its gaps against `ideal_gap`.
fn testbed_figures(stats: &str, ideal_gap: Option<f64>) -> Value {
    let lines: Vec<Value> = stats
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let summary = lines
        .iter()
        .rev()
        .find(|line| line["summary"] == true)
        .cloned();
    let periodic: Vec<&Value> = lines
        .iter()
        .filter(|line| line["summary"] != true)
        .collect();
    let since = |line: &Value| line["since_open"].as_f64();
    let first = periodic
        .iter()
        .find(|line| since(line).is_some_and(|at| at >= SETTLE));
    let last = periodic.last();
    let rate = |thread: &str| -> Value {
        let (Some(first), Some(last)) = (first, last) else {
            return Value::Null;
        };
        let span = since(last)
            .zip(since(first))
            .map(|(end, start)| end - start);
        let wakes = last[thread]["voluntary"]
            .as_f64()
            .zip(first[thread]["voluntary"].as_f64());
        match (span, wakes) {
            (Some(span), Some((end, start))) if span > 0.0 => {
                json!(figures::round((end - start) / span, 1))
            }
            _ => Value::Null,
        }
    };
    let relay = match (first, last) {
        (Some(first), Some(last)) => {
            relay_presentation(&first["presentation"], &last["presentation"], ideal_gap)
        }
        _ => Value::Null,
    };
    json!({
        "summary": summary,
        "last": last.map(|line| (*line).clone()),
        "wake_ups_per_second": {"main_thread": rate("main_thread"), "relay_thread": rate("relay_thread")},
        "relay_presentation": relay,
    })
}

/// The relay's `Presentation` between two of the testbed's lines: frames presented, scanned out,
/// discarded, and the refresh gaps, the longer ones counted as gaps of `COUNTED_GAPS`.
fn relay_presentation(first: &Value, last: &Value, ideal_gap: Option<f64>) -> Value {
    let between = |name: &str| {
        last[name]
            .as_u64()
            .zip(first[name].as_u64())
            .map(|(end, start)| end.saturating_sub(start))
    };
    let (Some(before), Some(after)) = (first["gaps"].as_array(), last["gaps"].as_array()) else {
        return Value::Null;
    };
    let mut gaps = std::collections::BTreeMap::new();
    for (length, (start, end)) in before.iter().zip(after).enumerate() {
        let count = end
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(start.as_u64().unwrap_or(0));
        if count > 0 {
            gaps.insert(length as u64, count);
        }
    }
    if let Some(longer) = between("longer_gaps").filter(|count| *count > 0) {
        gaps.insert(after.len() as u64, longer);
    }
    json!({
        "presented": between("presented"),
        "zero_copy": between("zero_copy"),
        "discarded": between("discarded"),
        "refresh_gaps": presentation::gap_figures(&gaps, ideal_gap),
    })
}

fn value_of(counters: &Counters, name: &str) -> Option<f64> {
    counters
        .values
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, value)| *value)
}

/// Reads the start of the clip once, so that the first frame's time does not depend on which rival
/// ran first.
fn warm(clip: &Path) -> io::Result<()> {
    let mut file = fs::File::open(clip)?.take(WARM_BYTES);
    io::copy(&mut file, &mut io::sink())?;
    Ok(())
}

/// The machine, as far as the system tells it: its model, its CPU and its kernel.
fn machine_description() -> Value {
    let read = |path: &str| {
        fs::read_to_string(path)
            .ok()
            .map(|text| text.trim().to_owned())
    };
    let cpu = read("/proc/cpuinfo").and_then(|info| {
        info.lines().find_map(|line| {
            line.strip_prefix("model name")
                .map(|rest| rest.trim_start_matches([' ', '\t', ':']).to_owned())
        })
    });
    json!({
        "model": read("/sys/class/dmi/id/product_name"),
        "cpu": cpu,
        "kernel": read("/proc/sys/kernel/osrelease"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_testbeds_figures_are_counted_from_two_seconds_after_the_open() {
        // A line of `--stats`: its threads' wake-ups, and the relay's frames presented with the
        // gaps of 9, 10 and 11 refreshes and those of 32 or more.
        let line = |at: f64, main: u64, relay: u64, presented: u64, gaps: [u64; 3], longer: u64| {
            let mut counts = vec![0_u64; 32];
            counts[9..12].copy_from_slice(&gaps);
            format!(
                "{{\"since_open\":{at},\"presentation\":{{\"committed\":{presented},\"presented\":{presented},\"discarded\":0,\"zero_copy\":{presented},\"gaps\":{counts:?},\"longer_gaps\":{longer}}},\"main_thread\":{{\"voluntary\":{main},\"involuntary\":0}},\"relay_thread\":{{\"voluntary\":{relay},\"involuntary\":0}}}}"
            )
        };
        let stats = [
            line(1.0, 10, 10, 20, [0, 19, 0], 0),
            line(2.0, 100, 200, 44, [1, 40, 2], 0),
            "first frame after 70 ms".to_owned(),
            line(4.0, 200, 500, 92, [2, 85, 3], 1),
            "{\"summary\":true,\"first_frame_ms\":70.1,\"since_open\":4.5}".to_owned(),
        ]
        .join("\n");
        let figures = testbed_figures(&stats, Some(10.0));
        assert_eq!(figures["wake_ups_per_second"]["main_thread"], 50.0);
        assert_eq!(figures["wake_ups_per_second"]["relay_thread"], 150.0);
        assert_eq!(figures["summary"]["first_frame_ms"], 70.1);
        assert_eq!(figures["last"]["since_open"], 4.0);
        let relay = &figures["relay_presentation"];
        assert_eq!(relay["presented"], 48);
        assert_eq!(relay["zero_copy"], 48);
        assert_eq!(
            relay["refresh_gaps"]["counts"],
            json!({"9": 1, "10": 45, "11": 1, "32": 1})
        );
        // Of 48 gaps, the ideal holds 48 of 10: the 9, the 11 and the 32 are off cadence.
        assert_eq!(relay["refresh_gaps"]["off_cadence"], 3);
        assert!(testbed_figures("", None)["wake_ups_per_second"]["main_thread"].is_null());
    }
}
