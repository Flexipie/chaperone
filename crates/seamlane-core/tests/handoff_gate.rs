//! The in-process `verdict_on_head` fixture (CONTRIBUTING.md): the gated
//! `handoff` on a real ledger. A lane that hands off done or ready without
//! a plan check, a quality gate and a YES verdict on the handed-off sha,
//! recorded by another session, appends one `gate.denied` and does not move.

use rusqlite::Connection;
use seamlane_core::ledger::{Actor, Applied, EventKind, Ledger, Mutation, Payload, Refusal};
use seamlane_core::work::{ClaimInput, CreateInput, HandoffInput, UpdateInput};
use serde_json::json;

const T0: &str = "2026-11-03T00:00:00.000Z";
const T1: &str = "2026-11-03T00:01:00.000Z";
const T2: &str = "2026-11-03T00:02:00.000Z";

fn pair(agent: &str, session: &str) -> Actor {
    Actor {
        agent: Some(agent.into()),
        session: Some(session.into()),
    }
}

struct Lane {
    _dir: tempfile::TempDir,
    ledger: Ledger,
    db: Connection,
    lane: Actor,
    checker: Actor,
}

impl Lane {
    /// Lane 1, a code PR lane claimed by the lane session s1.
    fn new() -> Lane {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let ledger = Ledger::open_and_migrate(&path).unwrap();
        let lane = Lane {
            db: Connection::open(&path).unwrap(),
            _dir: dir,
            ledger,
            lane: pair("lane", "s1"),
            checker: pair("verdict", "s2"),
        };
        let create = CreateInput {
            title: "t".into(),
            kind: "code".into(),
            pr_flow: "pr".into(),
            ..CreateInput::default()
        };
        let me = &lane.lane;
        lane.ledger
            .work_create(me, &create, Some(T0), false)
            .unwrap()
            .unwrap();
        let claim = ClaimInput::default();
        lane.ledger
            .work_claim(me, 1, &claim, Some(T0), false)
            .unwrap()
            .unwrap();
        lane
    }

    fn check(&self, who: &Actor, check: &str, rest: serde_json::Value) -> Applied {
        let mut payload = Payload {
            check: Some(check.into()),
            outcome: Some("ran".into()),
            head_sha: Some("abc1234".into()),
            ..Payload::default()
        };
        if let serde_json::Value::Object(rest) = rest {
            payload.rest = rest;
        }
        let m = Mutation::Event {
            kind: EventKind::CheckFinished,
            lane_id: Some(1),
            spec_id: None,
            payload,
        };
        self.ledger.apply(who, None, T1, m).unwrap()
    }

    fn stamps(&self) {
        for (check, rest) in [
            ("plan_check", json!({})),
            ("quality_gate", json!({})),
            ("merge_verdict", json!({"decision": "yes"})),
        ] {
            assert!(matches!(
                self.check(&self.checker, check, rest),
                Applied::Committed(_)
            ));
        }
    }

    fn handoff(&self, commit: &str, status: &str, ready: Option<bool>) -> Result<i64, Refusal> {
        let input = HandoffInput {
            summary: "s".into(),
            status: Some(status.into()),
            commit: Some(commit.into()),
            ready,
            ..HandoffInput::default()
        };
        let h = self
            .ledger
            .work_handoff(&self.lane, 1, &input, Some(T2), false)
            .unwrap();
        h.map(|h| h.id)
    }

    fn count(&self, kind: &str) -> i64 {
        self.db
            .query_row("SELECT count(*) FROM events WHERE kind = ?1", [kind], |r| {
                r.get(0)
            })
            .unwrap()
    }

