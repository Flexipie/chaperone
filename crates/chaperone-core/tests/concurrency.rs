//! R14 concurrency tests: real processes race the write function on one
//! ledger file.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use chaperone_core::ledger::{Applied, Change, EventKind, Ledger, LedgerError, Mutation, Payload};
use common::{AT, actor, assert_consistent, committed, create_lane, read_only, toggle};
use rusqlite::Connection;
use serde_json::{Value, json};

const WRITERS: usize = 50;
const EVENTS: usize = 40;

/// 50 processes, 40 events each: a lane, then status changes on it. The last
/// key goes in from two connections at once; one commits it, the other gets
/// the same event back.
#[test]
fn fifty_writers() {
    if let Some((path, i)) = common::child() {
        let ledger = Ledger::open(&path).unwrap();
        let who = actor(&i);
        let created = ledger
            .apply(&who, Some(&format!("p{i}-0")), AT, create_lane(&i))
            .unwrap();
        let lane = committed(created).lane_id.unwrap();
        for n in 1..EVENTS - 1 {
            let key = format!("p{i}-{n}");
            committed(
                ledger
                    .apply(&who, Some(&key), AT, toggle(lane, n - 1))
                    .unwrap(),
            );
        }
        let barrier = &Barrier::new(2);
        let key = &format!("p{i}-{}", EVENTS - 1);
        let who = &who;
        let [a, b] = std::thread::scope(|s| {
            [ledger, Ledger::open(&path).unwrap()]
                .map(|l| {
                    s.spawn(move || {
                        barrier.wait();
                        l.apply(who, Some(key), AT, toggle(lane, EVENTS - 2))
                            .unwrap()
                    })
                })
                .map(|h| h.join().unwrap())
        });
        match (a, b) {
            (Applied::Committed(c), Applied::Replayed(r))
            | (Applied::Replayed(r), Applied::Committed(c)) => assert_eq!(c, r),
            other => panic!("retry of the last key: {other:?}"),
        }
        return;
    }
    let (_dir, path, ledger) = common::ledger();
    drop(ledger);
    let start = Instant::now();
    let mut children: Vec<_> = (0..WRITERS)
        .map(|i| common::spawn("fifty_writers", &path, &i.to_string()))
        .collect();
    for child in &mut children {
        assert!(child.wait().success());
    }
    eprintln!(
        "{WRITERS} writers x {EVENTS} events in {:?}",
        start.elapsed()
    );
    let db = read_only(&path);
    let total = (WRITERS * EVENTS) as i64;
    assert_eq!(common::count(&db, "SELECT count(*) FROM events"), total);
    assert_eq!(
        common::count(&db, "SELECT count(DISTINCT idempotency_key) FROM events"),
        total
    );
    assert_consistent(&path);
}

/// Why the write function begins `IMMEDIATE`, shown on raw connections: two
/// deferred transactions that read and then both write. Once the first
/// commits, the second gets `SQLITE_BUSY_SNAPSHOT` at once, and
/// `busy_timeout` does not wait for it. `fifty_writers` is what fails if
/// `apply` itself begins deferred.
#[test]
fn deferred_txns_busy_snapshot() {
    let (_dir, path, ledger) = common::ledger();
    committed(
        ledger
            .apply(&actor("0"), None, AT, create_lane("L"))
            .unwrap(),
    );
    let [a, b] = [(); 2].map(|_| {
        let c = Connection::open(&path).unwrap();
        c.busy_timeout(Duration::from_millis(5000)).unwrap();
        c
    });
    for c in [&a, &b] {
        c.execute_batch("BEGIN").unwrap();
        assert_eq!(common::count(c, "SELECT count(*) FROM lanes"), 1);
    }
    a.execute("UPDATE lanes SET branch = 'a'", []).unwrap();
    a.execute_batch("COMMIT").unwrap();

    let start = Instant::now();
    let err = b.execute("UPDATE lanes SET branch = 'b'", []).unwrap_err();
    let elapsed = start.elapsed();
    let rusqlite::Error::SqliteFailure(e, _) = err else {
        panic!("{err}");
    };
    assert_eq!(e.extended_code, 517, "{e}");
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
}

