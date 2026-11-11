// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Samples, once a second, what a player costs the machine: the memory and CPU time of its whole
//! tree, and the GPU's power. It also watches that no process of the run opens a sound device: the
//! bench plays into its own silent sound server, and a player that reached the sound card would
//! play aloud on the owner's machine.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::process;

/// How often the sampler reads. Once a second keeps its own cost, and the wake-ups it causes in
/// the player, negligible.
pub const PERIOD: Duration = Duration::from_secs(1);

/// What one reading found.
#[derive(Clone, Debug)]
pub struct Sample {
    /// Seconds since the sampler started.
    pub at: f64,
    /// The player's tree's proportional set size, in KiB.
    pub pss_kib: u64,
    /// The CPU time the tree has used since it started, in ticks.
    pub cpu_ticks: u64,
    /// The GPU's power, in microwatts, when the machine reports it.
    pub gpu_microwatts: Option<u64>,
    /// How many processes the tree held.
    pub processes: usize,
    /// The CPU time the whole machine has used since boot, in ticks, every process included:
    /// what the rival's tree did not use shows what else ran (another build, another program).
    pub machine_busy_ticks: Option<u64>,
}

/// The busy time of every CPU since boot, from the first line of `/proc/stat`: user, nice, system,
/// irq, softirq and steal ticks; idle and iowait are left out.
pub fn machine_busy_ticks() -> Option<u64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let fields: Vec<u64> = stat
        .lines()
        .next()?
        .strip_prefix("cpu ")?
        .split_whitespace()
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    let [user, nice, system, _idle, _iowait, irq, softirq, steal, ..] = fields[..] else {
        return None;
    };
    Some(user + nice + system + irq + softirq + steal)
}

/// The GPU's power sensor: amdgpu reports `power1_average` on discrete GPUs and
/// `power1_input` on some APUs, both in microwatts.
pub fn gpu_power_sensor() -> Option<PathBuf> {
    let cards = fs::read_dir("/sys/class/drm").ok()?;
    let mut sensors: Vec<PathBuf> = cards
        .flatten()
        .map(|card| card.path().join("device/hwmon"))
        .filter_map(|hwmon| fs::read_dir(hwmon).ok())
        .flat_map(|monitors| monitors.flatten().map(|monitor| monitor.path()))
        .flat_map(|monitor| [monitor.join("power1_average"), monitor.join("power1_input")])
        .filter(|sensor| read_number(sensor).is_some())
        .collect();
    sensors.sort();
    sensors.dedup();
    sensors.into_iter().next()
}

fn read_number(path: &PathBuf) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// A sampler running on its own thread.
pub struct Sampler {
    started: Instant,
    stop: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<Sample>>>,
    sound_device: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Sampler {
    /// Starts sampling the tree of `player`; `guarded` are other processes of the run (the sound
    /// server) whose sound devices are watched too.
    pub fn start(player: u32, guarded: Vec<u32>, sensor: Option<PathBuf>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let sound_device = Arc::new(Mutex::new(None));
        let started = Instant::now();
        let thread = {
            let (stop, samples, sound_device) =
                (stop.clone(), samples.clone(), sound_device.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let tree = process::tree(player);
                    let watched = tree.iter().chain(guarded.iter());
                    if let Some((pid, device)) = watched
                        .flat_map(|pid| {
                            process::open_sound_devices(*pid)
                                .into_iter()
                                .map(move |d| (*pid, d))
                        })
                        .next()
                    {
                        *sound_device.lock().unwrap() =
                            Some(format!("process {pid} opened {}", device.display()));
                    }
                    let sample = Sample {
                        at: started.elapsed().as_secs_f64(),
                        pss_kib: tree.iter().filter_map(|pid| process::pss_kib(*pid)).sum(),
                        cpu_ticks: tree.iter().filter_map(|pid| process::cpu_ticks(*pid)).sum(),
                        gpu_microwatts: sensor.as_ref().and_then(read_number),
                        processes: tree.len(),
                        machine_busy_ticks: machine_busy_ticks(),
                    };
                    samples.lock().unwrap().push(sample);
                    thread::sleep(PERIOD);
                }
            })
        };
        Self {
            started,
            stop,
            samples,
            sound_device,
            thread: Some(thread),
        }
    }

    /// When sampling started: a sample's `at` counts from here.
    pub fn started(&self) -> Instant {
        self.started
    }

    /// A process of the run that opened a sound device, if one did.
    pub fn sound_device_opened(&self) -> Option<String> {
        self.sound_device.lock().unwrap().clone()
    }

    /// Stops the thread and returns every sample, oldest first.
    pub fn finish(mut self) -> Vec<Sample> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.samples.lock().unwrap().clone()
    }
}

/// Reads the GPU's power for `duration` while the bench does nothing: the baseline that the
/// power while playing is compared with.
pub fn idle_gpu_microwatts(sensor: &PathBuf, duration: Duration) -> Option<f64> {
    let deadline = Instant::now() + duration;
    let mut readings = Vec::new();
    while Instant::now() < deadline {
        if let Some(reading) = read_number(sensor) {
            readings.push(reading as f64);
        }
        thread::sleep(PERIOD);
    }
    (!readings.is_empty()).then(|| readings.iter().sum::<f64>() / readings.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn the_sampler_reads_a_running_tree() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let sampler = Sampler::start(child.id(), Vec::new(), None);
        thread::sleep(Duration::from_millis(1500));
        assert!(sampler.sound_device_opened().is_none());
        let samples = sampler.finish();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(samples.len() >= 2, "{samples:?}");
        assert!(
            samples
                .iter()
                .all(|sample| sample.processes == 1 && sample.pss_kib > 0)
        );
    }
}
