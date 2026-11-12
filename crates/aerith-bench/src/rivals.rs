// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The rivals the bench measures (the ROADMAP's references), each started as shipped, with a
//! fresh profile of its own under the run's folder, its defaults untouched, fullscreen, and its
//! sound sent to the bench's silent server.
//!
//! - **mpv** alone, with `--no-config` and the options that make it the image reference: gpu-next
//!   and safe hardware decoding.
//! - **Kodi**, driven over its JSON-RPC, which a fresh profile opens on `127.0.0.1:9090`.
//! - **Stremio** and **Jellyfin Desktop** embed libmpv. Each opens a page the bench writes, in place
//!   of its own interface; the page asks the embedded mpv to open an IPC socket
//!   (`set input-ipc-server`, which mpv allows at run time), and the bench then drives that mpv as it
//!   drives mpv alone. Their own options (hardware decoding, video sync) stay as the rival sets
//!   them, but the clip is loaded by the bench, not through their interface: the time to the first
//!   frame is their engine's, not their interface's.
//! - **Aerith** itself, through its test window, `aerith-testbed` ([ADR-0017]): the testbed opens
//!   the clip, as Aerith would, and plays it through `aerith-player` in its plane. The bench names
//!   the testbed's mpv: a wrapper of the bench's that runs the same mpv as mpv alone with the
//!   bench's IPC socket and log added after the player's options, so that the bench reads that
//!   mpv's counters as it reads mpv alone's, without driving it.
//!
//! mpv alone and Aerith's mpv log their Wayland messages (`WAYLAND_DEBUG=client`), from which
//! [`crate::presentation`] reads what the compositor did with each frame.
//!
//! [ADR-0017]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0017-a-test-interface-on-winit-and-wgpu.md

use std::fs;
use std::io;
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::engine::Engine;
use crate::process;
use crate::sandbox::Sandbox;

/// Kodi's JSON-RPC port in a fresh profile.
const KODI_PORT: u16 = 9090;
/// How long a rival may take to start, until its engine answers.
const START: Duration = Duration::from_secs(90);

/// One of the rivals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rival {
    /// mpv alone.
    Mpv,
    /// Kodi.
    Kodi,
    /// Stremio's Linux shell.
    Stremio,
    /// Jellyfin Desktop.
    Jellyfin,
    /// Aerith, through its test window.
    Aerith,
}

impl Rival {
    /// Every player the bench runs: the rivals, in the order the ROADMAP names them, then Aerith.
    pub const ALL: [Rival; 5] = [
        Rival::Mpv,
        Rival::Kodi,
        Rival::Stremio,
        Rival::Jellyfin,
        Rival::Aerith,
    ];

    /// The rival a command line names.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rival| rival.name() == name)
    }

    /// Its name on the command line and in the results.
    pub fn name(self) -> &'static str {
        match self {
            Rival::Mpv => "mpv",
            Rival::Kodi => "kodi",
            Rival::Stremio => "stremio",
            Rival::Jellyfin => "jellyfin",
            Rival::Aerith => "aerith",
        }
    }

    /// The program that starts it.
    pub fn program(self) -> &'static str {
        match self {
            Rival::Mpv => "mpv",
            Rival::Kodi => "kodi",
            Rival::Stremio => "stremio",
            Rival::Jellyfin => "jellyfin-desktop",
            Rival::Aerith => "aerith-testbed",
        }
    }
}

/// A rival that runs, and the engine the bench drives inside it.
pub struct Running {
    /// The rival's main process.
    pub child: Child,
    /// Its engine, connected.
    pub engine: Engine,
    /// The log of the engine's Wayland messages, for mpv alone and Aerith.
    pub wayland_log: Option<PathBuf>,
}

/// What Aerith's run asks of its testbed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Testbed {
    /// The clip, a path or a URL, which the testbed opens itself.
    pub clip: String,
    /// The order of the surfaces, `below` or `above`; the testbed's default when `None`.
    pub order: Option<String>,
    /// How many times the controls show and hide, from one second after the first frame.
    pub toggle_controls: Option<u32>,
    /// How long the testbed plays after its first frame before it quits by itself.
    pub seconds: f64,
}

