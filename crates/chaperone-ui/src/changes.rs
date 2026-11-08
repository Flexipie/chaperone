//! The change graph's thread: while the graph or a lane's page is shown,
//! `overlap` (each open lane's files) when the ledger changes or a lane's
//! worktree commits, then `impact` and lines changed for each lane whose
//! files or head moved since its last read, one lane at a time; with only
//! a lane's page open, for that lane alone, and the other lanes' reads
//! are kept. A repo's layout is built again only when one of its lanes was
//! or overlap's notes changed. Reads the ledger read-only; `impact` writes
//! the code graph's own cache (ADR 0043), never the ledger.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chaperone_core::git::git_bin;
use chaperone_core::ledger::Ledger;
use chaperone_core::overlap::{Overlap, overlap};
use chaperone_graph::{Error, impact_in, profile_file};

use crate::diff::{DEADLINE, against_base, reflog_time};
use crate::graph::{Lane, Layout, Repos, lane_layout, layout};
use crate::watch::{LaneGraph, LaneSnap, Shown};

/// A poll stats one file per lane, no process.
const POLL: Duration = Duration::from_millis(250);

/// What a lane's part was read from: its head and its files.
type Key = (String, Vec<String>);

/// A lane in a repo's layout: its id, key and overlap notes.
type Part = (i64, Key, Vec<String>);

/// The lanes a repo's layout was built from, overlap's files left out,
/// and the layout.
type Built = (Vec<Part>, usize, Arc<Layout>);

/// One board lane, as the thread needs it.
struct Want {
    id: i64,
    slug: String,
    worktree: Option<String>,
    base_sha: Option<String>,
}

