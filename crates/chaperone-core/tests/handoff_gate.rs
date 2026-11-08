//! The in-process `verdict_on_head` fixture (CONTRIBUTING.md): the gated
//! `handoff` on a real ledger. A lane that hands off done or ready without
//! a plan check, a quality gate and a YES verdict on the handed-off sha,
//! recorded by another session, appends one `gate.denied` and does not move.

use chaperone_core::ledger::{Actor, Applied, EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::work::{ClaimInput, CreateInput, HandoffInput, UpdateInput};
use rusqlite::Connection;
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
        Lane::with("code", "pr")
    }

    /// Lane 1 of `kind` on `pr_flow`, claimed by s1.
    fn with(kind: &str, pr_flow: &str) -> Lane {
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
            kind: kind.into(),
            pr_flow: pr_flow.into(),
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

    /// The plan check and quality gate skipped with an allowed reason.
    fn skips(&self) {
        for (check, reason) in [("plan_check", "docs"), ("quality_gate", "no_code")] {
            let applied = self.finish(check, "skipped", "abc1234", json!({ "reason": reason }));
            assert!(matches!(applied, Applied::Committed(_)), "{applied:?}");
        }
    }

    /// A done handoff with no commit.
    fn done_no_commit(&self) -> Result<i64, Refusal> {
        let input = HandoffInput {
            summary: "s".into(),
            status: Some("done".into()),
            ..HandoffInput::default()
        };
        let h = self
            .ledger
            .work_handoff(&self.lane, 1, &input, Some(T2), false)
            .unwrap();
        h.map(|h| h.id)
    }

    fn route(&self) -> String {
        self.db
            .query_row("SELECT route FROM lanes WHERE id = 1", [], |r| r.get(0))
            .unwrap()
    }

    fn check(&self, who: &Actor, check: &str, rest: serde_json::Value) -> Applied {
        self.finish_as(who, check, "ran", "abc1234", rest)
    }

    /// A check finished by the checker session.
    fn finish(&self, check: &str, outcome: &str, head: &str, rest: serde_json::Value) -> Applied {
        self.finish_as(&self.checker, check, outcome, head, rest)
    }

    fn finish_as(
        &self,
        who: &Actor,
        check: &str,
        outcome: &str,
        head: &str,
        rest: serde_json::Value,
    ) -> Applied {
        let mut payload = Payload {
            check: Some(check.into()),
            outcome: Some(outcome.into()),
            head_sha: Some(head.into()),
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

    /// v19's ready records: smoke ran and kept tests skipped (none).
    fn smoke_and_kept(&self) {
        self.check(&self.checker, "smoke", json!({}));
        self.finish(
            "kept_tests",
            "skipped",
            "abc1234",
            json!({"reason": "none"}),
        );
    }

    /// The gate a ready handoff on abc1234def is denied as, None when it passes.
    fn ready_denied_as(&self) -> Option<String> {
        match self.handoff("abc1234def", "review", Some(true)) {
            Ok(_) => None,
            Err(Refusal::GateDenied { gate, .. }) => Some(gate),
            Err(e) => panic!("{e:?}"),
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
    for check in [
        "plan_check",
        "quality_gate",
        "merge_verdict",
        "review_round",
    ] {
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
        denied(
            "done_verdict",
            &[
                "lane.changes_code",
                "verdict.decision",
                "verdict.sha",
                "commit.sha"
            ]
        )
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
    lane.smoke_and_kept();
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
        chaperone_core::ledger::Change {
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

/// A denial key another kind already took is refused, never read as a pass.
#[test]
fn a_taken_denial_key_is_refused() {
    let lane = Lane::new();
    let create = CreateInput {
        title: "t".into(),
        kind: "code".into(),
        pr_flow: "pr".into(),
        idempotency_key: Some("deny:1:handoff:plan_check_recorded:2".into()),
        ..CreateInput::default()
    };
    let me = &lane.lane;
    lane.ledger
        .work_create(me, &create, Some(T0), false)
        .unwrap()
        .unwrap();
    let update = UpdateInput {
        status: Some("done".into()),
        ..UpdateInput::default()
    };
    let r = lane
        .ledger
        .work_update(me, 1, &update, None, Some(T2), false)
        .unwrap();
    assert!(matches!(r, Err(Refusal::InvalidInput(_))), "{r:?}");
    assert_eq!(lane.count("gate.denied"), 0);
}

/// The board's ready: the owner's override, else status review and a ready
/// handoff. A later needs-you handoff or a move off review unsays it.
#[test]
fn the_board_reads_the_newest_handoff_and_the_override() {
    let lane = Lane::new();
    let owner = pair("owner", "s3");
    let board = || {
        let e = lane.ledger.work_board_item(1, T2).unwrap().unwrap();
        let o = e.ready_override.as_ref().map(|o| o.ready);
        (
            e.ready,
            e.ready_reason.clone(),
            o,
            e.handoff.as_ref().map(|h| h.id),
        )
    };
    let ready = |r: Option<bool>| {
        lane.ledger
            .work_ready(&owner, 1, r, Some("ok".into()), Some(T2), false)
            .unwrap()
            .unwrap();
    };
    assert_eq!(
        board(),
        (false, "status is claimed, not review".into(), None, None)
    );
    lane.stamps();
    lane.smoke_and_kept();
    let h = lane.handoff("abc1234def", "review", Some(true)).unwrap();
    assert_eq!(board(), (true, "the lane says ready".into(), None, Some(h)));
    let h = lane.handoff("abc1234def", "active", None).unwrap();
    assert_eq!(
        board(),
        (false, "status is active, not review".into(), None, Some(h))
    );
    ready(Some(true));
    assert_eq!(
        board(),
        (
            true,
            "marked ready by owner: ok".into(),
            Some(true),
            Some(h)
        )
    );
    ready(None);
    assert_eq!(
        board(),
        (false, "status is active, not review".into(), None, Some(h))
    );
    // In review again, the newest handoff is the needs-you one.
    let update = UpdateInput {
        status: Some("review".into()),
        ..UpdateInput::default()
    };
    lane.ledger
        .work_update(&lane.lane, 1, &update, None, Some(T2), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        board(),
        (false, "the lane has not said ready".into(), None, Some(h))
    );
}

/// v17.1: a lane that changes no code hands off done on its skips, with no
/// verdict and no commit, on a PR route and with no PR at all.
#[test]
fn a_no_code_lane_is_done_without_a_verdict_or_commit() {
    for (kind, flow, route) in [
        ("docs", "pr", "code_pr"),
        ("measure", "none", "no_pr"),
        ("investigation", "pr", "investigation"),
    ] {
        let lane = Lane::with(kind, flow);
        assert_eq!(lane.route(), route);
        lane.skips();
        lane.done_no_commit().unwrap();
        assert_eq!(lane.row().0, "done", "{kind} {flow}");
    }
}

/// A code lane still needs a YES verdict on its sha on every route, a lane
/// with no PR included.
#[test]
fn a_code_lane_needs_its_verdict_on_every_route() {
    for flow in ["pr", "direct", "none"] {
        let lane = Lane::with("code", flow);
        for check in ["plan_check", "quality_gate"] {
            lane.check(&lane.checker, check, json!({}));
        }
        let before = lane.row();
        assert!(
            matches!(lane.done_no_commit(), Err(Refusal::GateDenied { gate, .. }) if gate == "done_verdict"),
            "{flow}"
        );
        assert_eq!(lane.row(), before);
        lane.check(&lane.checker, "merge_verdict", json!({"decision": "yes"}));
        lane.handoff("abc1234def", "done", None).unwrap();
        assert_eq!(lane.row().0, "done", "{flow}");
    }
}

/// A verdict on the full sha matches a handoff of its short form.
#[test]
fn a_full_sha_verdict_matches_a_short_commit() {
    let full = "abc1234def567890abc1234def567890abc12345";
    let lane = Lane::new();
    for check in ["plan_check", "quality_gate"] {
        lane.check(&lane.checker, check, json!({}));
    }
    lane.finish("merge_verdict", "ran", full, json!({"decision": "yes"}));
    lane.handoff("abc1234", "done", None).unwrap();
}

/// A lane stamped v17 keeps v17: no verdict still denies a docs lane's done.
#[test]
fn a_v17_lane_keeps_the_v17_rule() {
    let lane = Lane::with("docs", "pr");
    lane.db
        .execute(
            "UPDATE lanes SET pipeline_id = 'v17',
             pipeline_hash = '1a8a599ca26abaf2ab5b1dc9aaa301c019237d45fa3293f4326da0b47b6ea2c5'
             WHERE id = 1",
            [],
        )
        .unwrap();
    lane.skips();
    assert_eq!(
        lane.done_no_commit(),
        denied("verdict_is_yes", &["verdict.decision"])
    );
}

/// v19: a code lane's ready needs smoke, then kept tests, recorded; a skip
/// with its reason counts.
#[test]
fn v19_ready_needs_smoke_and_kept_tests_on_a_code_lane() {
    let lane = Lane::new();
    lane.stamps();
    assert_eq!(lane.ready_denied_as().as_deref(), Some("smoke_ok"));
    lane.finish(
        "smoke",
        "skipped",
        "abc1234",
        json!({"reason": "nothing_runs"}),
    );
    assert_eq!(lane.ready_denied_as().as_deref(), Some("kept_tests_ok"));
    lane.check(&lane.checker, "kept_tests", json!({}));
    assert_eq!(lane.ready_denied_as(), None);
    assert_eq!(lane.row().1.as_deref(), Some("ready"));
}

/// v19: a lane that changes no code is ready without smoke or kept tests.
#[test]
fn v19_ready_needs_no_smoke_on_a_no_code_lane() {
    let lane = Lane::with("docs", "pr");
    lane.skips();
    lane.check(&lane.checker, "merge_verdict", json!({"decision": "yes"}));
    assert_eq!(lane.ready_denied_as(), None);
}

/// v19: ready allows at most `local_rounds` (3) review rounds, recorded by a
/// fresh session as `review_round`.
#[test]
fn v19_ready_caps_review_rounds() {
    for (rounds, want) in [(3, None), (4, Some("rounds_at_most"))] {
        let lane = Lane::new();
        lane.stamps();
        lane.smoke_and_kept();
        for _ in 0..rounds {
            lane.check(&lane.checker, "review_round", json!({}));
        }
        assert_eq!(lane.ready_denied_as().as_deref(), want, "{rounds} rounds");
    }
}

/// A lane stamped v17.1 keeps v17.1: ready needs no smoke, kept tests or
/// round cap.
#[test]
fn a_v17_1_lane_keeps_the_v17_1_rule() {
    let lane = Lane::new();
    lane.db
        .execute(
            "UPDATE lanes SET pipeline_id = 'v17.1',
             pipeline_hash = 'd99d9de3d9fe7e59aa046cabdb80a6d34670d5a1dfcab1873a248ba245e32fd5'
             WHERE id = 1",
            [],
        )
        .unwrap();
    lane.stamps();
    for _ in 0..4 {
        lane.check(&lane.checker, "review_round", json!({}));
    }
    assert_eq!(lane.ready_denied_as(), None);
}
