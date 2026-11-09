//! The watcher, as in chaperone-ui's `watch.rs`: its own read-only ledger,
//! polled for `PRAGMA data_version`, and a new board only when that
//! changes. The poll loop runs on a background thread and wakes GPUI's
//! main thread only with something to show: a new board or a new error.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::ledger::{Ledger, LedgerError};
use chaperone_core::work::LaneBoard;
use gpui::{AsyncApp, WeakEntity};

/// A poll is one pragma on an open connection; a commit shows within this.
const POLL: Duration = Duration::from_millis(100);

/// What a poll read: a new board, or the error that closed the ledger.
pub type Read = Result<LaneBoard, String>;

/// Polls `path` until `view` is gone, calling `got` with each new read;
/// `got` asks for the redraw.
pub async fn run<V: 'static>(
    path: PathBuf,
    view: WeakEntity<V>,
    cx: &mut AsyncApp,
    got: impl Fn(&mut V, Read, &mut gpui::Context<V>) + 'static,
) {
    let ex = cx.background_executor().clone();
    // The ledger, the data_version shown, and the error shown (so a ledger
    // that stays broken redraws once), handed between background tasks.
    let mut state = (None, None, None::<String>);
    loop {
        let (path, timer) = (path.clone(), ex.clone());
        let (back, read) = ex
            .spawn(async move {
                let (mut ledger, mut seen, mut said) = state;
                loop {
                    let start = Instant::now();
                    match poll(&path, &mut ledger, seen) {
                        Ok(None) => {}
                        Ok(Some(board)) => {
                            seen = Some(board.data_version);
                            return ((ledger, seen, None), Ok(board));
                        }
                        Err(e) => {
                            // Reopened next tick: the file may be migrated or created.
                            ledger = None;
                            seen = None;
                            let text = e.to_string();
                            if said.as_ref() != Some(&text) {
                                said = Some(text.clone());
                                return ((ledger, seen, said), Err(text));
                            }
                        }
                    }
                    timer.timer(POLL.saturating_sub(start.elapsed())).await;
                }
            })
            .await;
        state = back;
        if view.update(cx, |v, cx| got(v, read, cx)).is_err() {
            return;
        }
    }
}

/// A new board when the ledger changed since `seen`.
fn poll(
    path: &Path,
    ledger: &mut Option<Ledger>,
    seen: Option<i64>,
) -> Result<Option<LaneBoard>, LedgerError> {
    let ledger = match ledger {
        Some(l) => l,
        None => ledger.insert(Ledger::open_read_only(path)?),
    };
    if seen == Some(ledger.data_version()?) {
        return Ok(None);
    }
    Ok(Some(ledger.lane_board(&ledger.now()?)?))
}
