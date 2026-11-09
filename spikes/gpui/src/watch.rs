//! The watcher, as in chaperone-ui's `watch.rs`: its own read-only ledger,
//! polled for `PRAGMA data_version`, and a new board only when that
//! changes. It runs on GPUI's executors: the reads on a background
//! thread, the board handed to the view on the main one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::ledger::{Ledger, LedgerError};
use chaperone_core::work::LaneBoard;
use gpui::{AsyncApp, WeakEntity};

/// A poll is one pragma on an open connection; a commit shows within this.
const POLL: Duration = Duration::from_millis(100);

/// What a poll read: a new board, or the error that closed the ledger.
pub type Read = Result<LaneBoard, String>;

/// Polls `path` until `view` is gone, calling `got` with each new read.
pub async fn run<V: 'static>(
    path: PathBuf,
    view: WeakEntity<V>,
    cx: &mut AsyncApp,
    got: impl Fn(&mut V, Read, &mut gpui::Context<V>) + 'static,
) {
    let mut ledger = None;
    let mut seen = None;
    // The error shown, so a ledger that stays broken redraws once.
    let mut said: Option<String> = None;
    loop {
        let start = Instant::now();
        let path = path.clone();
        let (back, polled) = cx
            .background_executor()
            .spawn(async move {
                let polled = poll(&path, &mut ledger, seen);
                (ledger, polled)
            })
            .await;
        ledger = back;
        let read = match polled {
            Ok(None) => None,
            Ok(Some(board)) => {
                seen = Some(board.data_version);
                said = None;
                Some(Ok(board))
            }
            Err(e) => {
                // Reopened next tick: the file may be migrated or created.
                ledger = None;
                seen = None;
                let text = e.to_string();
                (said.as_ref() != Some(&text)).then(|| {
                    said = Some(text.clone());
                    Err(text)
                })
            }
        };
        if let Some(read) = read
            && view
                .update(cx, |v, cx| {
                    got(v, read, cx);
                    cx.notify();
                })
                .is_err()
        {
            return;
        }
        cx.background_executor()
            .timer(POLL.saturating_sub(start.elapsed()))
            .await;
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