/// Another process holds the write lock past the 5 s busy limit; the write
/// gives up at about 5 s with "database is locked" and writes nothing.
#[test]
fn held_txn_times_out() {
    if let Some((path, _)) = common::child() {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        std::fs::write(marker(&path), "").unwrap();
        // Held until the parent kills this process.
        std::thread::sleep(Duration::from_secs(60));
        panic!("not killed");
    }
    let (_dir, path, ledger) = common::ledger();
    let mut child = common::spawn("held_txn_times_out", &path, "");
    common::wait_for(Duration::from_secs(10), || marker(&path).exists());
    let held = Instant::now();
    let err = ledger
        .apply(&actor("0"), None, AT, create_lane("L"))
        .unwrap_err();
    let elapsed = held.elapsed();
    assert!(matches!(&err, LedgerError::Sqlite(_)), "{err}");
    assert!(err.to_string().contains("database is locked"), "{err}");
    // The limit is wall time from the first busy callback, and a slow runner
    // can oversleep the last retry, so only the floor is tight; the child
    // holds the lock for 60 s.
    assert!(
        (Duration::from_millis(4500)..Duration::from_secs(20)).contains(&elapsed),
        "{elapsed:?}"
    );
    eprintln!("the write gave up after {elapsed:?}");

    // Released after at least 6 s held.
    std::thread::sleep(Duration::from_secs(6).saturating_sub(held.elapsed()));
    child.kill();
    assert_eq!(
        common::count(&read_only(&path), "SELECT count(*) FROM events"),
        0
    );
    assert_consistent(&path);
    committed(
        ledger
            .apply(&actor("0"), None, AT, create_lane("L"))
            .unwrap(),
    );
}

fn marker(path: &Path) -> std::path::PathBuf {
    path.with_extension("held")
}

/// Two processes claim one lane while both are inside the write function:
/// the parent holds the write lock until both are waiting on it. One claim
/// commits, the other is refused and appends nothing.
#[test]
fn two_claimants_one_winner() {
    if let Some((path, i)) = common::child() {
        let ledger = Ledger::open(&path).unwrap();
        let who = actor(&i);
        let change = |from, to| Change { from, to };
        let changed = BTreeMap::from([
            ("status".into(), change(json!("queued"), json!("claimed"))),
            ("agent".into(), change(Value::Null, json!(who.agent))),
            ("session".into(), change(Value::Null, json!(who.session))),
        ]);
        let claim = Mutation::Event {
            kind: EventKind::LaneClaimed,
            lane_id: Some(1),
            spec_id: None,
            payload: Payload {
                changed,
                lease_secs: Some(900),
                ..Payload::default()
            },
        };
        std::fs::write(path.with_extension(format!("ready{i}")), "").unwrap();
        let key = format!("claim-{i}");
        match ledger.apply(&who, Some(&key), AT, claim).unwrap() {
            Applied::Committed(_) => std::process::exit(0),
            Applied::Refused(_) => std::process::exit(3),
            other => panic!("{other:?}"),
        }
    }
    let (_dir, path, ledger) = common::ledger();
    committed(
        ledger
            .apply(&actor("0"), None, AT, create_lane("L"))
            .unwrap(),
    );
    let db = Connection::open(&path).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut children: Vec<_> = (0..2)
        .map(|i| common::spawn("two_claimants_one_winner", &path, &i.to_string()))
        .collect();
    common::wait_for(Duration::from_secs(4), || {
        (0..2).all(|i| path.with_extension(format!("ready{i}")).exists())
    });
    // Both are now blocked on the lock inside `apply`.
    std::thread::sleep(Duration::from_millis(200));
    db.execute_batch("ROLLBACK").unwrap();

    let codes: Vec<_> = children.iter_mut().map(|c| c.wait().code()).collect();
    let winner = codes.iter().position(|c| *c == Some(0));
    assert!(winner.is_some() && codes.contains(&Some(3)), "{codes:?}");
    let db = read_only(&path);
    assert_eq!(
        common::count(
            &db,
            "SELECT count(*) FROM events WHERE kind = 'lane.claimed'"
        ),
        1
    );
    let agent: String = db
        .query_row("SELECT agent FROM lanes WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(agent, format!("a{}", winner.unwrap()));
    assert_consistent(&path);
}
