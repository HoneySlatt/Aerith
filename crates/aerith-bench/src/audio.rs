// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The bench's own sound server, silent (ADR-0013): a PipeWire instance of its own, whose only
//! output is a null sink paced by the system's clock, and a WirePlumber that links players to it
//! and is forbidden every device. Both run in the run's sandbox, where no sound device exists.
//! The players reach it, and only it, through the environment [`SilentAudio::environment`] gives
//! them; the owner's sound server and sound card are never touched. The sampler checks the last point at every second ([`crate::sampler`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::process;
use crate::sandbox::Sandbox;

/// The sink's name, which a run's report quotes.
pub const SINK: &str = "aerith-bench-sink";

/// Added to PipeWire's own configuration: one null sink, eight channels, so that a 7.1 stream is
/// not mixed down before the player has said what it sends.
const PIPEWIRE_FRAGMENT: &str = r#"context.objects = [
    { factory = adapter
      args = { factory.name = support.null-audio-sink node.name = aerith-bench-sink
               node.description = "Aerith bench, silent" media.class = Audio/Sink
               audio.position = [ FL FR FC LFE RL RR SL SR ] priority.session = 1000 } }
]
"#;

/// A WirePlumber profile with the policy that links streams to the sink, without any device:
/// no ALSA, no Bluetooth, no camera, no D-Bus, no state saved between runs.
const WIREPLUMBER_FRAGMENT: &str = r#"wireplumber.profiles = {
  aerith-bench = {
    inherits = [ base, mixin.systemwide-session, mixin.stateless ]
    metadata.sm-settings = required
    metadata.sm-objects = required
    policy.standard = required
    hardware.audio = disabled
    hardware.bluetooth = disabled
    hardware.video-capture = disabled
    support.dbus = disabled
  }
}
"#;

/// The running sound server, stopped when dropped, on every path: each process is owned here
/// as soon as it is started.
pub struct SilentAudio {
    runtime: PathBuf,
    pipewire: Option<Child>,
    wireplumber: Option<Child>,
}

