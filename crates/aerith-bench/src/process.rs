// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the bench reads of running processes in `/proc`: a player's whole tree (Stremio and
//! Jellyfin run helpers: a web engine, a streaming server), its memory and CPU time, and whether
//! any of them holds the sound card open. And how a tree is stopped.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// Linux reports CPU times in clock ticks of `USER_HZ`, fixed at 100 by the kernel's ABI on every
/// architecture Aerith's desktop targets.
pub const TICKS_PER_SECOND: f64 = 100.0;

/// The parent of each process, read from `/proc/<pid>/stat`.
fn parents() -> HashMap<u32, u32> {
    let mut parents = HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return parents;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        if let Some(fields) = stat_fields(pid) {
            // After the command's name: state, then the parent's pid.
            if let Some(parent) = fields.get(1).and_then(|field| field.parse().ok()) {
                parents.insert(pid, parent);
            }
        }
    }
    parents
}

/// The fields of `/proc/<pid>/stat` after the command's name, which may itself hold spaces and
/// parentheses: the name ends at the last `)`.
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = &stat[stat.rfind(')')? + 1..];
    Some(after_name.split_whitespace().map(str::to_owned).collect())
}

/// `root` and every process descended from it, still running.
pub fn tree(root: u32) -> Vec<u32> {
    let parents = parents();
    let mut tree = vec![root];
    let mut index = 0;
    while index < tree.len() {
        let parent = tree[index];
        tree.extend(
            parents
                .iter()
                .filter(|(_, p)| **p == parent)
                .map(|(pid, _)| *pid),
        );
        index += 1;
    }
    tree.retain(|pid| Path::new(&format!("/proc/{pid}")).exists());
    tree
}

/// The proportional set size of a process in KiB: its own pages, and its share of the pages it
/// shares, so that a tree's sum counts shared libraries once. `None` once it has exited.
pub fn pss_kib(pid: u32) -> Option<u64> {
    let rollup = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("Pss:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
}

/// The CPU time a process has used, in ticks, its reaped children's included.
pub fn cpu_ticks(pid: u32) -> Option<u64> {
    let fields = stat_fields(pid)?;
    // utime, stime, cutime and cstime are fields 14 to 17 of the file, 12 to 15 after the name.
    fields[11..15]
        .iter()
        .map(|field| field.parse::<i64>().ok())
        .sum::<Option<i64>>()
        .map(|t| u64::try_from(t).unwrap_or(0))
}

/// The sound devices that `pid` holds open, if any.
pub fn open_sound_devices(pid: u32) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .filter(|target| target.starts_with("/dev/snd"))
        .collect()
}

/// Asks every process of `pids` to stop (SIGTERM), waits up to `grace` for them to exit, then
/// kills those left (SIGKILL). Only processes the bench started are passed here.
pub fn stop(pids: &[u32], grace: Duration) {
    if pids.is_empty() {
        return;
    }
    signal("TERM", pids);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !pids.iter().any(|pid| is_running(*pid)) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    signal("KILL", pids);
}

/// Whether `pid` still runs: its stat is readable and it is not a zombie, which has ended and
/// waits only to be reaped. One read, so that a process reaped in between is not miscounted.
fn is_running(pid: u32) -> bool {
    stat_fields(pid).is_some_and(|fields| fields.first().is_some_and(|state| state != "Z"))
}

/// Sends a signal through kill(1): the standard library sends only SIGKILL, and the bench needs
/// no other dependency for this.
fn signal(name: &str, pids: &[u32]) {
    let mut command = Command::new("kill");
    command.arg(format!("-{name}")).arg("--");
    command.args(pids.iter().map(u32::to_string));
    // A process that exited in between makes kill fail; that is the aim, not an error.
    let _ = command.output();
}

/// The file a command name resolves to through `PATH`, links followed: in a Nix shell, a store
/// path that names the package and its version.
pub fn resolve(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(command))
        .find(|candidate| candidate.is_file())
        .and_then(|found| fs::canonicalize(found).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[test]
    fn a_tree_holds_the_children_and_stop_ends_them() {
        let mut parent = Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30 & wait"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let root = parent.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = tree(root);
        while found.len() < 3 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
            found = tree(root);
        }
        assert_eq!(found.len(), 3, "the shell and its two sleeps: {found:?}");
        assert!(pss_kib(root).is_some_and(|kib| kib > 0));
        assert!(cpu_ticks(root).is_some());
        assert!(open_sound_devices(root).is_empty());
        stop(&found, Duration::from_secs(5));
        parent.wait().unwrap();
        // A killed child may stay a zombie until its new parent reaps it: it no longer runs.
        assert!(found[1..].iter().all(|pid| !is_running(*pid)));
    }

    #[test]
    fn a_process_that_has_exited_reads_as_none() {
        let mut child = Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert_eq!(pss_kib(pid), None);
        assert_eq!(cpu_ticks(pid), None);
        assert!(tree(pid).is_empty());
    }

    #[test]
    fn a_process_that_ignores_sigterm_is_killed() {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; while :; do sleep 0.05; done"])
            .spawn()
            .unwrap();
        // Let the shell install its trap before it is signalled.
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        stop(&[child.id()], Duration::from_millis(300));
        let status = child.wait().unwrap();
        assert!(!status.success(), "it was killed, not ended by itself");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
