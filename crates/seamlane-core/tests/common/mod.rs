//! Helpers for the crash and concurrency tests: re-run this test binary as a
//! child process that writes to a ledger, and check the ledger once the
//! children are gone.

// Each test binary uses part of this module.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use seamlane_core::ledger::{Actor, Applied, Change, Event, EventKind, Ledger, Mutation, Payload};
use serde_json::json;

const DB_ENV: &str = "SEAMLANE_TEST_CHILD_DB";
const ARG_ENV: &str = "SEAMLANE_TEST_CHILD_ARG";

pub const AT: &str = "2026-11-03T00:00:00.000Z";

/// The ledger path and argument `spawn` gave this process, when it is a child.
/// A test starts with this check and runs its child role when it is set.
pub fn child() -> Option<(PathBuf, String)> {
    let db = std::env::var_os(DB_ENV)?;
    Some((db.into(), std::env::var(ARG_ENV).unwrap_or_default()))
}

/// A child process, killed and reaped on drop, so a failed assert in the
/// parent leaves nothing running.
pub struct Proc(Child);

impl Proc {
    pub fn wait(&mut self) -> ExitStatus {
        self.0.wait().unwrap()
    }

    /// `SIGKILL` (std `Child::kill`), then the exit status.
    pub fn kill(&mut self) -> ExitStatus {
        self.0.kill().unwrap();
        self.wait()
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs only the test `test` of this binary again, as a child, with `db` and
/// `arg` in its environment.
pub fn spawn(test: &str, db: &Path, arg: &str) -> Proc {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture"])
        .env(DB_ENV, db)
        .env(ARG_ENV, arg)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    Proc(child)
}

/// A fresh ledger in a temp dir.
pub fn ledger() -> (tempfile::TempDir, PathBuf, Ledger) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let ledger = Ledger::open_and_migrate(&path).unwrap();
    (dir, path, ledger)
}

pub fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

pub fn actor(who: &str) -> Actor {
    Actor {
        agent: Some(format!("a{who}")),
        session: Some(format!("s{who}")),
    }
}

pub fn committed(applied: Applied) -> Event {
    match applied {
        Applied::Committed(event) => event,
        other => panic!("not committed: {other:?}"),
    }
}

pub fn create_lane(lane_key: &str) -> Mutation {
    Mutation::Event {
        kind: EventKind::LaneCreated,
        lane_id: None,
        spec_id: None,
        payload: Payload {
            lane_key: Some(lane_key.into()),
            ..Payload::default()
        },
    }
}

/// The `n`th status change of a new lane: queued, then active and blocked
/// in turn.
pub fn toggle(lane_id: i64, n: usize) -> Mutation {
    const TURNS: [&str; 2] = ["active", "blocked"];
    let from = if n == 0 { "queued" } else { TURNS[(n - 1) % 2] };
    let changed = BTreeMap::from([(
        "status".to_string(),
        Change {
            from: json!(from),
            to: json!(TURNS[n % 2]),
        },
    )]);
    Mutation::Event {
        kind: EventKind::LaneUpdated,
        lane_id: Some(lane_id),
        spec_id: None,
        payload: Payload {
            changed,
            ..Payload::default()
        },
    }
}

pub fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Every idempotency key in the log, in commit order.
pub fn keys(db: &Connection) -> Vec<String> {
    let mut stmt = db
        .prepare("SELECT idempotency_key FROM events ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Polls `done` every 10 ms; panics after `limit`.
pub fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "gave up after {limit:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `integrity_check` is ok and the state tables equal the log folded into
/// empty tables. The read-write open comes first: it recovers the WAL a
/// killed writer left.
pub fn assert_consistent(path: &Path) {
    let ledger = Ledger::open(path).unwrap();
    let db = read_only(path);
    let integrity: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    let before = dump(&db);
    ledger.rebuild().unwrap();
    assert_eq!(before, dump(&db), "the tables differ from the folded log");
}

/// Every row of every table, events included, as text.
pub fn dump(db: &Connection) -> BTreeMap<String, Vec<String>> {
    let tables = [
        "events",
        "specs",
        "spec_versions",
        "lanes",
        "gate_results",
        "findings",
        "messages",
    ];
    tables
        .iter()
        .map(|t| {
            let mut stmt = db
                .prepare(&format!("SELECT * FROM {t} ORDER BY 1, 2, 3"))
                .unwrap();
            let n = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    let cells: Vec<_> = (0..n)
                        .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                        .collect();
                    Ok(cells.join("|"))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            (t.to_string(), rows)
        })
        .collect()
}
