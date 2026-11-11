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

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::audio::{self, SilentAudio};
use crate::engine::Counters;
use crate::figures;
use crate::process;
use crate::rivals::{self, Rival};
use crate::sampler::{self, Sampler};
use crate::sandbox::Sandbox;
use crate::serve::ClipServer;

/// The version of `result.json`'s layout; a change to its fields raises it.
pub const RESULT_FORMAT: u32 = 1;
/// When the engine is asked what it decodes and outputs, after the first frame: once it settled.
const REPORT_AFTER: Duration = Duration::from_secs(2);
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
    /// Options added to mpv alone's command line, as `name=value`.
    pub mpv_options: Vec<String>,
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
    let result = json!({
        "format": RESULT_FORMAT,
        "date": figures::iso_utc(started_at),
        "rival": options.rival.name(),
        "binary": binary.map(|path| path.display().to_string()),
        "mpv_options": options.mpv_options,
        "clip": clip_name,
        "clip_path": clip.display().to_string(),
        "source": if options.stream { "HTTP on 127.0.0.1, served by the bench" } else { "local file" },
        "machine": machine_description(),
        "display": options.display,
        "audio": format!("the bench's silent PipeWire, sink {}", audio::SINK),
        "seconds_asked": options.seconds,
        "seconds_played": played.seconds_played.map(|s| figures::round(s, 1)),
        "first_frame_ms": played.first_frame.map(|d| figures::round(d.as_secs_f64() * 1000.0, 1)),
        "first_frame_marker": if options.rival == Rival::Kodi {
            "Kodi's Player.OnAVStart notification after Player.Open"
        } else {
            "mpv's playback-restart event after loadfile"
        },
        "frames": frames,
        "sync": figures::sync_figures(logged_sync.as_deref().unwrap_or(&avsync)),
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
    eprintln!("starting {}", options.rival.name());
    let mut running = rivals::launch(
        options.rival,
        directory,
        &sockets.join("mpv.sock"),
        &sandbox,
        &audio.environment(),
        &options.mpv_options,
    )?;
    let sampler = Sampler::start(running.child.id(), audio.pids(), sensor);
    let server = if options.stream {
        Some(ClipServer::start(clip.to_owned()))
    } else {
        None
    };
    let outcome = match server.transpose() {
        Ok(server) => {
            let target = server
                .as_ref()
                .map_or_else(|| clip.display().to_string(), ClipServer::url);
            play(&mut running.engine, &target, options.seconds, &sampler)
        }
        Err(error) => Err(error),
    };
    let logged_drops = running.engine.logged_drops();
    let logged_sync = running.engine.logged_sync();
    running.engine.quit();
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
}

/// Loads the clip and reads the engine each second until `seconds` have played or the clip ends.
fn play(
    engine: &mut crate::engine::Engine,
    clip: &str,
    seconds: f64,
    sampler: &Sampler,
) -> io::Result<Played> {
    let first_frame = engine.load(clip)?;
    let playing_since = Instant::now();
    let mut played = Played {
        first_frame: Some(first_frame),
        first_frame_at: Some(sampler.started().elapsed().as_secs_f64()),
        seconds_played: None,
        readings: Vec::new(),
        report: Value::Null,
        sound_device: None,
        error: None,
    };
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
    if played.report.is_null() && played.error.is_none() {
        played.report = engine
            .report()
            .unwrap_or_else(|error| json!({"error": error.to_string()}));
    }
    Ok(played)
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
