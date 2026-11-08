//! The change graph's thread: while the graph is shown, `overlap` (each
//! open lane's files) when the ledger changes or a lane's worktree commits,
//! then `impact` and lines changed for each lane whose files or head moved
//! since its last read, one lane at a time. A repo's layout is built again
//! only when one of its lanes was or overlap's notes changed. Reads the
//! ledger read-only; `impact` writes the code graph's own cache (ADR 0043),
//! never the ledger.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chaperone_core::git::git_bin;
use chaperone_core::ledger::Ledger;
use chaperone_core::overlap::overlap;
use chaperone_graph::{Error, impact_in};

use crate::diff::{DEADLINE, against_base, reflog_time};
use crate::graph::{Lane, Layout, Repos, layout};
use crate::watch::Shown;

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

/// Starts the thread. `changed` runs after each change to `shown.graph`.
pub fn spawn(path: PathBuf, shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let mut ledger: Option<Ledger> = None;
        // The (data_version, reflog times) last read.
        let mut seen = None;
        let mut lanes: HashMap<i64, (Key, Lane)> = HashMap::new();
        let mut repos: HashMap<String, Built> = HashMap::new();
        loop {
            thread::sleep(POLL);
            let (version, want) = {
                let s = shown.lock().unwrap();
                match (&s.board, s.graph_open) {
                    (Some(b), true) => (
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
                    ),
                    _ => continue,
                }
            };
            let times: Vec<Option<SystemTime>> = want
                .iter()
                .map(|w| w.worktree.as_deref().and_then(reflog_time))
                .collect();
            let now = Some((version, times));
            if seen == now {
                continue;
            }
            let read = match &ledger {
                Some(l) => overlap(l, None, false),
                None => Ledger::open_read_only(&path)
                    .and_then(|l| overlap(ledger.insert(l), None, false)),
            };
            let over = match read {
                Ok(o) => o,
                Err(e) => {
                    // Reopened and read again next poll; shown once.
                    ledger = None;
                    let text = e.to_string();
                    let mut s = shown.lock().unwrap();
                    if !matches!(&s.graph, Some(Err(t)) if *t == text) {
                        s.graph = Some(Err(text));
                        drop(s);
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
            }

            lanes.retain(|id, _| want.iter().any(|w| w.id == *id));
            let mut by_repo: BTreeMap<String, (String, Vec<Part>)> = BTreeMap::new();
            for w in &want {
                let paths = files.remove(&w.id).unwrap_or_default();
                let note = notes.remove(&w.id).unwrap_or_default();
                if paths.is_empty() && note.is_empty() {
                    lanes.remove(&w.id);
                    continue;
                }
                let key = (heads.get(&w.id).cloned().unwrap_or_default(), paths);
                // An unread worktree keeps the part last read from it.
                let fresh = lanes
                    .get(&w.id)
                    .is_some_and(|(k, _)| *k == key || unread.contains(&w.id));
                if !fresh {
                    let lane = read_lane(w, &key);
                    lanes.insert(w.id, (key, lane));
                }
                by_repo
                    .entry(w.slug.to_lowercase())
                    .or_insert_with(|| (w.slug.clone(), Vec::new()))
                    .1
                    .push((w.id, lanes[&w.id].0.clone(), note));
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

fn publish(shown: &Mutex<Shown>, graph: Result<Repos, String>, changed: &impl Fn()) {
    shown.lock().unwrap().graph = Some(graph);
    changed();
}

/// One lane's impact and lines changed, read at `key`.
fn read_lane(w: &Want, key: &Key) -> Lane {
    let (head, paths) = key;
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
    let impact = match impact_in(wt, paths.clone(), None, true, None) {
        Ok(i) => Ok(i),
        Err(e) => {
            notes.push(format!("no entry points: {}", why(e)));
            impact_in(wt, paths.clone(), None, false, None).map_err(why)
        }
    };
    if head.is_empty() {
        notes.push("no commits since the base".into());
    }
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