/// Starts `rival` with its profile under `directory`, its environment extended by `environment`
/// (the silent sound server's), and connects to its engine; an mpv engine opens its IPC at
/// `socket`. `mpv_options` are added to mpv's command line, as `--name=value`, for mpv alone and
/// Aerith's mpv. Aerith's testbed plays as `testbed` says.
pub fn launch(
    rival: Rival,
    directory: &Path,
    socket: &Path,
    sandbox: &Sandbox,
    environment: &[(String, String)],
    mpv_options: &[String],
    testbed: &Testbed,
) -> io::Result<Running> {
    let mut command = player_command(rival, directory, sandbox, environment, "player")?;
    match rival {
        Rival::Mpv => {
            command.args([
                "--no-config",
                "--idle=yes",
                "--force-window=yes",
                "--fullscreen",
                "--vo=gpu-next",
                // Never mpv's DRM or direct-display contexts: without them, a failed Wayland
                // context ends the run instead of reaching for the screen the compositor holds.
                "--gpu-context=waylandvk,wayland",
                "--hwdec=auto-safe",
                "--ao=pipewire",
            ]);
            command.arg(format!("--input-ipc-server={}", socket.display()));
            command.arg(format!(
                "--log-file={}",
                directory.join("mpv.log").display()
            ));
            command.args(mpv_options.iter().map(|option| format!("--{option}")));
            command.env("WAYLAND_DEBUG", "client");
            let child = command.spawn()?;
            let log = directory.join("player.err");
            connected(child, Some(log), || Engine::connect_mpv(socket, START))
        }
        Rival::Aerith => {
            let mpv = process::resolve("mpv").ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "no mpv in PATH for Aerith to run")
            })?;
            let wrapper = directory.join("mpv-wrapper");
            let log = directory.join("mpv-wayland.log");
            fs::write(
                &wrapper,
                mpv_wrapper(&mpv, socket, &directory.join("mpv.log"), &log, mpv_options),
            )?;
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
            command
                .arg(&testbed.clip)
                .args(["--fullscreen", "--stats", "--sound", "--seconds"])
                .arg(testbed.seconds.to_string())
                .arg("--mpv")
                .arg(&wrapper);
            if let Some(order) = &testbed.order {
                command.args(["--order", order]);
            }
            if let Some(toggles) = testbed.toggle_controls {
                command.args(["--toggle-controls", &toggles.to_string()]);
            }
            let child = command.spawn()?;
            connected(child, Some(log), || Engine::connect_mpv(socket, START))
        }
        Rival::Kodi => {
            if TcpStream::connect(("127.0.0.1", KODI_PORT)).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("port {KODI_PORT} is taken: is a Kodi already running?"),
                ));
            }
            let temp = directory.join("profile/kodi-temp");
            command
                .env("KODI_DATA", directory.join("profile/kodi"))
                .env("KODI_TEMP", &temp)
                // Debug logging is what records the frames Kodi drops (Engine::logged_drops).
                .args([
                    "--windowing=wayland",
                    "--audio-backend=pipewire",
                    "-fs",
                    "--debug",
                ]);
            let child = command.spawn()?;
            connected(child, None, || {
                Engine::connect_kodi(KODI_PORT, temp.join("kodi.log"), START)
            })
        }
        Rival::Stremio => {
            let page = directory.join("stremio.html");
            fs::write(&page, stremio_page(socket))?;
            command
                .arg("--url")
                .arg(format!("file://{}", page.display()));
            let child = command.spawn()?;
            connected(child, None, || Engine::connect_mpv(socket, START))
        }
        Rival::Jellyfin => {
            let page = directory.join("jellyfin.html");
            fs::write(&page, jellyfin_page(socket))?;
            let config = directory.join("profile/jellyfin");
            prepare_jellyfin(&config, &page, sandbox, environment, directory)?;
            command.arg(format!("--config-dir={}", config.display()));
            let child = command.spawn()?;
            connected(child, None, || Engine::connect_mpv(socket, START))
        }
    }
}

/// The command that starts `rival` in the run's own profile under `directory`, its output in
/// `<log>.log` and `<log>.err` there.
fn player_command(
    rival: Rival,
    directory: &Path,
    sandbox: &Sandbox,
    environment: &[(String, String)],
    log: &str,
) -> io::Result<Command> {
    let profile = directory.join("profile");
    for folder in ["config", "data", "cache", "state"] {
        fs::create_dir_all(profile.join(folder))?;
    }
    // In the run's sandbox: no sound device, the run's home in place of the owner's, and every
    // helper the rival starts ended with it (crate::sandbox).
    let mut command = sandbox.command(rival.program());
    command
        .env("XDG_CONFIG_HOME", profile.join("config"))
        .env("XDG_DATA_HOME", profile.join("data"))
        .env("XDG_CACHE_HOME", profile.join("cache"))
        .env("XDG_STATE_HOME", profile.join("state"))
        // Away from the owner's session bus: no media keys taken, no notification, no portal.
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", directory.join("no-bus").display()),
        )
        // Wayland only: a toolkit must not fall back on X11, nor a player on the GPU's own display;
        // the sandbox removes DISPLAY.
        .env("GDK_BACKEND", "wayland")
        .env("QT_QPA_PLATFORM", "wayland")
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(fs::File::create(directory.join(format!("{log}.log")))?)
        .stderr(fs::File::create(directory.join(format!("{log}.err")))?);
    Ok(command)
}

