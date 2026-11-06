//! `chaperone overlap`: per file, the open lanes touching it. The ledger
//! read is the board's; git runs in each worktree only after it has ended
//! (ADR 0007: a ledger read runs no git), within one deadline.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chaperone_core::ledger::Ledger;
use serde::Serialize;
use serde_json::json;

use crate::child;
use crate::output::{CliError, Outcome};
use crate::work::one;

/// For every git call of one run, together.
const DEADLINE: Duration = Duration::from_secs(5);
const WORKERS: usize = 8;
/// Worktree paths kept per lane; the rest are counted in `cut`.
const PATHS: usize = 500;
const OUT_BYTES: usize = 256 * 1024;
/// File rows printed; the rest are counted in `files_omitted`.
const ROWS: usize = 2000;
/// Where a lane's base is looked for, before its claimed `base_sha`.
const BASES: [&str; 3] = ["origin/HEAD", "origin/main", "origin/master"];

struct Lane {
    id: i64,
    repo: Option<String>,
    worktree: Option<String>,
    base_sha: Option<String>,
    commit: Option<String>,
    files: Vec<String>,
}

#[derive(Serialize)]
struct Evidence {
    id: i64,
    source: &'static str,
    commit: Option<String>,
}

/// What git said for one lane: `ok`, `none` (no worktree recorded),
/// `missing`, `no_base`, `timed_out`, `error`, or `no_repo` (not compared).
#[derive(Serialize)]
struct Tree {
    id: i64,
    repo: Option<String>,
    worktree: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    paths: usize,
    cut: usize,
}

pub fn run(ledger: &Ledger, repo: Option<String>) -> Result<Outcome, CliError> {
    let want = repo.map(|r| r.trim().to_lowercase());
    let mut lanes = Vec::new();
    let Ok(()) = ledger.work_board_each(&ledger.now()?, |b| {
        let repo = b.item.repo_slug.as_ref().map(|r| r.to_lowercase());
        if want.is_none() || repo == want {
            let (commit, files) = match b.handoff {
                Some(h) => (h.handoff.commit, h.handoff.changed_files),
                None => (None, Vec::new()),
            };
            lanes.push(Lane {
                id: b.item.id,
                repo,
                worktree: b.item.worktree_path,
                base_sha: b.item.base_sha,
                commit,
                files,
            });
        }
        Ok::<_, Infallible>(())
    })?;

    let trees = worktrees(&lanes);
    let mut files: BTreeMap<(&str, String), Vec<Evidence>> = BTreeMap::new();
    let mut report = Vec::new();
    for (lane, tree) in lanes.iter().zip(trees) {
        let mut row = Tree {
            id: lane.id,
            repo: lane.repo.clone(),
            worktree: "ok",
            error: None,
            paths: 0,
            cut: 0,
        };
        // A lane with no repo is never compared: the same path in two
        // unknown repos is not a clash.
        let Some(repo) = lane.repo.as_deref() else {
            row.worktree = "no_repo";
            report.push(row);
            continue;
        };
        let wt = lane.worktree.as_deref();
        for path in &lane.files {
            add(
                &mut files,
                repo,
                norm(path, wt),
                lane.id,
                "handoff",
                &lane.commit,
            );
        }
        match tree {
            Git::Ok { head, paths, cut } => {
                row.paths = paths.len();
                row.cut = cut;
                for path in paths {
                    add(
                        &mut files,
                        repo,
                        path,
                        lane.id,
                        "worktree",
                        &Some(head.clone()),
                    );
                }
            }
            Git::Skip(status) => row.worktree = status,
            Git::Err(status, e) => {
                row.worktree = status;
                row.error = Some(e);
            }
        }
        report.push(row);
    }

    let mut rows: Vec<_> = files
        .into_iter()
        .map(|((repo, path), lanes)| {
            let mut ids: Vec<_> = lanes.iter().map(|e| e.id).collect();
            ids.dedup();
            json!({ "repo": repo, "path": path, "shared": ids.len() > 1, "lanes": lanes })
        })
        .collect();
    // Stable, so each group keeps repo and path order.
    rows.sort_by_key(|r| r["shared"] != true);
    let files_omitted = rows.len().saturating_sub(ROWS);
    rows.truncate(ROWS);
    one(json!({ "files": rows, "files_omitted": files_omitted, "lanes": report }))
}