/// Starts the thread. `changed` runs after each change to `shown.graph`
/// or `shown.lane_graph`.
pub fn spawn(path: PathBuf, shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let mut ledger: Option<Ledger> = None;
        // The (data_version, reflog times, graph shown, lane selected, its
        // profile's mtime) last read.
        let mut seen = None;
        let mut lanes: HashMap<i64, (Key, Lane)> = HashMap::new();
        // Lanes whose last read failed, read again on the next overlap run.
        let mut retry: BTreeSet<i64> = BTreeSet::new();
        let mut repos: HashMap<String, Built> = HashMap::new();
        loop {
            thread::sleep(POLL);
            let (version, want, open, sel) = {
                let s = shown.lock().unwrap();
                let sel = s.selected.as_ref().map(|l| (l.id, l.repo.clone()));
                match &s.board {
                    Some(b) if s.graph_open || sel.is_some() => (
                        b.data_version,
                        b.lanes
                            .iter()
                            .filter_map(|l| {
                                let i = &l.board.item;
                                Some(Want {
                                    id: i.id,
                                    slug: i.repo_slug.clone()?,
                                    worktree: i.worktree_path.clone(),
                                    base_sha: i.base_sha.clone(),
                                })
                            })
                            .collect::<Vec<_>>(),
                        s.graph_open,
                        sel,
                    ),
                    _ => continue,
                }
            };
            // The profile names the entry points: an edit reads the lane again.
            let (sel, profile) = match sel {
                Some((id, repo)) => {
                    let p = repo.as_deref().and_then(profile_file);
                    let m = p.and_then(|p| fs::metadata(p).ok()?.modified().ok());
                    (Some(id), m)
                }
                None => (None, None),
            };
            let times: Vec<Option<SystemTime>> = want
                .iter()
                .map(|w| w.worktree.as_deref().and_then(reflog_time))
                .collect();
            let now = Some((version, times, open, sel, profile));
            if seen == now {
                continue;
            }
            if let (Some(id), Some((.., was, p))) = (sel, &seen)
                && *was == sel
                && *p != profile
            {
                retry.insert(id);
            }
            let read = match &ledger {
                Some(l) => overlap(l, None, false),
                None => Ledger::open_read_only(&path)
                    .and_then(|l| overlap(ledger.insert(l), None, false)),
            };
            let over = match read {
                Ok(o) => o,
                Err(e) => {
                    // Reopened and read again next poll.
                    ledger = None;
                    let text = e.to_string();
                    let mut s = shown.lock().unwrap();
                    // Shown once.
                    let same = |g: &LaneGraph| {
                        Some(g.lane) == sel && matches!(&g.read, Err(t) if *t == text)
                    };
                    let new = open && !matches!(&s.graph, Some(Err(t)) if *t == text)
                        || sel.is_some() && !s.lane_graph.as_ref().is_some_and(same);
                    if open {
                        s.graph = Some(Err(text.clone()));
                    }
                    if let Some(id) = sel {
                        s.lane_graph = Some(LaneGraph {
                            lane: id,
                            read: Err(text),
                        });
                    }
                    drop(s);
                    if new {
                        changed();
                    }
                    continue;
                }
            };
            seen = now;

            let mut files: BTreeMap<i64, Vec<String>> = BTreeMap::new();
            let mut heads: BTreeMap<i64, String> = BTreeMap::new();
            for row in &over.files {
                for e in &row.lanes {
                    // A lane's handoff and worktree evidence are adjacent.
                    let f = files.entry(e.id).or_default();
                    if f.last() != Some(&row.path) {
                        f.push(row.path.clone());
                    }
                    if e.source == "worktree"
                        && let Some(c) = &e.commit
                    {
                        heads.insert(e.id, c.clone());
                    }
                }
            }
            let mut notes: BTreeMap<i64, Vec<String>> = BTreeMap::new();
            // Lanes whose worktree read failed this time.
            let mut unread = BTreeSet::new();
            for t in &over.lanes {
                let n = notes.entry(t.id).or_default();
                if t.worktree != "ok" {
                    // Only a failed read passes; no worktree or base may last.
                    if matches!(t.worktree, "timed_out" | "error") {
                        unread.insert(t.id);
                    }
                    let why = t
                        .error
                        .as_deref()
                        .map_or(String::new(), |e| format!(": {e}"));
                    n.push(format!("worktree {}{why}", t.worktree));
                }
                if t.cut > 0 {
                    n.push(format!("{} worktree paths cut", t.cut));
                }
                if t.worktree == "ok" && t.paths == 0 {
                    n.push("no commits since the base".into());
                }
            }

            let sel_note = sel.and_then(|id| notes.get(&id).cloned());
            if open {
                lanes.retain(|id, _| want.iter().any(|w| w.id == *id));
                retry.retain(|id| lanes.contains_key(id));
            }
            let mut by_repo: BTreeMap<String, (String, Vec<Part>)> = BTreeMap::new();
            for w in want.iter().filter(|w| open || Some(w.id) == sel) {
                // Sorted: overlap lists shared files first, so another lane's
                // clash would reorder them.
                let mut paths = files.remove(&w.id).unwrap_or_default();
                paths.sort_unstable();
                let note = notes.remove(&w.id).unwrap_or_default();
                if paths.is_empty() && note.is_empty() {
                    lanes.remove(&w.id);
                    continue;
                }
                let key = (heads.get(&w.id).cloned().unwrap_or_default(), paths);
                // An unread worktree keeps the part last read from it.
                let fresh = lanes.get(&w.id).is_some_and(|(k, _)| {
                    (*k == key && !retry.contains(&w.id)) || unread.contains(&w.id)
                });
                if !fresh {
                    let lane = read_lane(w, &key);
                    // A failed read (a timeout on a busy machine) is tried
                    // again on the next overlap run, not kept for good.
                    if lane.impact.is_err()
                        || lane.notes.iter().any(|n| n.starts_with("no line counts"))
                    {
                        retry.insert(w.id);
                    } else {
                        retry.remove(&w.id);
                    }
                    lanes.insert(w.id, (key, lane));
                    // A read under the same key (a retry) changes the part,
                    // so the repo's layout is stale, shown or not.
                    repos.remove(&w.slug.to_lowercase());
                }
                by_repo
                    .entry(w.slug.to_lowercase())
                    .or_insert_with(|| (w.slug.clone(), Vec::new()))
                    .1
                    .push((w.id, lanes[&w.id].0.clone(), note));
            }

            if let Some(id) = sel {
                let read = match (want.iter().any(|w| w.id == id), lanes.get(&id)) {
                    (false, _) => Err("No repo recorded for the lane.".into()),
                    (true, None) => Err("No changes since the base.".into()),
                    (true, Some((key, _))) if key.1.is_empty() => {
                        Err(sel_note.unwrap_or_default().join("; "))
                    }
                    (true, Some((_, lane))) => Ok(snapshot(lane, sel_note, &over)),
                };
                shown.lock().unwrap().lane_graph = Some(LaneGraph { lane: id, read });
                changed();
            }
            if !open {
                continue;
            }
            repos.retain(|r, _| by_repo.contains_key(r));
            let mut out = Vec::new();
            for (repo, (slug, keys)) in by_repo {
                let built = match repos.get(&repo) {
                    Some((k, n, l)) if *k == keys && *n == over.files_omitted => l.clone(),
                    _ => {
                        let part: Vec<&Lane> = keys.iter().map(|(id, ..)| &lanes[id].1).collect();
                        let mut l = layout(&part);
                        for (id, _, n) in keys.iter().filter(|(.., n)| !n.is_empty()) {
                            l.notes.push(format!("#{id}: {}", n.join("; ")));
                        }
                        if over.files_omitted > 0 {
                            l.notes
                                .push(format!("overlap left out {} files", over.files_omitted));
                        }
                        let l = Arc::new(l);
                        repos.insert(repo, (keys, over.files_omitted, l.clone()));
                        l
                    }
                };
                out.push((slug, built));
            }
            publish(&shown, Ok(out), &changed);
        }
    });
}

