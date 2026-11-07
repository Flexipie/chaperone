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
    if seen == Some(ledger.data_version()?) {
        return Ok(None);
    }
    let now = ledger.now()?;
    ledger.lane_board(&now).map(Some)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use chaperone_core::checks::CheckInput;
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

    /// R14 U2 and the reload cost, on a busy ledger. Release only:
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
        for id in 1..=LANES {
            let me = lane(&format!("s{id}"));
            let create = CreateInput {
                title: format!("lane {id}"),
                repo: Some(format!("Flexipie/repo{}", id % 5)),
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
        }

        let ro = Ledger::open_read_only(&path).unwrap();
        let now = ro.now().unwrap();
        let mut reloads: Vec<Duration> = (0..20)
            .map(|_| {
                let start = Instant::now();
                assert_eq!(ro.lane_board(&now).unwrap().lanes.len(), LANES as usize);
                start.elapsed()
            })
            .collect();
        let reload = p(&mut reloads, 50);
        eprintln!(
            "reload ({LANES} lanes, {} events each): p50 {reload:?}",
            2 * EVENTS
        );

        let shown = Arc::new(Mutex::new(Shown::default()));
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
