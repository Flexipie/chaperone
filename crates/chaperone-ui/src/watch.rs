//! The watcher thread: its own read-only ledger, polled for
//! `PRAGMA data_version`, and a new board only when that changes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chaperone_core::ledger::{Ledger, LedgerError};
use chaperone_core::work::LaneBoard;

/// A poll is one pragma on an open connection, so idle costs next to
/// nothing; a commit shows within this.
const POLL: Duration = Duration::from_millis(100);

/// What the window draws: the newest board, or why there is none.
#[derive(Default)]
pub struct Shown {
    pub board: Option<LaneBoard>,
    pub error: Option<String>,
}

/// Starts the watcher. `changed` runs after each change to `shown`.
pub fn spawn(path: PathBuf, shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let mut ledger = None;
        let mut seen = None;
        loop {
            match poll(&path, &mut ledger, seen) {
                Ok(None) => {}
                Ok(Some(board)) => {
                    seen = Some(board.data_version);
                    let mut s = shown.lock().unwrap();
                    s.board = Some(board);
                    s.error = None;
                    drop(s);
                    changed();
                }
                Err(e) => {
                    // Reopened next tick: the file may be migrated or created.
                    ledger = None;
                    seen = None;
                    let text = e.to_string();
                    let mut s = shown.lock().unwrap();
                    if s.error.as_deref() != Some(text.as_str()) {
                        s.error = Some(text);
                        drop(s);
                        changed();
                    }
                }
            }
            thread::sleep(POLL);
        }
    });
}

/// A new board when the ledger changed since `seen`, else None.
fn poll(
    path: &Path,
    ledger: &mut Option<Ledger>,
    seen: Option<i64>,
) -> Result<Option<LaneBoard>, LedgerError> {
    let ledger = match ledger {
        Some(l) => l,
        None => ledger.insert(Ledger::open_read_only(path)?),
    };
    if seen.is_some() && seen == Some(ledger.data_version()?) {
        return Ok(None);
    }
    let now = ledger.now()?;
    ledger.lane_board(&now).map(Some)
}