/// The lane's graph, ringing the files other open lanes change too.
fn snapshot(lane: &Lane, notes: Option<Vec<String>>, over: &Overlap) -> LaneSnap {
    let mut others: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for row in over
        .files
        .iter()
        .filter(|r| r.lanes.iter().any(|e| e.id == lane.id))
    {
        for e in row.lanes.iter().filter(|e| e.id != lane.id) {
            let ids = others.entry(row.path.clone()).or_default();
            if !ids.contains(&e.id) {
                ids.push(e.id);
            }
        }
    }
    let impact = lane.impact.as_ref().ok();
    let ep = impact.and_then(|i| i.entry_points.as_ref());
    LaneSnap {
        layout: Arc::new(lane_layout(lane, &others)),
        notes: notes.unwrap_or_default(),
        files_omitted: over.files_omitted,
        reached: ep.map(|e| (e.reached.len() + e.omitted, e.total)),
        exact: impact.is_some_and(|i| i.bound == "exact") && ep.is_some_and(|e| e.bound == "exact"),
    }
}

fn publish(shown: &Mutex<Shown>, graph: Result<Repos, String>, changed: &impl Fn()) {
    shown.lock().unwrap().graph = Some(graph);
    changed();
}

/// One lane's impact and lines changed, read at `key`.
fn read_lane(w: &Want, key: &Key) -> Lane {
    let (_, paths) = key;
    let mut notes = Vec::new();
    let Some(wt) = w.worktree.as_deref().map(Path::new).filter(|p| p.is_dir()) else {
        return Lane {
            id: w.id,
            files: paths.iter().map(|p| (p.clone(), 0)).collect(),
            impact: Err("no worktree to read".into()),
            notes,
        };
    };
    let lines = numstat(wt, w.base_sha.as_deref()).unwrap_or_else(|e| {
        notes.push(format!("no line counts: {e}"));
        HashMap::new()
    });
    let files = paths
        .iter()
        .map(|p| (p.clone(), lines.get(p).copied().unwrap_or(0)))
        .collect();
    let why = |e| match e {
        Error::Invalid(e) | Error::Internal(e) => e,
    };
    // Entry points need the repo's pipeline profile; without one, the
    // dependents still show.
    let impact = match impact_in(wt, paths.clone(), None, true, None, None) {
        Ok(i) => Ok(i),
        Err(e) => {
            notes.push(format!("no entry points: {}", why(e)));
            impact_in(wt, paths.clone(), None, false, None, None).map_err(why)
        }
    };
    Lane {
        id: w.id,
        files,
        impact,
        notes,
    }
}

/// Lines added and removed per path in HEAD's commits since the
/// merge-base with the first base this clone has (as the diff panel).
fn numstat(wt: &Path, base_sha: Option<&str>) -> Result<HashMap<String, u64>, String> {
    let bin = git_bin(Instant::now() + DEADLINE);
    let args = [
        "diff",
        "--numstat",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
    ];
    let (_, out, _) = against_base(&bin, wt, base_sha, &args, &["HEAD"])?;
    // `added\tremoved\tpath`; `-` for a binary file.
    Ok(String::from_utf8_lossy(&out)
        .split('\0')
        .filter_map(|e| {
            let mut f = e.splitn(3, '\t');
            let n = |s: Option<&str>| s.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let (a, r) = (n(f.next()), n(f.next()));
            Some((f.next()?.to_string(), a + r))
        })
        .collect())
}
