//! git calls in a worktree, under a deadline: `overlap`, `impact` and the
//! UI's diff.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::child;

/// Where a lane's base is looked for, before its claimed `base_sha`.
pub const BASES: [&str; 3] = ["origin/HEAD", "origin/main", "origin/master"];

/// The git binary itself, from its exec path: macOS's `/usr/bin/git` is
/// an xcrun shim that adds about 9 ms to every call. Else plain `git`.
pub fn git_bin(deadline: Instant) -> PathBuf {
    let found = git(
        Path::new("git"),
        Path::new("."),
        &["--exec-path"],
        deadline,
        4096,
    )
    .ok();
    found
        .map(|(out, _)| Path::new(String::from_utf8_lossy(&out).trim()).join("git"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| "git".into())
}

pub enum Failure {
    TimedOut,
    /// A revision this clone does not have.
    BadRevision,
    Other(String),
}

/// Variables that point git at a repository other than `-C`'s: the ones
/// git sets for hooks and `rebase --exec`.
const GIT_REPO_VARS: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// One git call in `wt`, never taking the worktree's index lock (it
/// belongs to the lane) and never prompting.
pub fn git(
    bin: &Path,
    wt: &Path,
    args: &[&str],
    deadline: Instant,
    cap: usize,
) -> Result<(Vec<u8>, bool), Failure> {
    let mut cmd = Command::new(bin);
    for v in GIT_REPO_VARS {
        cmd.env_remove(v);
    }
    cmd.arg("-C")
        .arg(wt)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        // Messages in English, so a missing revision is recognised.
        .env("LC_ALL", "C");
    let done = child::run(&mut cmd, deadline, cap).map_err(|e| match e {
        child::Failed::TimedOut => Failure::TimedOut,
        child::Failed::Spawn(e) | child::Failed::Wait(e) => Failure::Other(format!("git: {e}")),
    })?;
    if !done.status.success() {
        let err = String::from_utf8_lossy(&done.err);
        if ["unknown revision", "bad revision", "Invalid revision range"]
            .iter()
            .any(|m| err.contains(m))
        {
            return Err(Failure::BadRevision);
        }
        // The fatal line is last; warnings come before it.
        let line: String = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .chars()
            .take(300)
            .collect();
        return Err(Failure::Other(format!("git {}: {line}", args[0])));
    }
    Ok((done.out, done.cut))
}