/// The script Aerith's testbed runs as mpv: `mpv` with the player's arguments, then the bench's
/// IPC socket, its log and `mpv_options`, its Wayland messages appended to `wayland_log`.
fn mpv_wrapper(
    mpv: &Path,
    socket: &Path,
    mpv_log: &Path,
    wayland_log: &Path,
    mpv_options: &[String],
) -> String {
    let mut line = format!(
        "WAYLAND_DEBUG=client exec {} \"$@\" {} {}",
        shell_quoted(&mpv.display().to_string()),
        shell_quoted(&format!("--input-ipc-server={}", socket.display())),
        shell_quoted(&format!("--log-file={}", mpv_log.display())),
    );
    for option in mpv_options {
        line.push(' ');
        line.push_str(&shell_quoted(&format!("--{option}")));
    }
    line.push_str(&format!(
        " 2>>{}",
        shell_quoted(&wayland_log.display().to_string())
    ));
    format!("#!/bin/sh\n# Written by aerith-bench: Aerith's mpv, observed by the bench.\n{line}\n")
}

/// `text` as one word of a POSIX shell.
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Connects to the engine of a rival just started; stops the rival if that fails.
fn connected(
    child: Child,
    wayland_log: Option<PathBuf>,
    connect: impl FnOnce() -> io::Result<Engine>,
) -> io::Result<Running> {
    match connect() {
        Ok(engine) => Ok(Running {
            child,
            engine,
            wayland_log,
        }),
        Err(error) => {
            stop_tree(child);
            Err(error)
        }
    }
}

/// Stops a rival and all it started.
pub fn stop_tree(mut child: Child) {
    let tree = process::tree(child.id());
    process::stop(&tree, Duration::from_secs(10));
    let _ = child.wait();
}

/// The page Stremio opens: its shell injects `window.ipc` into every page, through which the page
/// sends mpv commands, as Stremio's own interface does (`type` 6, an `mpv-command`).
fn stremio_page(socket: &Path) -> String {
    page(&format!(
        r#"// The shell's script forwards its messages to this transport; nothing here listens.
window.qt.webChannelTransport.onmessage = () => {{}};
const send = (args) => window.ipc.postMessage(JSON.stringify({{ type: 6, args }}));
const start = () => {{
    if (!window.ipc) return setTimeout(start, 100);
    send(["win-set-visibility", {{ fullscreen: true }}]);
    send(["mpv-command", ["set", "input-ipc-server", {socket}]]);
}};
start();"#,
        socket = Value::from(socket.display().to_string())
    ))
}

/// The page Jellyfin Desktop opens: its shell injects `window.api` into every page, whose
/// `player.userCommand` runs an mpv command line, and whose `player.setAudioStream` chooses the
/// sound track the shell will select. The path is quoted for mpv's parser: in double
/// quotes, with `\\` and `"` escaped by a backslash.
fn jellyfin_page(socket: &Path) -> String {
    let path = socket
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let command = format!("set input-ipc-server \"{path}\"");
    page(&format!(
        r#"const start = () => {{
    if (!window.api || !window.api.player) return setTimeout(start, 100);
    // The shell sets mpv's tracks itself once a file is loaded, from what its web client chose;
    // with nothing chosen it turns the sound off. Choose the first sound track and no subtitle,
    // as its web client does by default.
    window.api.player.setAudioStream(1);
    window.api.player.setSubtitleStream(-1);
    window.api.player.userCommand({command});
}};
start();"#,
        command = Value::from(command)
    ))
}

/// A page with a transparent background, so that the video drawn under the web view shows.
fn page(script: &str) -> String {
    format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>Aerith bench</title>\n\
         <style>html, body {{ background: transparent; margin: 0; }}</style>\n\
         <script>\n{script}\n</script>\n"
    )
}

