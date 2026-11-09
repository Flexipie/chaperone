//! The watcher, as in chaperone-ui's `watch.rs`: its own read-only ledger,
//! polled for `PRAGMA data_version`, and a new board only when that
//! changes. The shown lane's page, its stats row and its repo median are
//! read on the same connection, keyed on the version and the lane. The poll
//! loop runs on a background thread and wakes GPUI's main thread only with
//! something to show: a new board, a new page or a new error.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chaperone_core::ledger::{Ledger, LedgerError};
use chaperone_core::stats::Cohort;
use chaperone_core::work::{LaneBoard, LanePage};
use gpui::{AsyncApp, WeakEntity};
use serde_json::Value;

/// A poll is one pragma on an open connection; a commit shows within this.
const POLL: Duration = Duration::from_millis(100);

/// The repo median folds 30 days of merges: read again at most this often,
/// as chaperone-ui does.
const MEDIAN_EVERY: Duration = Duration::from_secs(60);

/// What a poll read: a new board, or the error that closed the ledger.
pub type Read = Result<LaneBoard, String>;

/// The lane whose page the view shows, as the board names it; the view
/// sets it, the poll loop reads it every tick.
#[derive(Clone)]
pub struct Want {
    pub id: i64,
    pub key: Option<String>,
    pub repo: Option<String>,
}

pub type Wanted = Arc<Mutex<Option<Want>>>;

/// Lane `id`'s page as read at one version.
pub struct Page {
    pub id: i64,
    pub read: Result<LanePage, String>,
    /// The lane's `stats` row.
    pub stats: Result<Value, String>,
    /// The repo and its median, None when not read again this time.
    pub median: Option<(String, Result<Value, String>)>,
}

/// Handed between background tasks.
#[derive(Default)]
struct State {
    ledger: Option<Ledger>,
    /// The data_version shown.
    seen: Option<i64>,
    /// The error shown, so a ledger that stays broken redraws once.
    said: Option<String>,
    /// When the last poll started, so polls stay POLL apart across a
    /// hand-over.
    last: Option<Instant>,
    /// The (version, lane) of the page last read.
    page: Option<(i64, i64)>,
    /// The repo median last read and when.
    median: Option<(String, Instant)>,
}

/// Polls `path` until `view` is gone, calling `got` with each new board or
/// error and `paged` with each new page of the lane `wanted` names; both
/// ask for the redraw.
pub async fn run<V: 'static>(
    path: PathBuf,
    view: WeakEntity<V>,
    cx: &mut AsyncApp,
    wanted: Wanted,
    got: impl Fn(&mut V, Read, &mut gpui::Context<V>) + 'static,
    paged: impl Fn(&mut V, Page, &mut gpui::Context<V>) + 'static,
) {
    let ex = cx.background_executor().clone();
    let mut state = State::default();
    loop {
        let (path, timer, wanted) = (path.clone(), ex.clone(), wanted.clone());
        let (back, (read, page)) = ex
            .spawn(async move {
                let mut s = state;
                if let Some(last) = s.last {
                    timer.timer(POLL.saturating_sub(last.elapsed())).await;
                }
                loop {
                    let start = Instant::now();
                    s.last = Some(start);
                    let want = wanted.lock().unwrap().clone();
                    match poll(&path, &mut s, want) {
                        Ok((None, None)) => {}
                        Ok((board, page)) => return (s, (board.map(Ok), page)),
                        Err(e) => {
                            // Reopened next tick: the file may be migrated or created.
                            let said = s.said.take();
                            s = State {
                                last: s.last,
                                ..State::default()
                            };
                            let text = e.to_string();
                            if said.as_ref() != Some(&text) {
                                s.said = Some(text.clone());
                                return (s, (Some(Err(text)), None));
                            }
                            s.said = said;
                        }
                    }
                    timer.timer(POLL.saturating_sub(start.elapsed())).await;
                }
            })
            .await;
        state = back;
        let shown = view.update(cx, |v, cx| {
            if let Some(read) = read {
                got(v, read, cx);
            }
            if let Some(page) = page {
                paged(v, page, cx);
            }
        });
        if shown.is_err() {
            return;
        }
    }
}

/// A new board when the ledger changed since the last one, and the wanted
/// lane's page when the version or the lane changed since the last page.
fn poll(
    path: &Path,
    s: &mut State,
    want: Option<Want>,
) -> Result<(Option<LaneBoard>, Option<Page>), LedgerError> {
    let ledger = match &mut s.ledger {
        Some(l) => l,
        None => s.ledger.insert(Ledger::open_read_only(path)?),
    };
    let version = ledger.data_version()?;
    let board = match s.seen == Some(version) {
        true => None,
        false => {
            let b = ledger.lane_board(&ledger.now()?)?;
            s.seen = Some(b.data_version);
            s.said = None;
            Some(b)
        }
    };
    let Some(w) = want.filter(|w| s.page != Some((version, w.id))) else {
        return Ok((board, None));
    };
    s.page = Some((version, w.id));
    let read = ledger
        .lane_page(w.id, w.key.as_deref())
        .map_err(|e| e.to_string());
    let stats = match ledger.stats(&Cohort::Work(vec![w.id])) {
        Ok(Ok(mut v)) => Ok(v["lanes"][0].take()),
        Ok(Err(refused)) => Err(format!("{refused:?}")),
        Err(e) => Err(e.to_string()),
    };
    let median = match (&w.repo, &s.median) {
        (None, _) => None,
        (Some(r), Some((m, at))) if r == m && at.elapsed() < MEDIAN_EVERY => None,
        (Some(r), _) => {
            s.median = Some((r.clone(), Instant::now()));
            Some((r.clone(), ledger.repo_median(r).map_err(|e| e.to_string())))
        }
    };
    let page = Page {
        id: w.id,
        read,
        stats,
        median,
    };
    Ok((board, Some(page)))
}
