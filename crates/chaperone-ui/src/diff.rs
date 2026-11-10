//! The diff thread: the selected lane's worktree against its base
//! (`git diff --merge-base`, tracked files, uncommitted edits included),
//! read again when the worktree commits or the viewer asks. Locks are off
//! and text conversion and external diff drivers are not run; a clean
//! filter set in `.gitattributes` (git-lfs) still runs for a modified
//! file, and git has no switch for that. The lane page's files (area and
//! +/-, untracked included) are read with it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chaperone_core::git::{BASES, ChangedFile, Failure, base_rev, git, git_bin, lane_files};
use chaperone_graph::{Error, profile_file, rules};

use crate::watch::{Selected, Shown};

/// A poll stats one file, no process; a commit shows within this.
const POLL: Duration = Duration::from_millis(250);
pub const DEADLINE: Duration = Duration::from_secs(5);
pub const OUT_BYTES: usize = 1024 * 1024;

/// One lane's diff and files, or why there are none.
pub struct Diff {
    pub lane: i64,
    pub state: State,
    /// None while the diff is read.
    pub files: Option<Result<Files, String>>,
}

/// The lane's files against `base`, and why they have no areas, if so.
pub struct Files {
    pub base: String,
    pub files: Vec<ChangedFile>,
    pub no_areas: Option<String>,
}

pub enum State {
    Running,
    /// Against `base`; `cut` when it passed [`OUT_BYTES`].
    Text {
        base: String,
        lines: Vec<String>,
        cut: bool,
    },
    Failed(String),
}

/// What a diff was read for: the lane, the refresh asked, and the
/// worktree's newest reflog write (a commit, checkout or reset).
#[derive(PartialEq)]
struct Read {
    lane: Selected,
    refresh: u64,
    reflog: Option<SystemTime>,
}

/// Starts the thread. `changed` runs after each change to `shown.diff`.
pub fn spawn(shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let bin = git_bin(Instant::now() + DEADLINE);
        let mut done: Option<Read> = None;
        loop {
            let want = {
                let s = shown.lock().unwrap();
                s.selected.clone().map(|lane| (lane, s.refresh))
            };
            if let Some((lane, refresh)) = want {
                let read = Read {
                    reflog: lane.worktree.as_deref().and_then(reflog_time),
                    lane,
                    refresh,
                };
                // A diff for a lane no longer selected is dropped, so it is
                // read again if the lane comes back. The diff shows before
                // the files are read.
                let id = read.lane.id;
                if done.as_ref() != Some(&read)
                    && set(&shown, &read, &changed, |d| {
                        *d = Some(Diff {
                            lane: id,
                            state: State::Running,
                            files: kept(d, id),
                        })
                    })
                    && set(&shown, &read, &changed, {
                        let state = diff(&bin, &read.lane);
                        move |d| {
                            *d = Some(Diff {
                                lane: id,
                                state,
                                files: kept(d, id),
                            })
                        }
                    })
                    && set(&shown, &read, &changed, {
                        let files = files(&bin, &read.lane);
                        move |d| {
                            if let Some(d) = d {
                                d.files = Some(files);
                            }
                        }
                    })
                {
                    done = Some(read);
                }
            }
            thread::sleep(POLL);
        }
    });
}

/// Writes `shown.diff` if the viewer still wants what `read` read.
fn set(
    shown: &Mutex<Shown>,
    read: &Read,
    changed: &impl Fn(),
    write: impl FnOnce(&mut Option<Diff>),
) -> bool {
    let mut s = shown.lock().unwrap();
    if s.selected.as_ref() != Some(&read.lane) || s.refresh != read.refresh {
        return false;
    }
    write(&mut s.diff);
    drop(s);
    changed();
    true
}

/// The files the lane showed, kept until the new ones are read.
fn kept(d: &mut Option<Diff>, lane: i64) -> Option<Result<Files, String>> {
    d.take().filter(|d| d.lane == lane).and_then(|d| d.files)
}