/// Jellyfin Desktop writes its settings, with their version, on its first start: the bench lets it
/// start once, then points its start page at the bench's page and asks for fullscreen.
fn prepare_jellyfin(
    config: &Path,
    page: &Path,
    sandbox: &Sandbox,
    environment: &[(String, String)],
    directory: &Path,
) -> io::Result<()> {
    let settings = find_file(config, "jellyfin-desktop.conf");
    let settings = match settings {
        Some(found) => found,
        None => {
            let mut first = player_command(
                Rival::Jellyfin,
                directory,
                sandbox,
                environment,
                "jellyfin-first-start",
            )?;
            first.arg(format!("--config-dir={}", config.display()));
            let child = first.spawn()?;
            let deadline = Instant::now() + START;
            let found = loop {
                if let Some(found) = find_file(config, "jellyfin-desktop.conf") {
                    break Some(found);
                }
                if Instant::now() >= deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(250));
            };
            // Let it finish writing before it is stopped.
            thread::sleep(Duration::from_secs(2));
            stop_tree(child);
            found.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Jellyfin Desktop wrote no settings",
                )
            })?
        }
    };
    let mut values: Value = serde_json::from_str(&fs::read_to_string(&settings)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    // serde_json's indexing panics on a value that is not an object: check the shape first.
    let sections = values
        .get_mut("sections")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Jellyfin's settings have no sections",
            )
        })?;
    for (section, key, value) in [
        (
            "path",
            "startupurl_desktop",
            json!(format!("file://{}", page.display())),
        ),
        ("main", "fullscreen", json!(true)),
    ] {
        let entry = sections.entry(section).or_insert_with(|| json!({}));
        let entry = entry.as_object_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Jellyfin's {section} settings are not an object"),
            )
        })?;
        entry.insert(key.to_owned(), value);
    }
    let partial = settings.with_extension("conf.partial");
    fs::write(
        &partial,
        serde_json::to_vec_pretty(&values).map_err(io::Error::other)?,
    )?;
    fs::rename(partial, settings)
}

/// The first file named `name` under `directory`, searched depth first.
fn find_file(directory: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|file| file == name) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapper_runs_mpv_with_every_argument_intact_and_the_benchs_after() {
        let root =
            std::env::temp_dir().join(format!("aerith-bench-wrapper-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mpv = root.join("fake mpv");
        fs::write(
            &mpv,
            "#!/bin/sh\nprintf '%s\\n' \"$@\"\necho \"$WAYLAND_DEBUG\" >&2\n",
        )
        .unwrap();
        fs::set_permissions(&mpv, fs::Permissions::from_mode(0o755)).unwrap();
        let wrapper = root.join("wrapper");
        let log = root.join("it's wayland.log");
        fs::write(
            &wrapper,
            mpv_wrapper(
                &mpv,
                Path::new("/tmp/a socket"),
                Path::new("/tmp/mpv.log"),
                &log,
                &["video-sync=display-resample".to_owned()],
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(&wrapper)
            .args(["--title=it's \"Aerith\"", "two words"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .collect::<Vec<_>>(),
            [
                "--title=it's \"Aerith\"",
                "two words",
                "--input-ipc-server=/tmp/a socket",
                "--log-file=/tmp/mpv.log",
                "--video-sync=display-resample",
            ]
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "client\n",
            "the Wayland log"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rivals_are_named_both_ways() {
        for rival in Rival::ALL {
            assert_eq!(Rival::from_name(rival.name()), Some(rival));
        }
        assert_eq!(Rival::from_name("vlc"), None);
    }

    #[test]
    fn a_page_quotes_the_socket_as_a_javascript_string() {
        // mpv must read the path /run/a "b"/mpv.sock: quoted for mpv, then for JavaScript.
        let page = jellyfin_page(Path::new("/run/a \"b\"/mpv.sock"));
        assert!(
            page.contains(r#"userCommand("set input-ipc-server \"/run/a \\\"b\\\"/mpv.sock\"")"#),
            "{page}"
        );
        let page = stremio_page(Path::new("/run/x/mpv.sock"));
        assert!(
            page.contains(r#"["set", "input-ipc-server", "/run/x/mpv.sock"]"#),
            "{page}"
        );
    }

    #[test]
    fn find_file_searches_folders_below() {
        let root = std::env::temp_dir().join(format!("aerith-bench-find-{}", std::process::id()));
        fs::create_dir_all(root.join("profiles/abc")).unwrap();
        fs::write(root.join("profiles/abc/jellyfin-desktop.conf"), "{}").unwrap();
        assert_eq!(
            find_file(&root, "jellyfin-desktop.conf"),
            Some(root.join("profiles/abc/jellyfin-desktop.conf"))
        );
        assert_eq!(find_file(&root, "missing"), None);
        fs::remove_dir_all(root).unwrap();
    }
}
