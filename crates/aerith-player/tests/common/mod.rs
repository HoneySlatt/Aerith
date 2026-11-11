// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the tests against the real mpv share: a folder per test, mpv in a sandbox, clips made by
//! mpv itself, a stream server that records the headers it receives, and waiting for events.
//!
//! mpv is the one `AERITH_TEST_MPV` names, which the development shell sets (`flake.nix`). Each
//! test runs it through bubblewrap, as the bench runs its rivals (ADR-0013): the person's home is
//! replaced by the test's; the sound devices and the session's runtime folders (its sound server,
//! its compositor, its bus) are hidden, and the variables that name them removed. Each test then checks that mpv wrote nothing in that home.

#![allow(
    dead_code,
    reason = "each test binary uses its own part of these helpers"
)]

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use aerith_player::{Audio, Event, Settings, Video};

/// How long the tests wait for an event before they fail.
pub const WAIT: Duration = Duration::from_secs(20);

/// A test's own folder under the system's temporary folder, removed at the end.
pub struct TestDir {
    pub root: PathBuf,
}

impl TestDir {
    pub fn new(name: &str) -> Self {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "aerith-player-{name}-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        for folder in ["home", "cache", "clips"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        Self { root }
    }

    /// The sandboxed mpv's command: a script that starts the real one through bubblewrap, `readable`
    /// bound back read-only over the replaced home.
    pub fn mpv(&self, readable: &[&Path]) -> PathBuf {
        let mpv = std::env::var("AERITH_TEST_MPV")
            .expect("AERITH_TEST_MPV names the mpv to test; run inside `nix develop`");
        let home = std::env::var("HOME").expect("HOME is set");
        let mut arguments = vec![
            "--dev-bind".to_owned(),
            "/".to_owned(),
            "/".to_owned(),
            "--bind".to_owned(),
            self.root.join("home").display().to_string(),
            home,
        ];
        for folder in readable {
            let folder = folder.display().to_string();
            arguments.extend(["--ro-bind".to_owned(), folder.clone(), folder]);
        }
        // A machine without a sound card has no /dev/snd: nothing to hide there.
        if Path::new("/dev/snd").exists() {
            arguments.extend(["--tmpfs".to_owned(), "/dev/snd".to_owned()]);
        }
        // The session's runtime folders, where its compositor, sound server and bus listen: all of
        // /run/user, since an agent's own runtime folder is only a subfolder of the person's.
        let mut runtime_folders = vec![PathBuf::from("/run/user")];
        runtime_folders.extend(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from));
        for folder in runtime_folders {
            if folder.exists()
                && (folder == Path::new("/run/user") || !folder.starts_with("/run/user"))
            {
                arguments.extend(["--tmpfs".to_owned(), folder.display().to_string()]);
            }
        }
        for variable in [
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "PIPEWIRE_REMOTE",
            "PULSE_SERVER",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            arguments.extend(["--unsetenv".to_owned(), variable.to_owned()]);
        }
        arguments.extend(["--die-with-parent", "--", &mpv].map(str::to_owned));
        let line: Vec<String> = arguments.iter().map(|a| quoted(a)).collect();
        let draft = self.root.join("mpv.draft");
        fs::write(
            &draft,
            format!("#!/bin/sh\nexec bwrap {} \"$@\"\n", line.join(" ")),
        )
        .unwrap();
        // Copied by another program: a file this process opens for writing could be inherited,
        // for an instant, by a process another test forks, and starting it would then fail as
        // busy (ETXTBSY).
        let script = self.root.join("mpv");
        let copied = Command::new("install")
            .args(["-m", "755"])
            .arg(&draft)
            .arg(&script)
            .status()
            .unwrap();
        assert!(copied.success());
        script
    }

    /// Settings that discard the picture and the sound, with the sandboxed mpv.
    pub fn settings(&self, readable: &[&Path]) -> Settings {
        Settings {
            engine: self.mpv(readable),
            video: Video::Discarded,
            audio: Audio::Discarded,
            cache: self.root.join("cache"),
        }
    }

    /// A clip of `seconds`, a test picture at 24 frames a second and a tone, made by mpv's own
    /// encoder, so that the tests need no other program: FFV1 and FLAC in Matroska.
    pub fn clip(&self, seconds: u32) -> PathBuf {
        let clip = self.root.join(format!("clips/{seconds}s.mkv"));
        // FFmpeg's filters as sources. Written apart from their arguments: the link check reads
        // the whole string as a URL whose port is malformed.
        let lavfi = "av://lavfi";
        let status = Command::new(self.mpv(&[]))
            .args([
                "--no-config",
                "--terminal=no",
                &format!("{lavfi}:testsrc2=size=320x240:rate=24:duration={seconds}"),
                &format!("--audio-file={lavfi}:sine=frequency=440:duration={seconds}"),
                &format!("--o={}", clip.display()),
                "--of=matroska",
                "--ovc=ffv1",
                "--oac=flac",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "mpv made the clip");
        clip
    }

    /// Fails if anything was written in the home that stood for the person's. `readable` are the
    /// folders given to [`Self::mpv`]: bubblewrap makes their mount points, empty, in that home.
    pub fn assert_home_untouched(&self, readable: &[&Path]) {
        let home = self.root.join("home");
        let person = PathBuf::from(std::env::var("HOME").unwrap());
        let mount_points: Vec<PathBuf> = readable
            .iter()
            .filter_map(|folder| folder.strip_prefix(&person).ok())
            .map(|inside| home.join(inside))
            .collect();
        let mut written = Vec::new();
        let mut folders = vec![home.clone()];
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if mount_points.iter().any(|mount| mount.starts_with(&path)) {
                    folders.push(path);
                } else {
                    written.push(path);
                }
            }
        }
        assert!(written.is_empty(), "mpv wrote in the home: {written:?}");
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// `text` quoted for the shell.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Waits for the first event `wanted` accepts, and returns it. Fails on a failure of the engine,
/// or after [`WAIT`].
pub fn wait_for(events: &Receiver<Event>, wanted: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(event) if wanted(&event) => return event,
            Ok(Event::EngineFailed(reason)) => panic!("the engine failed: {reason}"),
            Ok(_) => {}
            Err(error) => panic!("no event awaited within {WAIT:?}: {error}"),
        }
    }
}

