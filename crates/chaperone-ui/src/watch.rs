//! The watcher thread: its own read-only ledger, polled for
//! `PRAGMA data_version`, and a new board only when that changes; the
//! selected lane's detail too when it changes or another lane is picked.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chaperone_core::ledger::{Ledger, LedgerError};
use chaperone_core::work::{LaneBoard, LaneDetail, WorkItem};

use crate::diff::Diff;

/// A poll is one pragma on an open connection, so idle costs next to
/// nothing; a commit shows within this.
const POLL: Duration = Duration::from_millis(100);

/// What the window draws: the newest board, or why there is none, and
/// the selected lane's detail and diff. The window sets `selected` and
/// `refresh`; a detail or diff is drawn only for the lane it was read for.
#[derive(Default)]
pub struct Shown {
    pub board: Option<LaneBoard>,
    pub error: Option<String>,
    pub selected: Option<Selected>,
    /// Bumped by the diff's Refresh button.
    pub refresh: u64,
    pub detail: Option<Detail>,
    pub diff: Option<Diff>,
}

/// One lane's detail as read at `version`, or why it could not be. A
/// failed read shows in the panel and is tried again on the next change,
/// never every poll.
pub struct Detail {
    pub lane: i64,
    version: i64,
    pub read: Result<LaneDetail, String>,
}

/// The lane picked on the board, with what its detail and diff read.
#[derive(Clone, Debug, PartialEq)]
pub struct Selected {
    pub id: i64,
    pub key: Option<String>,
    pub worktree: Option<String>,
    pub base_sha: Option<String>,
}

impl Selected {
    pub fn of(item: &WorkItem) -> Self {
        Selected {
            id: item.id,
            key: item.source_key.clone(),
            worktree: item.worktree_path.clone(),
            base_sha: item.base_sha.clone(),
        }
    }
}

