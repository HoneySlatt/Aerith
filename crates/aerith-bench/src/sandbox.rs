// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The namespace every program of a run starts in, rivals and sound servers alike (ADR-0013),
//! through bubblewrap:
//!
//! - the sound devices do not exist: `/dev/snd` is empty, so nothing can open a card, not even to
//!   read its name;
//! - the owner's home is replaced by the run's own: anything a program writes under `$HOME` lands
//!   in the run's folder. The clip's folder and the run's folder are bound back, the clip's read
//!   only;
//! - a rival has its own process numbers: when its first process ends, or when the bench dies,
//!   the kernel ends every process in it, the helpers it started included. The sound server
//!   cannot: PipeWire checks each client by its process, which it must see; its two programs
//!   start no helper, and each dies with the bench.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a run's sandbox lets through.
pub struct Sandbox {
    /// The folder that stands for the owner's home inside.
    pub home: PathBuf,
    /// Folders the programs must reach as they are: the run's folder, read and written.
    pub writable: Vec<PathBuf>,
    /// Folders the programs read: the clip's.
    pub readable: Vec<PathBuf>,
    /// Folders hidden behind an empty one: the system's own sound configuration, for the bench's
    /// sound server.
    pub hidden: Vec<PathBuf>,
    /// Whether the programs get process numbers of their own (see the module's documentation).
    pub own_processes: bool,
}

impl Sandbox {
    /// The command that runs `program` inside the sandbox.
    pub fn command(&self, program: &str) -> Command {
        let mut command = Command::new("bwrap");
        command.args(self.arguments()).arg("--").arg(program);
        // Inside, the owner's home path shows the run's home; HOME keeps its value.
        command.env_remove("PIPEWIRE_REMOTE").env_remove("DISPLAY");
        command
    }

    fn arguments(&self) -> Vec<String> {
        let mut arguments: Vec<String> = ["--dev-bind", "/", "/"].map(str::to_owned).to_vec();
        if let Some(owner_home) = std::env::var_os("HOME").map(PathBuf::from) {
            push(&mut arguments, "--bind", &self.home, &owner_home);
        }
        for folder in &self.writable {
            push(&mut arguments, "--bind", folder, folder);
        }
        for folder in &self.readable {
            push(&mut arguments, "--ro-bind", folder, folder);
        }
        for folder in &self.hidden {
            // bwrap mounts on a folder, not a link: NixOS's /etc folders are links into the store.
            if let Ok(folder) = std::fs::canonicalize(folder) {
                arguments.extend(["--tmpfs".to_owned(), folder.display().to_string()]);
            }
        }
        // A machine without a sound card has no /dev/snd: nothing to hide, nowhere to mount.
        if Path::new("/dev/snd").exists() {
            arguments.extend(["--tmpfs", "/dev/snd"].map(str::to_owned));
        }
        arguments.push("--die-with-parent".to_owned());
        if self.own_processes {
            arguments.extend(["--unshare-pid", "--proc", "/proc"].map(str::to_owned));
        }
        arguments
    }
}

fn push(arguments: &mut Vec<String>, kind: &str, source: &Path, target: &Path) {
    arguments.extend([
        kind.to_owned(),
        source.display().to_string(),
        target.display().to_string(),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sandbox(root: &Path) -> Sandbox {
        Sandbox {
            home: root.join("home"),
            writable: vec![root.join("run")],
            readable: vec![root.join("corpus")],
            hidden: vec![],
            own_processes: true,
        }
    }

    #[test]
    fn the_owners_home_and_the_sound_devices_are_out_of_reach() {
        let root =
            std::env::temp_dir().join(format!("aerith-bench-sandbox-{}", std::process::id()));
        for folder in ["home", "run", "corpus"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        fs::write(root.join("corpus/clip"), "clip\n").unwrap();
        let home = std::env::var("HOME").unwrap();
        let script = format!(
            "touch \"$HOME/.aerith-bench-sandbox-probe\" && ls -A /dev/snd 2>/dev/null | wc -l && \
             cat {corpus}/clip && touch {run}/written && ! touch {corpus}/denied 2>/dev/null && echo $$",
            corpus = root.join("corpus").display(),
            run = root.join("run").display(),
        );
        let output = sandbox(&root)
            .command("sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{text} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "0", "no sound device inside");
        assert_eq!(lines[1], "clip", "the clip is readable");
        assert_eq!(
            lines[2], "2",
            "its own process numbers: sh is the second, after bwrap's"
        );
        assert!(
            root.join("home/.aerith-bench-sandbox-probe").exists(),
            "HOME is the run's home"
        );
        assert!(
            !Path::new(&home)
                .join(".aerith-bench-sandbox-probe")
                .exists(),
            "not the owner's"
        );
        assert!(root.join("run/written").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_session_display_and_sound_server_variables_are_removed() {
        let command = sandbox(Path::new("/x")).command("true");
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert!(removed.contains(&"PIPEWIRE_REMOTE".to_owned()));
        assert!(removed.contains(&"DISPLAY".to_owned()));
    }
}
