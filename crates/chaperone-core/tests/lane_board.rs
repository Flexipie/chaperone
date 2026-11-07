//! The viewer's ledger: `Ledger::open_read_only` writes nothing and is
//! refused like `Ledger::open`, and `lane_board` reads one snapshot with
//! the gate stamps the projection gives.

use chaperone_core::checks::CheckInput;
use chaperone_core::ledger::{Actor, Ledger, LedgerError, SCHEMA_VERSION};
use chaperone_core::work::CreateInput;
use rusqlite::Connection;

const T0: &str = "2026-11-03T00:00:00.000Z";

fn pair(agent: &str, session: &str) -> Actor {
    Actor {
        agent: Some(agent.into()),
        session: Some(session.into()),
    }
}

fn create() -> CreateInput {
    CreateInput {
        title: "t".into(),
        repo: Some("Flexipie/chaperone".into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    }
}

#[test]
fn read_only_writes_nothing_and_sees_other_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    assert!(matches!(
        Ledger::open_read_only(&path),
        Err(LedgerError::Missing(_))
    ));
    assert!(!path.exists());

    let rw = Ledger::open_and_migrate(&path).unwrap();
    let me = pair("lane", "s1");
    rw.work_create(&me, &create(), Some(T0), false)
        .unwrap()
        .unwrap();
    let ro = Ledger::open_read_only(&path).unwrap();
    let before = std::fs::read(&path).unwrap();
    let version = ro.data_version().unwrap();

    // Every write goes through the connection, and SQLite refuses it.
    let err = ro.work_create(&me, &create(), Some(T0), false).err();
    assert!(
        matches!(&err, Some(LedgerError::Sqlite(e))
            if e.sqlite_error_code() == Some(rusqlite::ErrorCode::ReadOnly)),
        "{err:?}"
    );
    assert_eq!(ro.data_version().unwrap(), version);
    assert!(
        before == std::fs::read(&path).unwrap(),
        "ledger bytes changed"
    );

    // Another connection's commit moves data_version and shows on the board.
    let checker = pair("verdict", "s2");
    let check = CheckInput {
        work: 1,
        check: "plan_check".into(),
        head: "abc1234".into(),
        outcome: "ran".into(),
        findings: Some(2),
        ..CheckInput::default()
    };
    rw.check_record(&checker, &check).unwrap().unwrap();
    let board = ro.lane_board(T0).unwrap();
    assert_ne!(board.data_version, version);
    assert_eq!(board.lanes.len(), 1);
    let stamps = &board.lanes[0].stamps;
    assert_eq!(stamps.plan_check.as_deref(), Some("ran"));
    assert_eq!(stamps.quality_gate, None);
    assert_eq!(stamps.verdict, None);
}

#[test]
fn read_only_refuses_other_versions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    drop(Ledger::open_and_migrate(&path).unwrap());
    let ro = Ledger::open_read_only(&path).unwrap();

    // A newer binary migrates while the viewer is open: the next board is
    // refused, not read through the old shape.
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    for result in [
        ro.lane_board(T0).map(drop),
        Ledger::open_read_only(&path).map(drop),
    ] {
        assert!(
            matches!(result, Err(LedgerError::SchemaMismatch { found, .. }) if found == SCHEMA_VERSION + 1),
            "{result:?}"
        );
    }

    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "not a database").unwrap();
    assert!(matches!(
        Ledger::open_read_only(&text),
        Err(LedgerError::NotALedger(_))
    ));
}