    /// The lane row's status, phase and updated_at.
    fn row(&self) -> (String, Option<String>, String) {
        self.db
            .query_row(
                "SELECT status, phase, updated_at FROM lanes WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }
}

fn denied(gate: &str, fact_paths: &[&str]) -> Result<i64, Refusal> {
    Err(Refusal::GateDenied {
        transition: "handoff".into(),
        gate: gate.into(),
        fact_paths: fact_paths.iter().map(|s| s.to_string()).collect(),
    })
}

#[test]
fn done_without_stamps_is_denied_once_and_moves_nothing() {
    let lane = Lane::new();
    let before = lane.row();
    let no_plan = denied(
        "plan_check_recorded",
        &[
            "plan_check.outcome",
            "lane.changes_code",
            "plan_check.reason",
        ],
    );
    assert_eq!(lane.handoff("abc1234def", "done", None), no_plan);
    // A retry with no new fact replays the denial.
    assert_eq!(lane.handoff("abc1234def", "done", None), no_plan);
    let update = UpdateInput {
        status: Some("done".into()),
        ..UpdateInput::default()
    };
    let r = lane
        .ledger
        .work_update(&lane.lane, 1, &update, None, Some(T2), false)
        .unwrap();
    assert!(matches!(r, Err(Refusal::GateDenied { .. })), "{r:?}");
    assert_eq!(lane.count("gate.denied"), 1);
    assert_eq!(lane.row(), before);
}

#[test]
fn the_lane_session_cannot_record_its_own_stamps() {
    let lane = Lane::new();
    for check in ["plan_check", "quality_gate", "merge_verdict"] {
        let applied = lane.check(&lane.lane, check, json!({"decision": "yes"}));
        assert!(
            matches!(applied, Applied::Refused(Refusal::InvalidInput(_))),
            "{applied:?}"
        );
    }
    assert_eq!(lane.count("check.finished"), 0);
}

#[test]
fn verdict_on_head_denies_another_commit_and_passes_its_own() {
    let lane = Lane::new();
    lane.stamps();
    let before = lane.row();
    assert_eq!(
        lane.handoff("fff9999aaa", "done", None),
        denied("verdict_on_head", &["verdict.sha", "commit.sha"])
    );
    assert_eq!((lane.count("gate.denied"), lane.row()), (1, before));

    let id = lane.handoff("abc1234def", "done", None).unwrap();
    let item = lane.ledger.work_get(1, T2).unwrap().unwrap();
    assert_eq!(
        (item.status.as_str(), item.phase.as_deref(), item.session),
        ("done", Some("done"), None)
    );
    let (kind, payload): (String, String) = lane
        .db
        .query_row(
            "SELECT kind, payload_json FROM events WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(kind, "phase.entered");
    assert_eq!(
        (
            &payload["transition"],
            &payload["from"],
            &payload["to"],
            &payload["head_sha"]
        ),
        (
            &json!("handoff"),
            &json!(null),
            &json!("done"),
            &json!("abc1234def")
        )
    );
    assert_eq!(lane.ledger.work_handoffs(1).unwrap().len(), 1);
}

#[test]
fn a_ready_handoff_passes_the_gate_and_reads_ready() {
    let lane = Lane::new();
    assert!(matches!(
        lane.handoff("abc1234def", "review", Some(true)),
        Err(Refusal::GateDenied { .. })
    ));
    lane.stamps();
    lane.handoff("abc1234def", "review", Some(true)).unwrap();
    let item = lane.ledger.work_get(1, T2).unwrap().unwrap();
    assert_eq!(
        (item.status.as_str(), item.phase.as_deref()),
        ("review", Some("ready"))
    );
    // `update --status done` names the latest handoff's commit.
    let update = UpdateInput {
        status: Some("done".into()),
        ..UpdateInput::default()
    };
    let item = lane
        .ledger
        .work_update(&lane.lane, 1, &update, None, Some(T2), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        (item.status.as_str(), item.phase.as_deref()),
        ("done", Some("done"))
    );
}

/// The write decides the gate whoever calls it: an ungated done, ready or
/// `phase.entered` through `Ledger::apply` is refused or denied.
#[test]
fn the_write_gates_every_path() {
    let lane = Lane::new();
    let apply = |kind: EventKind, payload: Payload| {
        let m = Mutation::Event {
            kind,
            lane_id: Some(1),
            spec_id: None,
            payload,
        };
        lane.ledger.apply(&lane.lane, None, T1, m).unwrap()
    };
    let mut done = Payload::default();
    done.changed.insert(
        "status".into(),
        seamlane_core::ledger::Change {
            from: json!("claimed"),
            to: json!("done"),
        },
    );
    let mut ready = Payload::default();
    ready
        .rest
        .insert("handoff".into(), json!({"summary": "s", "ready": true}));
    for (kind, payload) in [
        (EventKind::LaneUpdated, done.clone()),
        (EventKind::LaneHandedOff, ready),
        (EventKind::GateDenied, Payload::default()),
    ] {
        let applied = apply(kind, payload);
        assert!(
            matches!(applied, Applied::Refused(Refusal::InvalidInput(_))),
            "{kind}: {applied:?}"
        );
    }
    let entered = Payload {
        transition: Some("handoff".into()),
        to: Some("done".into()),
        head_sha: Some("abc1234".into()),
        ..done
    };
    let Applied::Committed(e) = apply(EventKind::PhaseEntered, entered) else {
        panic!("a denial is committed");
    };
    assert_eq!(e.kind, EventKind::GateDenied);
    assert_eq!(lane.count("phase.entered"), 0);
}
