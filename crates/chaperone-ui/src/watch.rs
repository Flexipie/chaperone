//! The watcher thread: its own read-only ledger, polled for
//! `PRAGMA data_version`, and a new board only when that changes; the
//! selected lane's page too when it changes or another lane is picked,
//! then the board's stats and the lane's repo median, after the board is
//! shown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chaperone_core::ledger::{Ledger, LedgerError};
use chaperone_core::stats::BoardStats;
use chaperone_core::work::{LaneBoard, LanePage, WorkItem};
use serde_json::Value;

use crate::areas::Areas;
use crate::backlog;
use crate::diff::Diff;
use crate::graph::Repos;
use crate::overlap::Markers;

/// A poll is one pragma on an open connection, so idle costs next to
/// nothing; a commit shows within this.
const POLL: Duration = Duration::from_millis(100);
/// Stats replay every open lane's events (68 ms at R14's U2 size), so a
/// ledger written every poll reads them at most this often.
const STATS_EVERY: Duration = Duration::from_secs(1);
/// The repo median folds 30 days of merges: read again for a new repo
/// at once, for new writes at most this often.
const MEDIAN_EVERY: Duration = Duration::from_secs(60);

/// What the window draws: the newest board, or why there is none, and
/// the selected lane's page, diff, files and repo median. The window sets
/// `selected` and `refresh`; each is drawn only for the lane (or repo) it
/// was read for.
#[derive(Default)]
pub struct Shown {
    pub board: Option<LaneBoard>,
    pub error: Option<String>,
    pub selected: Option<Selected>,
    /// Bumped by the diff's Refresh button.
    pub refresh: u64,
    pub detail: Option<Detail>,
    pub diff: Option<Diff>,
    /// Each declaring lane's areas against its diff, by lane id.
    pub areas: HashMap<i64, Areas>,
    /// The cohort's date the window asks for; None is the default.
    pub since: Option<String>,
    pub stats: Option<BoardStats>,
    /// The selected lane's repo median (`repo_median`), or why there is none.
    pub median: Option<Median>,
    pub overlap: Option<Markers>,
    /// Set by the window: the graph is read only while it is shown.
    pub graph_open: bool,
    /// Each repo's change graph with its slug, or why there is none.
    pub graph: Option<Result<Repos, String>>,
    /// Set by the window: the repo whose Backlog is shown, as the board
    /// spells it; daisy's mirror is read only then.
    pub backlog_for: Option<String>,
    pub backlog: Option<backlog::Read>,
}

/// One lane's page as read at `version`, or why it could not be. A
/// failed read shows on the page and is tried again on the next change,
/// never every poll.
pub struct Detail {
    pub lane: i64,
    version: i64,
    pub read: Result<LanePage, String>,
}

/// A repo's median claim to merge as read at `version`.
pub struct Median {
    pub repo: String,
    version: i64,
    pub read: Result<Value, String>,
}

/// The lane picked, with what its page, diff, files and median read.
#[derive(Clone, Debug, PartialEq)]
pub struct Selected {
    pub id: i64,
    pub key: Option<String>,
    pub worktree: Option<String>,
    pub base_sha: Option<String>,
    pub repo: Option<String>,
}