/// One row's evidence, kept in lane order with handoff before worktree,
/// and once per lane and source.
fn add<'a>(
    files: &mut BTreeMap<(&'a str, String), Vec<Evidence>>,
    repo: &'a str,
    path: String,
    id: i64,
    source: &'static str,
    commit: &Option<String>,
) {
    if path.is_empty() {
        return;
    }
    let row = files.entry((repo, path)).or_default();
    if !row.iter().any(|e| e.id == id && e.source == source) {
        row.push(Evidence {
            id,
            source,
            commit: commit.clone(),
        });
    }
}

/// A handoff path as git names it: less a leading `./` or the worktree.
/// The board's SQL does the same.
fn norm(path: &str, wt: Option<&str>) -> String {
    let p = path.strip_prefix("./").unwrap_or(path);
    wt.and_then(|wt| p.strip_prefix(wt)?.strip_prefix('/'))
        .unwrap_or(p)
        .to_string()
}

enum Git {
    Ok {
        head: String,
        paths: Vec<String>,
        cut: usize,
    },
    Skip(&'static str),
    Err(&'static str, String),
}

/// Every lane's worktree, in lane order, from at most [`WORKERS`] threads.
/// A lane not started by the deadline is `timed_out`.
fn worktrees(lanes: &[Lane]) -> Vec<Git> {
    let deadline = Instant::now() + DEADLINE;
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<Git>>> = Mutex::new((0..lanes.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(lanes.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(lane) = lanes.get(i) else { break };
                    let git = tree(lane, deadline);
                    if let Ok(mut out) = out.lock() {
                        out[i] = Some(git);
                    }
                }
            });
        }
    });
    out.into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|g| g.unwrap_or(Git::Skip("timed_out")))
        .collect()
}

fn tree(lane: &Lane, deadline: Instant) -> Git {
    let (Some(_), Some(wt)) = (&lane.repo, &lane.worktree) else {
        return Git::Skip("none");
    };
    let wt = Path::new(wt);
    if !wt.is_dir() {
        return Git::Skip("missing");
    }
    let run = |args: &[&str]| git(wt, args, deadline);
    let result = (|| {
        let head = String::from_utf8_lossy(&run(&["rev-parse", "HEAD"])?.0)
            .trim()
            .to_string();
        let mut base = None;
        for r in BASES {
            match run(&["merge-base", "HEAD", r]) {
                Ok((out, _)) => {
                    base = Some(String::from_utf8_lossy(&out).trim().to_string());
                    break;
                }
                Err(Failure::TimedOut) => return Err(Failure::TimedOut),
                Err(_) => {}
            }
        }
        let Some(base) = base.or_else(|| lane.base_sha.clone()) else {
            return Ok(None);
        };
        let (diff, diff_cut) = run(&["diff", "--name-only", "--no-renames", "-z", &base, "--"])?;
        let (new, new_cut) = run(&["ls-files", "-o", "--exclude-standard", "-z"])?;
        let mut paths = Vec::new();
        let mut cut = 0;
        for (out, was_cut) in [(diff, diff_cut), (new, new_cut)] {
            let mut names: Vec<&[u8]> = out.split(|b| *b == 0).filter(|n| !n.is_empty()).collect();
            // The last name may be a stub when the output was cut.
            if was_cut && names.pop().is_some() {
                cut += 1;
            }
            for n in names {
                if paths.len() < PATHS {
                    paths.push(String::from_utf8_lossy(n).into_owned());
                } else {
                    cut += 1;
                }
            }
        }
        Ok(Some((head, paths, cut)))
    })();
    match result {
        Ok(Some((head, paths, cut))) => Git::Ok { head, paths, cut },
        Ok(None) => Git::Skip("no_base"),
        Err(Failure::TimedOut) => Git::Skip("timed_out"),
        Err(Failure::Other(e)) => Git::Err("error", e),
    }
}

enum Failure {
    TimedOut,
    Other(String),
}

/// One git call in `wt`, never taking the worktree's index lock (it
/// belongs to the lane) and never prompting.
fn git(wt: &Path, args: &[&str], deadline: Instant) -> Result<(Vec<u8>, bool), Failure> {
    let done = child::run(
        Command::new("git")
            .arg("-C")
            .arg(wt)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0"),
        deadline,
        OUT_BYTES,
    )
    .map_err(|e| match e {
        child::Failed::TimedOut => Failure::TimedOut,
        child::Failed::Spawn(e) | child::Failed::Wait(e) => Failure::Other(format!("git: {e}")),
    })?;
    if !done.status.success() {
        let err = String::from_utf8_lossy(&done.err);
        let line: String = err.lines().next().unwrap_or("").chars().take(300).collect();
        return Err(Failure::Other(format!("git {}: {line}", args[0])));
    }
    Ok((done.out, done.cut))
}
