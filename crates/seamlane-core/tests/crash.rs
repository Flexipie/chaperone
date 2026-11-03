//! R14 crash tests: a child process is killed with `SIGKILL` inside the
//! write function, and the ledger it leaves has integrity, no trace of the
//! uncommitted event, and tables that equal the folded log.

mod common;

use std::os::unix::process::ExitStatusExt;
use std::time::Duration;

use common::{AT, actor, assert_consistent, committed, create_lane, keys, read_only, toggle};
use rusqlite::Connection;
use seamlane_core::ledger::Ledger;

/// Stalls a lane event's fold at its last statement, the `head_event_id`
/// update (fold.rs `fold_lane`), so the write sits after the insert and the
/// fold and before `COMMIT`. A cross join of 10^12 rows, not a recursive
/// CTE, which SQLite documents as unsupported in triggers.
const STALL: &str = "
    CREATE TABLE burn (n);
    WITH RECURSIVE c(n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM c WHERE n < 999)
    INSERT INTO burn SELECT n FROM c;
    CREATE TRIGGER stall AFTER UPDATE OF head_event_id ON lanes BEGIN
        SELECT count(*) FROM burn a, burn b, burn c, burn d;
    END;";

fn write_locked(db: &Connection) -> bool {
    match db.execute_batch("BEGIN IMMEDIATE; ROLLBACK") {
        Ok(()) => false,
        Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) => true,
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn sigkill_before_commit() {
    if let Some((path, _)) = common::child() {
        let ledger = Ledger::open(&path).unwrap();
        ledger
            .apply(&actor("0"), Some("stalled"), AT, toggle(1, 1))
            .unwrap();
        panic!("the stall trigger returned");
    }
    let (_dir, path, ledger) = common::ledger();
    let lane = committed(
        ledger
            .apply(&actor("0"), Some("seed-0"), AT, create_lane("L"))
            .unwrap(),
    );
    let lane = lane.lane_id.unwrap();
    committed(
        ledger
            .apply(&actor("0"), Some("seed-1"), AT, toggle(lane, 0))
            .unwrap(),
    );
    drop(ledger);
    let db = Connection::open(&path).unwrap();
    // The lock probe must not wait out rusqlite's default 5 s busy_timeout.
    db.busy_timeout(Duration::ZERO).unwrap();
    db.execute_batch(STALL).unwrap();

    let mut child = common::spawn("sigkill_before_commit", &path, "");
    common::wait_for(Duration::from_secs(10), || write_locked(&db));
    // The insert and fold take well under a millisecond; the stall does not end.
    std::thread::sleep(Duration::from_millis(300));
    assert!(write_locked(&db), "the child left the write function");
    assert_eq!(child.kill().signal(), Some(9));

    assert_eq!(keys(&read_only(&path)), ["seed-0", "seed-1"]);
    db.execute_batch("DROP TRIGGER stall; DROP TABLE burn")
        .unwrap();
    assert_consistent(&path);
}

#[test]
fn sigkill_in_commit_loop() {
    if let Some((path, _)) = common::child() {
        let ledger = Ledger::open(&path).unwrap();
        for n in 0.. {
            let key = format!("k{}", n + 1);
            committed(
                ledger
                    .apply(&actor("0"), Some(&key), AT, toggle(1, n))
                    .unwrap(),
            );
        }
    }
    let (_dir, path, ledger) = common::ledger();
    committed(
        ledger
            .apply(&actor("0"), Some("k0"), AT, create_lane("L"))
            .unwrap(),
    );
    drop(ledger);

    let mut child = common::spawn("sigkill_in_commit_loop", &path, "");
    let db = read_only(&path);
    common::wait_for(Duration::from_secs(15), || {
        common::count(&db, "SELECT count(*) FROM events") >= 200
    });
    assert_eq!(child.kill().signal(), Some(9));

    // The committed prefix: distinct keys, none skipped.
    let keys = keys(&db);
    eprintln!("{} events committed before the kill", keys.len());
    let expected: Vec<_> = (0..keys.len()).map(|n| format!("k{n}")).collect();
    assert_eq!(keys, expected);
    assert_consistent(&path);
}
