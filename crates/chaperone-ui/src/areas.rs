//! The areas thread: for each lane whose newest handoff declares areas,
//! `impact --entry-points --declared` on the files its worktree changes
//! against its base (committed, uncommitted and untracked), one lane at a
//! time. Read again when the worktree commits, the declared areas change,
//! or the lane's worktree or base does.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chaperone_core::git::{BASES, Failure, git, git_bin};
use chaperone_core::work::BoardLane;
use chaperone_graph::Error;

use crate::diff::reflog_time;
use crate::watch::Shown;

/// A poll stats one file per declaring lane, no process.
const POLL: Duration = Duration::from_millis(500);
const DEADLINE: Duration = Duration::from_secs(5);
/// Changed file names; a cut list would hide changes, so past it the
/// check fails.
const OUT_BYTES: usize = 1024 * 1024;

/// One lane's comparison, as read for `key`.
pub struct Areas {
    key: Key,
    pub state: State,
}

impl Areas {
    /// Read for this declaration: a result for an older one is not drawn.
    pub fn is_for(&self, declared: &[String]) -> bool {
        self.key.declared == declared
    }
}

pub enum State {
    Running,
    Done(chaperone_graph::Areas),
    Failed(String),
}

/// What a comparison was read for.
#[derive(Clone, PartialEq)]
struct Key {
    worktree: Option<String>,
    base_sha: Option<String>,
    declared: Vec<String>,
    reflog: Option<SystemTime>,
}

/// Starts the thread. `changed` runs after each change to `shown.areas`.
pub fn spawn(shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let bin = git_bin(Instant::now() + DEADLINE);
        loop {
            let lanes: Vec<(i64, Key)> = {
                let s = shown.lock().unwrap();
                s.board
                    .iter()
                    .flat_map(|b| &b.lanes)
                    .filter_map(|l| {
                        let declared = declared(l)?.to_vec();
                        let item = &l.board.item;
                        Some((
                            item.id,
                            Key {
                                worktree: item.worktree_path.clone(),
                                base_sha: item.base_sha.clone(),
                                declared,
                                reflog: None,
                            },
                        ))
                    })
                    .collect()
            };
            let lanes: HashMap<i64, Key> = lanes
                .into_iter()
                .map(|(id, mut key)| {
                    key.reflog = key.worktree.as_deref().and_then(reflog_time);
                    (id, key)
                })
                .collect();
            // A lane that left the board, or no longer declares, is dropped.
            {
                let mut s = shown.lock().unwrap();
                let before = s.areas.len();
                s.areas.retain(|id, _| lanes.contains_key(id));
                if s.areas.len() != before {
                    drop(s);
                    changed();
                }
            }
            for (id, key) in lanes {
                let stale = shown
                    .lock()
                    .unwrap()
                    .areas
                    .get(&id)
                    .is_none_or(|a| a.key != key);
                if stale {
                    set(&shown, id, &key, State::Running, &changed);
                    let state = read(&bin, &key);
                    set(&shown, id, &key, state, &changed);
                }
            }
            thread::sleep(POLL);
        }
    });
}

fn set(shown: &Mutex<Shown>, id: i64, key: &Key, state: State, changed: &impl Fn()) {
    shown.lock().unwrap().areas.insert(
        id,
        Areas {
            key: key.clone(),
            state,
        },
    );
    changed();
}

/// The lane's changed files against the first base this clone has (the
/// diff panel's), then `impact` on them.
fn read(bin: &Path, key: &Key) -> State {
    let Some(wt) = key.worktree.as_deref().map(Path::new) else {
        return State::Failed("no worktree recorded".into());
    };
    if !wt.is_dir() {
        return State::Failed(format!("worktree missing: {}", wt.display()));
    }
    let deadline = Instant::now() + DEADLINE;
    let run = |args: &[&str]| match git(bin, wt, args, deadline, OUT_BYTES) {
        Ok((_, true)) => Err(format!("git {}: past {OUT_BYTES} bytes", args[0])),
        Ok((out, false)) => Ok(Some(out)),
        // A base this clone lacks: try the next one.
        Err(Failure::BadRevision) => Ok(None),
        Err(Failure::TimedOut) => Err(format!("git {}: timed out", args[0])),
        Err(Failure::Other(e)) => Err(e),
    };
    let mut names = None;
    for base in BASES.iter().copied().chain(key.base_sha.as_deref()) {
        // `--end-of-options`: a claimed base_sha is not checked as hex.
        let args = [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
            // Both sides of a move: the old path's area changes too.
            "--no-renames",
            "--merge-base",
            "--end-of-options",
            base,
            "--",
        ];
        match run(&args) {
            Ok(Some(out)) => {
                names = Some(out);
                break;
            }
            Ok(None) => {}
            Err(e) => return State::Failed(e),
        }
    }
    let Some(mut names) = names else {
        return State::Failed(format!(
            "no base: none of {} or the claimed base_sha is in this clone",
            BASES.join(", ")
        ));
    };
    let args = [
        "ls-files",
        "-z",
        "-o",
        "--exclude-standard",
        "--full-name",
        "--",
        ":/",
    ];
    match run(&args) {
        Ok(out) => names.extend(out.unwrap_or_default()),
        Err(e) => return State::Failed(e),
    }
    let paths: Vec<String> = names
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).to_string())
        .collect();
    let declared = Some(key.declared.clone());
    match chaperone_graph::impact_in(wt, paths, None, true, None, declared) {
        Ok(i) => i
            .areas
            .map_or_else(|| State::Failed("impact gave no areas".into()), State::Done),
        Err(Error::Invalid(e) | Error::Internal(e)) => State::Failed(e),
    }
}

/// The areas the lane's newest handoff declares.
pub fn declared(lane: &BoardLane) -> Option<&[String]> {
    lane.board.handoff.as_ref()?.handoff.areas.as_deref()
}

/// The card's cell: the declared areas, and what the diff does beyond them.
pub fn card(declared: Option<&[String]>, areas: Option<&Areas>) -> String {
    let Some(declared) = declared else {
        return "not declared".into();
    };
    let d = declared.join(", ");
    let a = match areas.filter(|a| a.is_for(declared)).map(|a| &a.state) {
        None | Some(State::Running) => return format!("{d} (checking)"),
        Some(State::Failed(_)) => return format!("{d} (not checked)"),
        Some(State::Done(a)) => a,
    };
    let mut parts = Vec::new();
    if !a.undeclared.is_empty() {
        let names: Vec<&str> = a.undeclared.iter().map(|u| u.area.as_str()).collect();
        parts.push(format!("+{} undeclared", names.join(", ")));
    }
    if !a.unknown.is_empty() {
        parts.push(format!("unknown {}", a.unknown.join(", ")));
    }
    let unmapped = a.unmapped.len() + a.unmapped_omitted;
    if unmapped > 0 {
        let files = if unmapped == 1 { "file" } else { "files" };
        parts.push(format!("{unmapped} {files} in no area"));
    }
    match parts.is_empty() {
        true => format!("{d}: in scope"),
        false => format!("{d}: {}", parts.join("; ")),
    }
}
