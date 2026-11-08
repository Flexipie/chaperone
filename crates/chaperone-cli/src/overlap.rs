//! `chaperone overlap`: per file, the open lanes touching it. The ledger
//! read is the board's; git runs in each worktree only after it has ended
//! (ADR 0007: a ledger read runs no git), within one deadline.

use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chaperone_core::ledger::Ledger;
use serde::Serialize;

use crate::cli::Format;
use chaperone_core::git::{Failure, git, git_bin};
use crate::output::{self, CliError, Meta, Outcome};

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

// Fields in sorted order: the bytes `render_ok` gives (output::write_one).
#[derive(Serialize)]
struct Evidence {
    commit: Option<String>,
    id: i64,
    source: &'static str,
}

#[derive(Serialize)]
struct Row<'a> {
    lanes: Vec<Evidence>,
    path: String,
    repo: &'a str,
    shared: bool,
}

#[derive(Serialize)]
struct Data<'a> {
    files: Vec<Row<'a>>,
    files_omitted: usize,
    lanes: Vec<Tree>,
}

/// What git said for one lane: `ok`, `none` (no worktree recorded),
/// `missing`, `no_base`, `timed_out`, `error`, or `no_repo` (not compared).
#[derive(Serialize)]
struct Tree {
    cut: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    id: i64,
    paths: usize,
    repo: Option<String>,
    worktree: &'static str,
}

pub fn run(
    ledger: &Ledger,
    repo: Option<String>,
    uncommitted: bool,
    meta: &Meta,
    format: Format,
) -> Result<Outcome, CliError> {
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

    let trees = worktrees(&lanes, uncommitted);
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
            Git::Err(e) => {
                row.worktree = "error";
                row.error = Some(e);
            }
        }
        report.push(row);
    }

    let mut rows: Vec<_> = files
        .into_iter()
        .map(|((repo, path), lanes)| Row {
            shared: lanes.iter().any(|e| e.id != lanes[0].id),
            lanes,
            path,
            repo,
        })
        .collect();
    // Stable, so each group keeps repo and path order.
    rows.sort_by_key(|r| !r.shared);
    let files_omitted = rows.len().saturating_sub(ROWS);
    rows.truncate(ROWS);
    let data = Data {
        files: rows,
        files_omitted,
        lanes: report,
    };
    output::write_one(&data, meta, format)
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
    Err(String),
}

/// Every lane's worktree, in lane order, from at most [`WORKERS`] threads.
/// A lane not started by the deadline is `timed_out`.
fn worktrees(lanes: &[Lane], uncommitted: bool) -> Vec<Git> {
    let deadline = Instant::now() + DEADLINE;
    let bin = git_bin(deadline);
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<Git>>> = Mutex::new((0..lanes.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(lanes.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(lane) = lanes.get(i) else { break };
                    let git = tree(&bin, lane, deadline, uncommitted);
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

/// One git call: the paths HEAD's own commits touch since the merge-base
/// with the first base found, newest commit (HEAD) first. It reads commits
/// only. With `uncommitted`, a second call adds the worktree's uncommitted
/// and untracked paths, which stats every tracked file.
fn tree(bin: &Path, lane: &Lane, deadline: Instant, uncommitted: bool) -> Git {
    let (Some(_), Some(wt)) = (&lane.repo, &lane.worktree) else {
        return Git::Skip("none");
    };
    let wt = Path::new(wt);
    if !wt.is_dir() {
        return Git::Skip("missing");
    }
    let run = |args: &[&str]| git(bin, wt, args, deadline, OUT_BYTES);
    let result = (|| {
        let mut committed = None;
        for base in BASES.iter().copied().chain(lane.base_sha.as_deref()) {
            let range = format!("{base}..HEAD");
            // `--end-of-options`: a claimed base_sha is not checked as hex.
            let args = [
                "log",
                "--no-renames",
                "--no-show-signature",
                "--name-only",
                "-z",
                "--format=%x01%H",
                "--end-of-options",
                &range,
                "--",
            ];
            match run(&args) {
                Ok(out) => {
                    committed = Some(out);
                    break;
                }
                // A base this clone lacks: try the next one.
                Err(Failure::BadRevision) => {}
                Err(e) => return Err(e),
            }
        }
        let Some(committed) = committed else {
            return Ok(None);
        };
        let mut outs = vec![(true, committed)];
        if uncommitted {
            let args = [
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--no-renames",
                "--untracked-files=all",
            ];
            outs.push((false, run(&args)?));
        }
        let mut head = String::new();
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        let mut cut = 0;
        for (log, (out, was_cut)) in outs {
            let mut entries: Vec<&[u8]> =
                out.split(|b| *b == 0).filter(|n| !n.is_empty()).collect();
            // The last entry may be a stub when the output was cut.
            if was_cut && entries.pop().is_some() {
                cut += 1;
            }
            for e in entries {
                let e = String::from_utf8_lossy(e);
                // In the log, a commit's paths follow its header's newline.
                let e = if log { e.trim_start_matches('\n') } else { &e };
                let oid = match log {
                    true => e.strip_prefix('\u{1}'),
                    false => e.strip_prefix("# branch.oid "),
                };
                if let Some(oid) = oid {
                    if head.is_empty() {
                        head = oid.to_string();
                    }
                    continue;
                }
                let path = match (log, e.as_bytes().first()) {
                    (_, None) => None,
                    (true, _) => Some(e),
                    // porcelain v2: the path is the last field.
                    (false, Some(b'1')) => e.splitn(9, ' ').nth(8),
                    (false, Some(b'u')) => e.splitn(11, ' ').nth(10),
                    (false, Some(b'?')) => e.get(2..),
                    _ => None,
                };
                match path {
                    Some(p) if seen.contains(p) => {}
                    Some(p) if paths.len() < PATHS => {
                        seen.insert(p.to_string());
                        paths.push(p.to_string());
                    }
                    Some(_) => cut += 1,
                    None => {}
                }
            }
        }
        Ok(Some((head, paths, cut)))
    })();
    match result {
        Ok(Some((head, paths, cut))) => Git::Ok { head, paths, cut },
        Ok(None) => Git::Skip("no_base"),
        Err(Failure::TimedOut) => Git::Skip("timed_out"),
        Err(Failure::BadRevision) => Git::Skip("no_base"),
        Err(Failure::Other(e)) => Git::Err(e),
    }
}
