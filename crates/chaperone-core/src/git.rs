//! git calls in a worktree, under a deadline: `overlap`, `impact`, the
//! UI's diff and areas, and the lane page's files.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde::Serialize;

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

/// A file a lane's worktree changes against its base.
#[derive(Debug, Serialize)]
pub struct ChangedFile {
    pub path: String,
    /// The repo profile's area, as the caller of [`lane_files`] maps it.
    pub area: Option<String>,
    /// Lines, worktree against the merge base (the change graph's count
    /// is `HEAD`'s). None for a binary file, and for an untracked one past
    /// [`COUNT_BYTES`] or not counted.
    pub added: Option<u64>,
    pub removed: Option<u64>,
    pub untracked: bool,
}

/// An untracked file's lines are counted up to this size.
const COUNT_BYTES: u64 = 1024 * 1024;

/// Every file the worktree `wt` changes against the first base this clone
/// has ([`BASES`], then the claimed `base_sha`): committed and uncommitted
/// (no renames, so both sides of a move), then untracked. The base and
/// the files; output past `cap` fails, since a cut list hides changes.
/// Uncommitted edits do not move the reflog, so a caller that rereads on
/// it misses them until the next commit.
pub fn changed_files(
    bin: &Path,
    wt: &Path,
    base_sha: Option<&str>,
    deadline: Instant,
    cap: usize,
) -> Result<(String, Vec<ChangedFile>), String> {
    let run = |args: &[&str]| match git(bin, wt, args, deadline, cap) {
        Ok((_, true)) => Err(format!("git {}: past {cap} bytes", args[0])),
        Ok((out, false)) => Ok(Some(out)),
        // A base this clone lacks: try the next one.
        Err(Failure::BadRevision) => Ok(None),
        Err(Failure::TimedOut) => Err(format!("git {}: timed out", args[0])),
        Err(Failure::Other(e)) => Err(e),
    };
    let mut found = None;
    for base in BASES.iter().copied().chain(base_sha) {
        // `--end-of-options`: a claimed base_sha is not checked as hex.
        let args = [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "-z",
            "--no-renames",
            "--merge-base",
            "--end-of-options",
            base,
            "--",
        ];
        if let Some(out) = run(&args)? {
            found = Some((base.to_string(), out));
            break;
        }
    }
    let Some((base, out)) = found else {
        return Err(format!(
            "no base: none of {} or the claimed base_sha is in this clone",
            BASES.join(", ")
        ));
    };
    // `<added>\t<removed>\t<path>\0`, `-` for a binary file's counts.
    let mut files: Vec<ChangedFile> = split_z(&out)
        .filter_map(|row| {
            let mut cols = row.splitn(3, '\t');
            let (added, removed) = (cols.next()?.parse().ok(), cols.next()?.parse().ok());
            Some(ChangedFile {
                path: cols.next()?.to_string(),
                area: None,
                added,
                removed,
                untracked: false,
            })
        })
        .collect();
    let args = [
        "ls-files",
        "-z",
        "-o",
        "--exclude-standard",
        "--full-name",
        "--",
        ":/",
    ];
    let untracked = run(&args)?.unwrap_or_default();
    files.extend(split_z(&untracked).map(|path| ChangedFile {
        path: path.to_string(),
        area: None,
        added: None,
        removed: None,
        untracked: true,
    }));
    Ok((base, files))
}

/// [`changed_files`] with each file's area from `area`, and an untracked
/// text file's lines as added: the lane page's files.
pub fn lane_files(
    bin: &Path,
    wt: &Path,
    base_sha: Option<&str>,
    deadline: Instant,
    cap: usize,
    area: impl Fn(&str) -> Option<String>,
) -> Result<(String, Vec<ChangedFile>), String> {
    let (base, mut files) = changed_files(bin, wt, base_sha, deadline, cap)?;
    for f in &mut files {
        f.area = area(&f.path);
        if f.untracked {
            f.added = lines(&wt.join(&f.path));
            f.removed = f.added.map(|_| 0);
        }
    }
    Ok((base, files))
}

fn split_z(out: &[u8]) -> impl Iterator<Item = String> + '_ {
    out.split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
}

/// A text file's lines (a last line without a newline counts), None when
/// it is binary (has a NUL), past [`COUNT_BYTES`] or unreadable.
fn lines(path: &Path) -> Option<u64> {
    let mut bytes = Vec::new();
    let read = File::open(path)
        .ok()?
        .take(COUNT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if read as u64 > COUNT_BYTES || bytes.contains(&0) {
        return None;
    }
    let newlines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    Some(newlines + u64::from(bytes.last().is_some_and(|b| *b != b'\n')))
}