/// Waits for the engine's failure, and returns its reason. Fails after [`WAIT`].
pub fn engine_failure(events: &Receiver<Event>) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(Event::EngineFailed(reason)) => return reason,
            Ok(_) => {}
            Err(error) => panic!("no failure reported within {WAIT:?}: {error}"),
        }
    }
}

/// Every process descended from this test process, by pid.
pub fn descendants() -> Vec<u32> {
    let mut parents = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        // After the command's name, which may hold spaces: the state, then the parent.
        let after_name = &stat[stat.rfind(')').unwrap() + 1..];
        if let Some(parent) = after_name
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse::<u32>().ok())
        {
            parents.push((pid, parent));
        }
    }
    let mut found = BTreeSet::from([std::process::id()]);
    loop {
        let before = found.len();
        for (pid, parent) in &parents {
            if found.contains(parent) {
                found.insert(*pid);
            }
        }
        if found.len() == before {
            break;
        }
    }
    found.remove(&std::process::id());
    found.into_iter().collect()
}

/// The mpv processes of this test: descended from the bubblewrap whose command line names `dir`.
pub fn mpv_of(dir: &TestDir) -> Vec<u32> {
    let all = descendants();
    let marker = dir.root.display().to_string();
    let ours: Vec<u32> = all
        .iter()
        .copied()
        .filter(|pid| command_line(*pid).contains(&marker))
        .collect();
    all.into_iter()
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "mpv")
        })
        .filter(|pid| ours.iter().any(|sandbox| is_descendant(*pid, *sandbox)))
        .collect()
}

fn is_descendant(pid: u32, ancestor: u32) -> bool {
    let mut current = pid;
    for _ in 0..64 {
        let Ok(stat) = fs::read_to_string(format!("/proc/{current}/stat")) else {
            return false;
        };
        let after_name = &stat[stat.rfind(')').unwrap() + 1..];
        let Some(parent) = after_name
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse().ok())
        else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        if parent <= 1 {
            return false;
        }
        current = parent;
    }
    false
}

/// A process's command line, its arguments joined by spaces; empty once it has exited.
pub fn command_line(pid: u32) -> String {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .unwrap_or_default()
}

/// A process's environment, as `NAME=value` lines; empty once it has exited.
pub fn environment(pid: u32) -> String {
    fs::read(format!("/proc/{pid}/environ"))
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', "\n"))
        .unwrap_or_default()
}

/// Serves one clip over HTTP on 127.0.0.1, with range requests, as a Jellyfin server or an addon's
/// would, and records each request's head.
pub struct ClipServer {
    address: SocketAddr,
    heads: Arc<Mutex<Vec<String>>>,
}

impl ClipServer {
    pub fn start(clip: PathBuf) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&heads);
        thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                let (clip, recorded) = (clip.clone(), Arc::clone(&recorded));
                // A player closes connections it no longer needs: that is not an error.
                thread::spawn(move || {
                    let _ = serve(connection, &clip, &recorded);
                });
            }
        });
        Self { address, heads }
    }

    /// The clip's URL, followed by `query`.
    pub fn url(&self, query: &str) -> String {
        format!("http://{}/clip.mkv{query}", self.address)
    }

    /// The heads of the requests received so far.
    pub fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
}

fn serve(connection: TcpStream, clip: &Path, heads: &Mutex<Vec<String>>) -> io::Result<()> {
    let length = clip.metadata()?.len();
    let mut reader = BufReader::new(connection.try_clone()?);
    let mut writer = connection;
    loop {
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if (&mut reader).take(16 << 10).read_line(&mut line)? == 0 {
                return Ok(());
            }
            if line.trim().is_empty() {
                break;
            }
            head.push_str(&line);
        }
        heads.lock().unwrap().push(head.clone());
        let range = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("range"))
            .and_then(|(_, value)| value.trim().strip_prefix("bytes=")?.split_once('-'))
            .and_then(|(start, _)| start.parse::<u64>().ok())
            .filter(|start| *start < length);
        let start = range.unwrap_or(0);
        if range.is_some() {
            write!(
                writer,
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{length}\r\n",
                length - 1
            )?;
        } else {
            writer.write_all(b"HTTP/1.1 200 OK\r\n")?;
        }
        write!(
            writer,
            "Content-Type: video/x-matroska\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\n\r\n",
            length - start
        )?;
        if head.starts_with("GET ") {
            let mut file = File::open(clip)?;
            file.seek(SeekFrom::Start(start))?;
            io::copy(&mut file, &mut writer)?;
        }
    }
}