impl Selected {
    pub fn of(item: &WorkItem) -> Self {
        Selected {
            id: item.id,
            key: item.source_key.clone(),
            worktree: item.worktree_path.clone(),
            base_sha: item.base_sha.clone(),
            repo: item.repo_slug.clone(),
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
        // The (data_version, since asked) of the stats shown, and when.
        let mut counted: Option<(i64, Option<String>)> = None;
        let mut counted_at: Option<Instant> = None;
        let mut median_at: Option<Instant> = None;
        loop {
            let start = Instant::now();
            let (want, since) = {
                let s = shown.lock().unwrap();
                (s.selected.clone(), s.since.clone())
            };
            // Stats are read after the board is drawn: they replay every
            // open lane's events, and the board should not wait for that.
            let polled = poll(&path, &mut ledger, seen, loaded, want.as_ref()).and_then(|got| {
                publish(&shown, &mut seen, &mut loaded, got, &changed);
                let ledger = ledger.as_ref().expect("poll opened it");
                let version = ledger.data_version()?;
                // The throttle is for writes: a new date reads at once.
                let new_date = counted.as_ref().map(|c| &c.1) != Some(&since);
                if counted.as_ref() != Some(&(version, since.clone()))
                    && (new_date || counted_at.is_none_or(|t| t.elapsed() >= STATS_EVERY))
                {
                    let stats = ledger.board_stats(since.as_deref())?;
                    counted = Some((stats.data_version, since));
                    counted_at = Some(Instant::now());
                    shown.lock().unwrap().stats = Some(stats);
                    changed();
                }
                let repo = want.and_then(|l| l.repo);
                let stale = {
                    let s = shown.lock().unwrap();
                    match (&repo, &s.median) {
                        (None, _) => false,
                        (Some(r), Some(m)) if &m.repo == r => {
                            m.version != version
                                && median_at.is_none_or(|t| t.elapsed() >= MEDIAN_EVERY)
                        }
                        _ => true,
                    }
                };
                if let (true, Some(repo)) = (stale, repo) {
                    let read = ledger.repo_median(&repo).map_err(|e| e.to_string());
                    median_at = Some(Instant::now());
                    shown.lock().unwrap().median = Some(Median {
                        repo,
                        version,
                        read,
                    });
                    changed();
                }
                Ok(())
            });
            match polled {
                Ok(()) => {}
                Err(e) => {
                    // Reopened next tick: the file may be migrated or created.
                    ledger = None;
                    seen = None;
                    loaded = None;
                    counted = None;
                    let text = e.to_string();
                    let mut s = shown.lock().unwrap();
                    if s.error.as_deref() != Some(text.as_str()) {
                        s.error = Some(text);
                        drop(s);
                        changed();
                    }
                }
            }
            // A commit during a stats read waits for that read, not a
            // whole poll more.
            thread::sleep(POLL.saturating_sub(start.elapsed()));
        }
    });
}

