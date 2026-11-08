//! The overlap thread: `chaperone overlap`'s answer
//! (`chaperone_core::overlap`) for every open lane, from its own read-only
//! ledger, so the git it runs never holds up the board. Read again when
//! the ledger changes or a lane's worktree moves its HEAD (a commit,
//! checkout or reset touches its reflog, one stat a lane, no process), at
//! most every [`SOON`]: git costs about 13 ms of CPU a lane.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chaperone_core::ledger::{Ledger, LedgerError};
use chaperone_core::overlap::{Overlap, overlap};

use crate::diff::reflog_time;
use crate::watch::Shown;

/// A poll is one pragma and a stat per worktree.
const POLL: Duration = Duration::from_secs(1);
const SOON: Duration = Duration::from_secs(5);

/// What a read was taken at: the ledger's version, and each open lane's
/// worktree with its reflog's time.
#[derive(Default, PartialEq)]
struct At {
    version: i64,
    reflogs: Vec<(String, Option<SystemTime>)>,
}

/// One lane's marker: the lanes it shares paths with, those paths with
/// where each lane's side came from, its worktree's state when git could
/// not read it (`missing`, `no_base`, `timed_out`, `error`, or `no_repo`:
/// never compared), and the worktree paths past the library's cap, which
/// were not compared.
#[derive(Default)]
pub struct Marker {
    pub others: Vec<i64>,
    pub paths: Vec<String>,
    pub worktree: Option<String>,
    pub cut: usize,
}

/// Every lane's marker as of one read, or why there is none.
pub type Markers = Result<BTreeMap<i64, Marker>, String>;

/// Starts the thread. `changed` runs after each change to `shown.overlap`.
pub fn spawn(path: PathBuf, shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let mut ledger = None;
        let mut seen: Option<At> = None;
        let mut ran: Option<Instant> = None;
        loop {
            match read(&path, &mut ledger, seen.as_ref(), ran) {
                Ok(None) => {}
                Ok(Some((at, found))) => {
                    seen = Some(at);
                    ran = Some(Instant::now());
                    shown.lock().unwrap().overlap = Some(Ok(markers(&found)));
                    changed();
                }
                Err(e) => {
                    // Reopened next poll: the file may be migrated or created.
                    ledger = None;
                    seen = None;
                    shown.lock().unwrap().overlap = Some(Err(e.to_string()));
                    changed();
                }
            }
            thread::sleep(POLL);
        }
    });
}

/// A new overlap when the ledger or a worktree moved since `seen` and
/// [`SOON`] passed since the last.
fn read(
    path: &Path,
    ledger: &mut Option<Ledger>,
    seen: Option<&At>,
    ran: Option<Instant>,
) -> Result<Option<(At, Overlap)>, LedgerError> {
    let ledger = match ledger {
        Some(l) => l,
        None => ledger.insert(Ledger::open_read_only(path)?),
    };
    let version = ledger.data_version()?;
    if let Some(seen) = seen {
        let moved = seen
            .reflogs
            .iter()
            .any(|(wt, time)| reflog_time(wt) != *time);
        if seen.version == version && !moved || ran.is_some_and(|t| t.elapsed() < SOON) {
            return Ok(None);
        }
    }
    let mut reflogs = Vec::new();
    let Ok(()) = ledger.work_board_each(&ledger.now()?, |b| {
        if let Some(wt) = b.item.worktree_path {
            let time = reflog_time(&wt);
            reflogs.push((wt, time));
        }
        Ok::<_, Infallible>(())
    })?;
    // Reflogs before git: a commit after this moves one, and reads again.
    let found = overlap(ledger, None, false)?;
    Ok(Some((At { version, reflogs }, found)))
}

/// Each lane's marker, from the shared rows and the lanes' git states.
/// Every lane read has one, so a lane without is one not read yet.
fn markers(found: &Overlap) -> BTreeMap<i64, Marker> {
    let mut out: BTreeMap<i64, Marker> = BTreeMap::new();
    for tree in &found.lanes {
        let marker = out.entry(tree.id).or_default();
        marker.cut = tree.cut;
        if !matches!(tree.worktree, "ok" | "none") {
            marker.worktree = Some(match &tree.error {
                Some(e) => format!("{}: {e}", tree.worktree),
                None => tree.worktree.into(),
            });
        }
    }
    for row in found.files.iter().filter(|r| r.shared) {
        let sides: Vec<String> = row
            .lanes
            .iter()
            .map(|e| format!("#{} {}", e.id, e.source))
            .collect();
        let line = format!("{} ({})", row.path, sides.join(", "));
        for e in &row.lanes {
            let marker = out.entry(e.id).or_default();
            if marker.paths.last() != Some(&line) {
                marker.paths.push(line.clone());
            }
            for other in row.lanes.iter().filter(|o| o.id != e.id) {
                if !marker.others.contains(&other.id) {
                    marker.others.push(other.id);
                }
            }
        }
    }
    for marker in out.values_mut() {
        marker.others.sort_unstable();
    }
    out
}
