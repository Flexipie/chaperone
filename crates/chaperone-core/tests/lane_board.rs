//! The viewer's ledger: `Ledger::open_read_only` writes nothing and is
//! refused like `Ledger::open`, `lane_board` reads one snapshot with the
//! gate stamps the projection gives and the feed, and `lane_detail` one
//! lane's dossier, checks and findings.

mod common;

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
    let (dir, path, rw) = common::ledger();
    drop(rw);
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

#[test]
fn detail_and_feed_read_one_lane_and_the_board() {
    use chaperone_core::checks::AttentionInput;
    use chaperone_core::work::{ClaimInput, HandoffInput};
    use serde_json::json;

    let (_dir, path, rw) = common::ledger();
    let me = pair("lane", "s1");
    let body = "# P\n\n## slice 1: the slice  (o/r#2)\n\ndone when: x\n\n## decisions\n\n\
                - [slice 1] Q: a? A: b.\n- Q: c? A: d.\n";
    rw.spec_create(&me, "o/r#1", None, body, T0, false)
        .unwrap()
        .unwrap();
    let input = CreateInput {
        source: Some("github".into()),
        source_id: Some("o/r#2".into()),
        spec: Some("o/r#1".into()),
        ..create()
    };
    let id = rw.work_create(&me, &input, None, false).unwrap().unwrap().id;
    rw.work_claim(&me, id, &ClaimInput::default(), None, false)
        .unwrap()
        .unwrap();
    let handoff = HandoffInput {
        summary: "built it".into(),
        ..HandoffInput::default()
    };
    rw.work_handoff(&me, id, &handoff, None, false)
        .unwrap()
        .unwrap();
    let checker = pair("checker", "s2");
    let check = CheckInput {
        work: id,
        check: "plan_check".into(),
        head: "abc1234".into(),
        outcome: "ran".into(),
        findings: Some(1),
        ..CheckInput::default()
    };
    rw.check_record(&checker, &check).unwrap().unwrap();
    let view = json!({
        "job_id": 7, "source": "local", "status": "reviewed", "phase": "done", "done": true,
        "repo": "Flexipie/chaperone", "branch": "b", "head_sha": "def5678", "score": 8, "verdict": "safe",
        "findings": [{
            "id": "f1", "severity": "risk", "title": "a race", "body": "b", "path": "a.rs",
            "line": 1, "verified": true, "lenses": [], "body_truncated": false,
        }],
    });
    rw.review_record(&checker, id, 7, &view).unwrap().unwrap();
    let o = pair("orchestrator", "s3");
    let ask = AttentionInput {
        work: id,
        kind: "asked".into(),
        by: "owner".into(),
        reason: Some("question.scope".into()),
        note: Some("split it?".into()),
        ..AttentionInput::default()
    };
    let asked = rw.attention_record(&o, &ask).unwrap().unwrap();
    let answer = AttentionInput {
        kind: "answered".into(),
        reason: None,
        answers: Some(asked.event_id),
        note: Some("yes, in two".into()),
        ..ask
    };
    rw.attention_record(&o, &answer).unwrap().unwrap();

    let ro = Ledger::open_read_only(&path).unwrap();
    let d = ro.lane_detail(id, Some("o/r#2")).unwrap();
    let dossier = d.dossier.unwrap();
    let in_spec = dossier.slice.in_spec.unwrap();
    assert_eq!(in_spec.decisions, ["[slice 1] Q: a? A: b.", "Q: c? A: d."]);
    assert_eq!(dossier.handoffs.items[0].summary, "built it");
    let checks: Vec<_> = d.checks.iter().map(|c| c.check.as_str()).collect();
    assert_eq!(checks, ["local_review", "plan_check"]);
    assert_eq!(d.findings.len(), 1);
    assert_eq!(
        (d.findings[0].title.as_str(), d.findings[0].status.as_str()),
        ("a race", "open")
    );
    // A key with no record, or none at all, still reads the checks.
    assert!(ro.lane_detail(id, Some("o/r#99")).unwrap().dossier.is_none());
    assert_eq!(ro.lane_detail(id, None).unwrap().checks.len(), 2);

    let board = ro.lane_board(T0).unwrap();
    assert_eq!(board.specs[0].decisions, ["Q: c? A: d.", "[slice 1] Q: a? A: b."]);
    let feed: Vec<_> = board
        .attention
        .iter()
        .map(|a| (a.kind.as_str(), a.note.as_deref()))
        .collect();
    assert_eq!(
        feed,
        [("answered", Some("yes, in two")), ("asked", Some("split it?"))]
    );
}
