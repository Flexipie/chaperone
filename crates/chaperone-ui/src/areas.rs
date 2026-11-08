//! The areas thread: for each lane whose newest handoff declares areas,
//! `impact --entry-points --declared` on the files its worktree changes
//! against its base (committed, uncommitted and untracked), one lane at a
//! time. Read again when the worktree commits, the declared areas change,
//! the repo's profile is edited, or the lane's worktree or base does.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chaperone_core::git::{changed_files, git_bin};
use chaperone_core::work::BoardLane;
use chaperone_graph::{Error, profile_file};

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
    /// The profile's mtime: its `[areas]` decide the answer.
    profile: Option<SystemTime>,
}

/// Starts the thread. `changed` runs after each change to `shown.areas`.
pub fn spawn(shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let bin = git_bin(Instant::now() + DEADLINE);
        loop {
            let lanes: Vec<(i64, Key, Option<PathBuf>)> = {
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
                                profile: None,
                            },
                            item.repo_slug.as_deref().and_then(profile_file),
                        ))
                    })
                    .collect()
            };
            let lanes: HashMap<i64, Key> = lanes
                .into_iter()
                .map(|(id, mut key, profile)| {
                    key.reflog = key.worktree.as_deref().and_then(reflog_time);
                    key.profile = profile
                        .and_then(|p| fs::metadata(p).ok())
                        .and_then(|m| m.modified().ok());
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
/// diff panel's, and the lane page's set), then `impact` on them.
fn read(bin: &Path, key: &Key) -> State {
    let Some(wt) = key.worktree.as_deref().map(Path::new) else {
        return State::Failed("no worktree recorded".into());
    };
    if !wt.is_dir() {
        return State::Failed(format!("worktree missing: {}", wt.display()));
    }
    let deadline = Instant::now() + DEADLINE;
    let paths = match changed_files(bin, wt, key.base_sha.as_deref(), deadline, OUT_BYTES) {
        Ok((_, files)) => files.into_iter().map(|f| f.path).collect(),
        Err(e) => return State::Failed(e),
    };
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