/// Shows what `poll` read, if anything.
fn publish(
    shown: &Mutex<Shown>,
    seen: &mut Option<i64>,
    loaded: &mut Option<(i64, i64)>,
    (board, detail): (Option<LaneBoard>, Option<Detail>),
    changed: &impl Fn(),
) {
    if board.is_none() && detail.is_none() {
        return;
    }
    let mut s = shown.lock().unwrap();
    if let Some(board) = board {
        *seen = Some(board.data_version);
        s.board = Some(board);
    }
    // Dropped if another lane was picked meanwhile.
    if let Some(d) = detail
        && s.selected.as_ref().map(|l| l.id) == Some(d.lane)
    {
        *loaded = Some((d.version, d.lane));
        s.detail = Some(d);
    }
    s.error = None;
    drop(s);
    changed();
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
            let read = ledger.lane_page(l.id, l.key.as_deref());
            Some(Detail {
                lane: l.id,
                version: read.as_ref().map_or(version, |d| d.detail.data_version),
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

    use chaperone_core::checks::{AttentionInput, CheckInput, MergedInput};
    use chaperone_core::ledger::{Actor, EventKind, Mutation, Payload};
    use chaperone_core::work::{Brief, ClaimInput, CreateInput, HandoffInput, UpdateInput};

    use super::*;

    const LANES: i64 = 50;
    /// Per lane: this many updates and as many checks, so a reload replays
    /// a few hundred events for each lane's stamps.
    const EVENTS: u64 = 100;
    /// Per lane, `usage.reported` events: the bulk of a real ledger.
    const USAGE: u64 = 300;
    const SAMPLES: usize = 50;

    fn p(times: &mut [Duration], q: usize) -> Duration {
        times.sort();
        times[(times.len() - 1) * q / 100]
    }

    /// R14 U2 and the reload cost, on a busy ledger with a spec of 20
    /// decisions, a question per lane, usage on every lane, every other
    /// lane merged into the cohort and lane 1 selected, with a handoff and
    /// local reviews so its page reads them. Release only:
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
            let claim = ClaimInput {
                branch: Some(format!("b{id}")),
                ..ClaimInput::default()
            };
            rw.work_claim(&me, id, &claim, None, false)
                .unwrap()
                .unwrap();
            if id == 1 {
                for job in 1..=5 {
                    let view = serde_json::json!({
                        "job_id": job, "source": "local", "status": "reviewed",
                        "phase": "done", "done": true, "repo": "Flexipie/repo1",
                        "branch": "b1", "head_sha": "abc1234", "score": job,
                        "verdict": "safe", "summary": "s", "findings": [],
                    });
                    rw.review_record(&checker, id, job, &view).unwrap().unwrap();
                }
                let handoff = HandoffInput {
                    summary: "built".into(),
                    commit: Some("abc1234".into()),
                    brief: Some(Brief {
                        goal: "a goal".into(),
                        check_by_hand: vec!["the look".into()],
                        ..Brief::default()
                    }),
                    ..HandoffInput::default()
                };
                rw.work_handoff(&me, id, &handoff, None, false)
                    .unwrap()
                    .unwrap();
            }
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
            let at = rw.now().unwrap();
            for n in 0..USAGE {
                let payload = Payload {
                    source: Some(if n % 3 == 0 { "subagent" } else { "parent" }.into()),
                    rest: serde_json::json!({
                        "message_id": format!("m{n}"), "input_total": 1000 * n as i64,
                        "cache_read": 900 * n as i64, "cache_write": 50, "output_total": 40,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    ..Payload::default()
                };
                let event = Mutation::Event {
                    kind: EventKind::UsageReported,
                    lane_id: Some(id),
                    spec_id: None,
                    payload,
                };
                let key = format!("usage:{id}:m{n}");
                rw.apply(&checker, Some(&key), &at, event).unwrap();
            }
            if id % 2 == 0 {
                let pr = UpdateInput {
                    pr_ref: Some(format!("Flexipie/repo0#{}", 100 + id)),
                    ..UpdateInput::default()
                };
                rw.work_update(&me, id, &pr, None, None, false)
                    .unwrap()
                    .unwrap();
                let merged = MergedInput {
                    work: id,
                    pr: format!("Flexipie/repo0#{}", 100 + id),
                    merge_sha: "fedcba9".into(),
                    head: "abc1234".into(),
                    merged_at: at,
                    patch_id: None,
                };
                rw.work_merged(&checker, &merged).unwrap().unwrap();
            }
        }

        let ro = Ledger::open_read_only(&path).unwrap();
        let now = ro.now().unwrap();
        let mut reloads: Vec<Duration> = (0..20)
            .map(|_| {
                let start = Instant::now();
                assert_eq!(ro.lane_board(&now).unwrap().lanes.len(), LANES as usize);
                let page = ro.lane_page(1, Some("Flexipie/repo0#2")).unwrap();
                assert_eq!(page.reviews.len(), 5);
                start.elapsed()
            })
            .collect();
        let reload = p(&mut reloads, 50);
        let mut counts: Vec<Duration> = (0..20)
            .map(|_| {
                let start = Instant::now();
                let stats = ro.board_stats(None).unwrap();
                assert_eq!(stats.lanes.len(), LANES as usize);
                let cohort = stats.cohort.unwrap();
                assert_eq!(cohort["cohort"]["lanes"], LANES / 2);
                start.elapsed()
            })
            .collect();
        let count = p(&mut counts, 50);
        eprintln!(
            "reload ({LANES} lanes, {} events and {USAGE} usage each): board p50 {reload:?}, \
             stats p50 {count:?}",
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
        // The first board, then its stats.
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut seen = shown.lock().unwrap().board.as_ref().unwrap().data_version;
        let mut lags = Vec::with_capacity(SAMPLES);
        // Each commit follows the last board at once, never waiting for the
        // stats, so some land during a stats read.
        for i in 0..SAMPLES {
            let id = i as i64 % LANES + 1;
            let start = Instant::now();
            rw.work_heartbeat(&lane(&format!("s{id}")), id, None, false)
                .unwrap()
                .unwrap();
            // A stats repaint may come first: the board's is the one that
            // brings a new version.
            let drawn = loop {
                let at = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let version = shown.lock().unwrap().board.as_ref().unwrap().data_version;
                if version != seen {
                    seen = version;
                    break at;
                }
            };
            lags.push(drawn - start);
        }
        let (p50, p99) = (p(&mut lags, 50), p(&mut lags, 99));
        eprintln!("U2 commit to repaint request: p50 {p50:?}, p99 {p99:?}");
        // The stats catch up with the last commit within STATS_EVERY.
        let start = Instant::now();
        while shown.lock().unwrap().stats.as_ref().unwrap().data_version != seen {
            rx.recv_timeout(STATS_EVERY * 2)
                .expect("stats never caught up");
        }
        eprintln!("last commit's board to its stats: {:?}", start.elapsed());
        while rx.try_recv().is_ok() {}
        // Quiet ledger: no new board.
        assert!(rx.recv_timeout(POLL * 5).is_err(), "redrew with no change");
        assert!(p99 <= Duration::from_millis(150), "U2: {p99:?}");
    }
}