/// Against the first base this clone has. The last, the lane's `base_sha`,
/// is its newest handoff commit once it has handed off (as for `overlap`).
fn diff(bin: &Path, lane: &Selected) -> State {
    let Some(wt) = lane.worktree.as_deref().map(Path::new) else {
        return State::Failed("no worktree recorded".into());
    };
    if !wt.is_dir() {
        return State::Failed(format!("worktree missing: {}", wt.display()));
    }
    let args = ["diff", "--no-color", "--no-ext-diff", "--no-textconv"];
    match against_base(bin, wt, lane.base_sha.as_deref(), &args, &[]) {
        Ok((base, out, cut)) => State::Text {
            base,
            lines: String::from_utf8_lossy(&out)
                .lines()
                .map(str::to_owned)
                .collect(),
            cut,
        },
        Err(e) => State::Failed(e),
    }
}

/// The lane's changed files with their area in the repo's profile; no
/// profile, or one that does not parse, is said, never areas left blank.
fn files(bin: &Path, lane: &Selected) -> Result<Files, String> {
    let wt = lane.worktree.as_deref().map(Path::new);
    let wt = wt.filter(|w| w.is_dir()).ok_or("no worktree to read")?;
    let profile = lane.repo.as_deref().and_then(profile_file);
    let (rules, no_areas) = match rules(profile, None) {
        Ok(r) => (Some(r), None),
        Err(Error::Invalid(e) | Error::Internal(e)) => (None, Some(e)),
    };
    let area = |path: &str| {
        let a = rules.as_ref()?.area(path);
        (!a.is_empty()).then(|| a.to_string())
    };
    let deadline = Instant::now() + DEADLINE;
    let base = lane.base_sha.as_deref();
    let (base, files) = lane_files(bin, wt, base, deadline, OUT_BYTES, area)?;
    Ok(Files {
        base,
        files,
        no_areas,
    })
}

/// `git <pre> --merge-base <base> <post> --` in `wt` against the first base
/// this clone has, the claimed `base_sha` last: the base, git's output and
/// whether it was cut at [`OUT_BYTES`], or why there is none.
pub fn against_base(
    bin: &Path,
    wt: &Path,
    base_sha: Option<&str>,
    pre: &[&str],
    post: &[&str],
) -> Result<(String, Vec<u8>, bool), String> {
    let deadline = Instant::now() + DEADLINE;
    for base in BASES.iter().copied().chain(base_sha) {
        let full = base_rev(base);
        // `--end-of-options`: a claimed base_sha is not checked as hex.
        let args = [
            pre,
            &["--merge-base", "--end-of-options", &full],
            post,
            &["--"],
        ]
        .concat();
        match git(bin, wt, &args, deadline, OUT_BYTES) {
            Ok((out, cut)) => return Ok((base.into(), out, cut)),
            // A base this clone lacks: try the next one.
            Err(Failure::BadRevision) => {}
            Err(Failure::TimedOut) => {
                return Err(format!("git {} passed {}s", pre[0], DEADLINE.as_secs()));
            }
            Err(Failure::Other(e)) => return Err(e),
        }
    }
    Err(format!(
        "no base: none of {} or the claimed base_sha is in this clone",
        BASES.join(", ")
    ))
}

/// When the worktree's HEAD reflog was last written. `.git` is the git dir
/// in a main checkout and a `gitdir:` file in a linked worktree.
pub fn reflog_time(wt: &str) -> Option<SystemTime> {
    let dot_git = Path::new(wt).join(".git");
    let dir = match fs::read_to_string(&dot_git) {
        Ok(text) => {
            let dir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
            if dir.is_absolute() {
                dir
            } else {
                Path::new(wt).join(dir)
            }
        }
        Err(_) => dot_git,
    };
    fs::metadata(dir.join("logs").join("HEAD"))
        .and_then(|m| m.modified())
        .ok()
}