/// Starts the watcher. `changed` runs after each change to `shown`.
pub fn spawn(path: PathBuf, shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let mut ledger = None;
        let mut seen = None;
        // The (data_version, lane) of the detail shown.
        let mut loaded = None;
        loop {
            let want = shown.lock().unwrap().selected.clone();
            match poll(&path, &mut ledger, seen, loaded, want.as_ref()) {
                Ok((None, None)) => {}
                Ok((board, detail)) => {
                    let mut s = shown.lock().unwrap();
                    if let Some(board) = board {
                        seen = Some(board.data_version);
                        s.board = Some(board);
                    }
                    // Dropped if another lane was picked meanwhile.
                    if let Some(d) = detail
                        && s.selected.as_ref().map(|l| l.id) == Some(d.lane)
                    {
                        loaded = Some((d.version, d.lane));
                        s.detail = Some(d);
                    }
                    s.error = None;
                    drop(s);
                    changed();
                }
                Err(e) => {
                    // Reopened next tick: the file may be migrated or created.
                    ledger = None;
                    seen = None;
                    loaded = None;
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

/// A new board when the ledger changed since `seen`, and `want`'s detail
/// when the ledger or the lane changed since `loaded`.
fn poll(
    path: &Path,
    ledger: &mut Option<Ledger>,
    seen: Option<i64>,
    loaded: Option<(i64, i64)>,
    want: Option<&Selected>,
) -> Result<(Option<LaneBoard>, Option<Detail>), LedgerError> {
    let ledger = match ledger {
        Some(l) => l,
        None => ledger.insert(Ledger::open_read_only(path)?),
    };
    let version = ledger.data_version()?;
    let board = if seen == Some(version) {
        None
    } else {
        Some(ledger.lane_board(&ledger.now()?)?)
    };
    let detail = match want {
        Some(l) if loaded != Some((version, l.id)) => {
            let read = ledger.lane_detail(l.id, l.key.as_deref());
            Some(Detail {
                lane: l.id,
                version: read.as_ref().map_or(version, |d| d.data_version),
                read: read.map_err(|e| e.to_string()),
            })
        }
        _ => None,
    };
    Ok((board, detail))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use chaperone_core::checks::{AttentionInput, CheckInput};
    use chaperone_core::ledger::Actor;
    use chaperone_core::work::{ClaimInput, CreateInput, UpdateInput};

    use super::*;

    const LANES: i64 = 50;
    /// Per lane: this many updates and as many checks, so a reload replays
    /// a few hundred events for each lane's stamps.
    const EVENTS: u64 = 100;
    const SAMPLES: usize = 50;

    fn p(times: &mut [Duration], q: usize) -> Duration {
        times.sort();
        times[(times.len() - 1) * q / 100]
    }

    /// R14 U2 and the reload cost, on a busy ledger with a spec of 20
    /// decisions, a question per lane and lane 1 selected. Release only:
    /// `cargo test --release -p chaperone-ui -- --ignored --nocapture`.
    #[test]
    #[ignore = "release-only budget; run with --ignored"]
    fn commit_to_redraw() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let rw = Ledger::open_and_migrate(&path).unwrap();
        let lane = |s: &str| Actor {
            agent: Some("lane".into()),
            session: Some(s.into()),
        };
        let checker = lane("checker");
        let decisions: String = (0..20)
            .map(|n| format!("- Q: question {n}? A: answer {n}.\n"))
            .collect();
        let body =
            format!("# P\n\n## slice 1: one  (Flexipie/repo0#2)\n\n## decisions\n\n{decisions}");
        rw.spec_create(
            &checker,
            "Flexipie/repo0#1",
            None,
            &body,
            &rw.now().unwrap(),
            false,
        )
        .unwrap()
        .unwrap();
        for id in 1..=LANES {
            let me = lane(&format!("s{id}"));
            let create = CreateInput {
                title: format!("lane {id}"),
                repo: Some(format!("Flexipie/repo{}", id % 5)),
                source: Some("github".into()),
                source_id: Some(format!("Flexipie/repo0#{}", id + 1)),
                spec: Some("Flexipie/repo0#1".into()),
                lane_key: Some(format!("l{id}")),
                kind: "code".into(),
                pr_flow: "pr".into(),
                ..CreateInput::default()
            };
            rw.work_create(&me, &create, None, false).unwrap().unwrap();
            rw.work_claim(&me, id, &ClaimInput::default(), None, false)
                .unwrap()
                .unwrap();
            for n in 0..EVENTS {
                let update = UpdateInput {
                    next_action: Some(format!("step {n}")),
                    ..UpdateInput::default()
                };
                rw.work_update(&me, id, &update, None, None, false)
                    .unwrap()
                    .unwrap();
                let check = CheckInput {
                    work: id,
                    check: "plan_check".into(),
                    head: "abc1234".into(),
                    outcome: "ran".into(),
                    findings: Some(n),
                    ..CheckInput::default()
                };
                rw.check_record(&checker, &check).unwrap().unwrap();
            }
            let ask = AttentionInput {
                work: id,
                kind: "asked".into(),
                by: "owner".into(),
                reason: Some("question.scope".into()),
                note: Some("split it?".into()),
                ..AttentionInput::default()
            };
            rw.attention_record(&checker, &ask).unwrap().unwrap();
        }

        let ro = Ledger::open_read_only(&path).unwrap();
        let now = ro.now().unwrap();
        let mut reloads: Vec<Duration> = (0..20)
            .map(|_| {
                let start = Instant::now();
                assert_eq!(ro.lane_board(&now).unwrap().lanes.len(), LANES as usize);
                ro.lane_detail(1, Some("Flexipie/repo0#2")).unwrap();
                start.elapsed()
            })
            .collect();
        let reload = p(&mut reloads, 50);
        eprintln!(
            "reload ({LANES} lanes, {} events each): p50 {reload:?}",
            2 * EVENTS
        );

        let item = ro.work_get(1, &now).unwrap().unwrap();
        let shown = Arc::new(Mutex::new(Shown {
            selected: Some(Selected::of(&item)),
            ..Shown::default()
        }));
        let (tx, rx) = mpsc::channel();
        spawn(path, shown.clone(), move || {
            tx.send(Instant::now()).unwrap()
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut seen = shown.lock().unwrap().board.as_ref().unwrap().data_version;
        let mut lags = Vec::with_capacity(SAMPLES);
        for i in 0..SAMPLES {
            let id = i as i64 % LANES + 1;
            let start = Instant::now();
            rw.work_heartbeat(&lane(&format!("s{id}")), id, None, false)
                .unwrap()
                .unwrap();
            let drawn = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            lags.push(drawn - start);
            let version = shown.lock().unwrap().board.as_ref().unwrap().data_version;
            assert_ne!(version, seen, "a commit must bring a new board");
            seen = version;
        }
        let (p50, p99) = (p(&mut lags, 50), p(&mut lags, 99));
        eprintln!("U2 commit to repaint request: p50 {p50:?}, p99 {p99:?}");
        // Quiet ledger: no new board.
        assert!(rx.recv_timeout(POLL * 5).is_err(), "redrew with no change");
        assert!(p99 <= Duration::from_millis(150), "U2: {p99:?}");
    }
}
