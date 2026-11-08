//! The diff thread: the selected lane's worktree against its base
//! (`git diff --merge-base`, tracked files, uncommitted edits included),
//! read again when the worktree commits or the viewer asks. Locks are off
//! and text conversion and external diff drivers are not run; a clean
//! filter set in `.gitattributes` (git-lfs) still runs for a modified
//! file, and git has no switch for that.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chaperone_core::git::{BASES, Failure, git, git_bin};

use crate::watch::{Selected, Shown};

/// A poll stats one file, no process; a commit shows within this.
const POLL: Duration = Duration::from_millis(250);
pub const DEADLINE: Duration = Duration::from_secs(5);
pub const OUT_BYTES: usize = 1024 * 1024;

/// One lane's diff, or why there is none.
pub struct Diff {
    pub lane: i64,
    pub state: State,
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
                // read again if the lane comes back.
                if done.as_ref() != Some(&read)
                    && set(&shown, &read, State::Running, &changed)
                    && set(&shown, &read, diff(&bin, &read.lane), &changed)
                {
                    done = Some(read);
                }
            }
            thread::sleep(POLL);
        }
    });
}

/// Shows `state` if the viewer still wants what `read` read.
fn set(shown: &Mutex<Shown>, read: &Read, state: State, changed: &impl Fn()) -> bool {
    let mut s = shown.lock().unwrap();
    if s.selected.as_ref() != Some(&read.lane) || s.refresh != read.refresh {
        return false;
    }
    s.diff = Some(Diff {
        lane: read.lane.id,
        state,
    });
    drop(s);
    changed();
    true
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
        // `--end-of-options`: a claimed base_sha is not checked as hex.
        let args = [
            pre,
            &["--merge-base", "--end-of-options", base],
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