impl SilentAudio {
    /// Starts the server with its files under `directory` and its sockets in `runtime`, a short
    /// path (a socket's path is limited to 108 bytes), and waits until WirePlumber has made its
    /// sink the default. Both programs run in `sandbox`, with the system's own PipeWire and
    /// WirePlumber configuration hidden: only their built-in defaults and this module's
    /// fragments apply.
    pub fn start(directory: &Path, runtime: PathBuf, sandbox: &Sandbox) -> io::Result<Self> {
        let config = directory.join("config");
        let state = directory.join("state");
        fs::create_dir_all(&runtime)?;
        fs::create_dir_all(config.join("pipewire/pipewire.conf.d"))?;
        fs::create_dir_all(config.join("wireplumber/wireplumber.conf.d"))?;
        fs::create_dir_all(&state)?;
        fs::write(
            config.join("pipewire/pipewire.conf.d/aerith-bench.conf"),
            PIPEWIRE_FRAGMENT,
        )?;
        fs::write(
            config.join("wireplumber/wireplumber.conf.d/aerith-bench.conf"),
            WIREPLUMBER_FRAGMENT,
        )?;
        let sandbox = Sandbox {
            hidden: vec![
                PathBuf::from("/etc/pipewire"),
                PathBuf::from("/etc/wireplumber"),
            ],
            home: sandbox.home.clone(),
            writable: sandbox.writable.clone(),
            readable: Vec::new(),
            own_processes: false,
        };
        let server = |program: &str, log: &str| -> io::Result<Command> {
            let mut command = sandbox.command(program);
            command
                .env("PIPEWIRE_RUNTIME_DIR", &runtime)
                .env("XDG_CONFIG_HOME", &config)
                .env("XDG_STATE_HOME", &state)
                // No session bus: neither server may reach the owner's desktop services.
                .env(
                    "DBUS_SESSION_BUS_ADDRESS",
                    format!("unix:path={}", runtime.join("no-bus").display()),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(fs::File::create(directory.join(log))?);
            Ok(command)
        };
        let mut audio = Self {
            runtime: runtime.clone(),
            pipewire: None,
            wireplumber: None,
        };
        audio.pipewire = Some(server("pipewire", "pipewire.log")?.spawn()?);
        wait_for(&runtime.join("pipewire-0"), Duration::from_secs(10))?;
        audio.wireplumber = Some(
            server("wireplumber", "wireplumber.log")?
                .args(["--profile", "aerith-bench"])
                .spawn()?,
        );
        audio.wait_for_default_sink(Duration::from_secs(10))?;
        Ok(audio)
    }

    /// What a player's environment needs to play into this server and nowhere else.
    pub fn environment(&self) -> Vec<(String, String)> {
        vec![
            (
                "PIPEWIRE_RUNTIME_DIR".into(),
                self.runtime.display().to_string(),
            ),
            // A PulseAudio client must not fall back on the owner's server.
            (
                "PULSE_SERVER".into(),
                format!("unix:{}", self.runtime.join("no-pulse").display()),
            ),
            // A JACK client must not start a JACK server of its own.
            ("JACK_NO_START_SERVER".into(), "1".into()),
        ]
    }

    /// The server's processes, whose sound devices the sampler watches.
    pub fn pids(&self) -> Vec<u32> {
        self.pipewire
            .iter()
            .chain(self.wireplumber.iter())
            .map(Child::id)
            .collect()
    }

    /// Waits until WirePlumber names the sink as the default in PipeWire's `default` metadata:
    /// PipeWire makes the sink by itself, so only this shows that WirePlumber runs and links
    /// players to it.
    fn wait_for_default_sink(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            // pw-dump waits for ever on a server that does not answer: bound it.
            let dump = Command::new("timeout")
                .args(["3", "pw-dump"])
                .env("PIPEWIRE_RUNTIME_DIR", &self.runtime)
                .env_remove("PIPEWIRE_REMOTE")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()?;
            if default_sink(&dump.stdout).as_deref() == Some(SINK) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "WirePlumber did not make the silent sink the default",
        ))
    }
}

/// The default sink's name in a `pw-dump` of the graph, if one is set.
fn default_sink(dump: &[u8]) -> Option<String> {
    let objects: Vec<Value> = serde_json::from_slice(dump).ok()?;
    objects
        .iter()
        .filter(|object| object["type"] == "PipeWire:Interface:Metadata")
        .filter(|object| object["props"]["metadata.name"] == "default")
        .flat_map(|object| object["metadata"].as_array().into_iter().flatten())
        .find(|entry| entry["key"] == "default.audio.sink")
        .and_then(|entry| entry["value"]["name"].as_str().map(str::to_owned))
}

impl Drop for SilentAudio {
    fn drop(&mut self) {
        process::stop(&self.pids(), Duration::from_secs(3));
        for child in self.wireplumber.iter_mut().chain(self.pipewire.iter_mut()) {
            let _ = child.wait();
        }
    }
}

/// Waits until `path` exists.
fn wait_for(path: &Path, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{} did not appear", path.display()),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_sink_is_read_from_the_default_metadata() {
        let dump = br#"[
            {"type": "PipeWire:Interface:Node", "props": {"node.name": "aerith-bench-sink"}},
            {"type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "settings"},
             "metadata": [{"key": "default.audio.sink", "value": {"name": "wrong"}}]},
            {"type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
             "metadata": [{"key": "default.audio.sink", "value": {"name": "aerith-bench-sink"}}]}
        ]"#;
        assert_eq!(default_sink(dump).as_deref(), Some(SINK));
        assert_eq!(
            default_sink(br#"[{"type": "PipeWire:Interface:Node"}]"#),
            None
        );
        assert_eq!(default_sink(b"not json"), None);
    }
}
