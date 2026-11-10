//! The fact bundle for one lane (R7), projected from its events. Gates read
//! these facts, never events or prose. A fact no M1 event produces stays
//! absent, so a gate that reads it denies (events.md, facts in M1).

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, params};
use serde_json::Value;

use super::LedgerError;
use super::event::{Event, EventKind};
use super::write::{EVENT_COLUMNS, event_from_row};
use crate::pipeline::{Fact, FactBundle};

/// Projects lane `lane_id`. `head` is the commit the caller wants to hand
/// off or finish (`commit.sha`): the handoff's commit, or for `update
/// --status done` the latest handoff's. It is never read from stored
/// events, so a new commit cannot borrow an older one's verdict.
pub(crate) fn project(
    conn: &Connection,
    lane_id: i64,
    head: Option<&str>,
) -> Result<FactBundle, LedgerError> {
    let mut f = FactBundle::default();
    let (id, hash, kind): (Option<String>, Option<String>, String) = conn.query_row(
        "SELECT pipeline_id, pipeline_hash, kind FROM lanes WHERE id = ?1",
        [lane_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if let (Some(id), Some(hash)) = (id, hash) {
        f.set("pipeline.id", id.as_str())
            .set("pipeline.hash", hash.as_str());
    }
    // Until a pipeline reads the `diff` check, only a code lane changes code.
    f.set("lane.changes_code", kind == "code");
    if let Some(head) = head {
        f.set("commit.sha", head);
    }

    let mut stmt = conn.prepare(&format!(
        "SELECT {EVENT_COLUMNS} FROM events
         WHERE lane_id = ?1 AND kind != 'usage.reported' ORDER BY id"
    ))?;
    let mut rows = stmt.query([lane_id])?;
    let (mut smoke, mut kept, mut rounds) = (false, false, 0);
    let mut latest_round: Option<i64> = None;
    // A request with no job can never be matched by a finish, so it stays
    // running: it fails closed.
    let (mut requested, mut finished) = (BTreeSet::new(), BTreeSet::new());
    // The newest daisy job finished per review check. Daisy's ids only go
    // up, so a late finish of an older job never moves the lane back.
    let mut newest_job = BTreeMap::new();
    // The first time each quality gate (outcome, reason) was recorded, and
    // the latest one's key.
    let (mut qg_first, mut qg_latest) = (BTreeMap::new(), None);
    let mut review_state = "absent";
    while let Some(row) = rows.next()? {
        let e: Event = event_from_row(row)?;
        let p = &e.payload;
        let pr_set = p.changed.get("pr_ref").is_some_and(|c| !c.to.is_null());
        if pr_set && matches!(f.get("pr.opened_at"), Fact::Absent) {
            f.set("pr.opened_at", Fact::Time(e.at.clone()));
        }
        let job = p.rest.get("job").map(Value::to_string);
        match (e.kind, p.check.as_deref()) {
            (EventKind::CheckRequested, _) => {
                requested.insert(job);
            }
            (EventKind::CheckFinished, Some(check)) => {
                let ran = p.outcome.as_deref() == Some("ran");
                let text = |key: &str| p.rest.get(key).and_then(Value::as_str);
                match check {
                    "plan_check" | "quality_gate" => {
                        set(&mut f, &format!("{check}.outcome"), p.outcome.as_deref());
                        set(&mut f, &format!("{check}.reason"), text("reason"));
                        if check == "quality_gate" {
                            let key = (p.outcome.clone(), text("reason").map(String::from));
                            qg_first.entry(key.clone()).or_insert_with(|| e.at.clone());
                            qg_latest = Some(key);
                        }
                    }
                    // Only a verdict that ran is one (events.md: `ran` only).
                    "merge_verdict" if ran => {
                        set(&mut f, "verdict.decision", text("decision"));
                        set(&mut f, "verdict.sha", p.head_sha.as_deref());
                    }
                    "smoke" => smoke = true,
                    "kept_tests" => kept = true,
                    // A review round daisy did not run (a review agent's): it
                    // counts toward the cap and nothing else.
                    "review_round" => rounds += 1,
                    "local_review" | "pr_review" => {
                        if job.is_some() {
                            finished.insert(job);
                        }
                        let older = p.rest.get("job").and_then(Value::as_i64).is_some_and(|id| {
                            let newest = newest_job.entry(check.to_string()).or_insert(id);
                            *newest = id.max(*newest);
                            id < *newest
                        });
                        let local_round = check == "local_review" && ran;
                        if local_round {
                            rounds += 1;
                        }
                        if !older {
                            review_state = if ran { "finished" } else { "failed" };
                            if local_round {
                                latest_round = Some(e.id);
                                set(&mut f, "review.local_head", p.head_sha.as_deref());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    // Recording the same result again after the PR keeps its first time; a
    // different result is timed from when it was first recorded.
    if let Some(at) = qg_latest.and_then(|k| qg_first.remove(&k)) {
        f.set("quality_gate.at", Fact::Time(at));
    }
    f.set("smoke.recorded", smoke)
        .set("kept_tests.recorded", kept)
        .set("review.local_rounds", rounds);
    if !requested.is_subset(&finished) {
        review_state = "running";
    }
    f.set("daisy_review.state", review_state);
    if let Some(round) = latest_round {
        let open: i64 = conn.query_row(
            "SELECT count(*) FROM findings
             WHERE lane_id = ?1 AND check_event_id = ?2 AND status = 'open'",
            params![lane_id, round],
            |r| r.get(0),
        )?;
        f.set("review.previous_round_closed", open == 0);
    }
    Ok(f)
}

/// A text fact, absent when the field is.
fn set(f: &mut FactBundle, path: &str, v: Option<&str>) {
    f.set(path, v.map_or(Fact::Absent, |v| Fact::Str(v.into())));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ledger::{Actor, Applied, Change, Finding, Findings, Ledger, Mutation, Payload};
    use crate::pipeline::{Phase, Pipeline, Route};

    const T: [&str; 4] = [
        "2026-11-03T00:00:00.000Z",
        "2026-11-03T00:00:01.000Z",
        "2026-11-03T00:00:02.000Z",
        "2026-11-03T00:00:03.000Z",
    ];

    struct Lane {
        _dir: tempfile::TempDir,
        ledger: Ledger,
        me: Actor,
    }

    impl Lane {
        /// Lane 1 of `kind`, claimed by `me` at T[0].
        fn new(kind: &str) -> Lane {
            let dir = tempfile::tempdir().unwrap();
            let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
            let lane = Lane {
                _dir: dir,
                ledger,
                me: Actor {
                    agent: Some("a".into()),
                    session: Some("s".into()),
                },
            };
            lane.apply(
                EventKind::LaneCreated,
                Payload {
                    title: Some("t".into()),
                    source: Some("manual".into()),
                    kind: Some(kind.into()),
                    pr_flow: Some("pr".into()),
                    ..Payload::default()
                },
                T[0],
            );
            lane
        }

        fn apply(&self, kind: EventKind, payload: Payload, at: &str) -> Event {
            let lane_id = (kind != EventKind::LaneCreated).then_some(1);
            let m = Mutation::Event {
                kind,
                lane_id,
                spec_id: None,
                payload,
            };
            match self.ledger.apply(&self.me, None, at, m).unwrap() {
                Applied::Committed(e) => e,
                other => panic!("{other:?}"),
            }
        }

        fn change(&self, kind: EventKind, field: &str, from: Value, to: Value, at: &str) {
            let mut p = Payload::default();
            p.changed.insert(field.into(), Change { from, to });
            p.lease_secs = kind.leases().then_some(60);
            self.apply(kind, p, at);
        }

        fn claim(&self) {
            let mut p = Payload {
                lease_secs: Some(60),
                ..Payload::default()
            };
            p.changed.insert(
                "agent".into(),
                Change {
                    from: json!(null),
                    to: json!("a"),
                },
            );
            p.changed.insert(
                "session".into(),
                Change {
                    from: json!(null),
                    to: json!("s"),
                },
            );
            self.apply(EventKind::LaneClaimed, p, T[0]);
        }

        /// A `check.finished` with `outcome` and the `rest` fields given.
        fn check(&self, check: &str, outcome: &str, rest: Value, at: &str) -> Event {
            let mut p = Payload {
                check: Some(check.into()),
                outcome: Some(outcome.into()),
                head_sha: Some("abc1234".into()),
                ..Payload::default()
            };
            if let Value::Object(rest) = rest {
                p.rest = rest;
            }
            self.apply(EventKind::CheckFinished, p, at)
        }

        /// The lane's facts, each one a path `FACTS` lists.
        fn facts(&self) -> FactBundle {
            self.facts_at(None)
        }

        fn facts_at(&self, head: Option<&str>) -> FactBundle {
            let f = self.ledger.project(1, head).unwrap();
            let unlisted: Vec<&str> = f
                .paths()
                .filter(|p| !crate::pipeline::FACTS.contains(p))
                .collect();
            assert!(unlisted.is_empty(), "FACTS does not list {unlisted:?}");
            f
        }
    }

    fn s(v: &str) -> Fact {
        Fact::Str(v.into())
    }

    #[test]
    fn stamp_kind_and_head() {
        let lane = Lane::new("code");
        let f = lane.facts();
        assert_eq!(f.get("pipeline.id"), &Fact::Absent);
        assert_eq!(f.get("lane.changes_code"), &Fact::Bool(true));
        assert_eq!(f.get("commit.sha"), &Fact::Absent);
        lane.claim();
        let newest = crate::pipeline::current();
        let f = lane.facts_at(Some("abc1234def"));
        assert_eq!(f.get("pipeline.id"), &s(&newest.id));
        assert_eq!(f.get("pipeline.hash"), &s(&newest.hash));
        assert_eq!(f.get("commit.sha"), &s("abc1234def"));
        assert_eq!(
            Lane::new("docs").facts().get("lane.changes_code"),
            &Fact::Bool(false)
        );
    }

    #[test]
    fn plan_check_and_quality_gate_take_the_latest_outcome() {
        let lane = Lane::new("docs");
        for check in ["plan_check", "quality_gate"] {
            lane.check(check, "skipped", json!({"reason": "docs"}), T[1]);
            let f = lane.facts();
            assert_eq!(f.get(&format!("{check}.outcome")), &s("skipped"));
            assert_eq!(f.get(&format!("{check}.reason")), &s("docs"));
            lane.check(check, "ran", json!({}), T[2]);
            let f = lane.facts();
            assert_eq!(f.get(&format!("{check}.outcome")), &s("ran"));
            assert_eq!(f.get(&format!("{check}.reason")), &Fact::Absent);
        }
        assert_eq!(
            lane.facts().get("quality_gate.at"),
            &Fact::Time(T[2].into())
        );
        assert_eq!(lane.facts().get("plan_check.at"), &Fact::Absent);
    }

    /// v17's `quality_gate_before_pr` on projected times: the first quality
    /// gate counts, so one recorded again after the PR does not deny.
    #[test]
    fn quality_gate_before_pr_on_projected_times() {
        let doc = r#"
schema = 1
id = "t"

[[gate]]
id = "quality_gate_before_pr"
fn = "event_before"
args = { earlier = "quality_gate", later = "pr.opened" }

[[transition]]
id = "handoff"
routes = ["code_pr"]
to = "done"
requires = ["quality_gate_before_pr"]
"#;
        let p = Pipeline::load(doc).unwrap();
        let eval = |f: &FactBundle| p.evaluate("handoff", Route::CodePr, Phase::Done, f).is_ok();
        let lane = Lane::new("code");
        assert!(eval(&lane.facts()), "no PR yet: vacuous");
        lane.change(
            EventKind::LaneUpdated,
            "pr_ref",
            json!(null),
            json!("o/r#1"),
            T[1],
        );
        assert_eq!(lane.facts().get("pr.opened_at"), &Fact::Time(T[1].into()));
        assert!(!eval(&lane.facts()), "PR with no quality gate");
        lane.check("quality_gate", "ran", json!({}), T[2]);
        assert!(!eval(&lane.facts()), "quality gate after the PR");

        let lane = Lane::new("code");
        lane.check("quality_gate", "ran", json!({}), T[1]);
        lane.change(
            EventKind::LaneUpdated,
            "pr_ref",
            json!(null),
            json!("o/r#1"),
            T[2],
        );
        lane.check("quality_gate", "ran", json!({}), T[3]);
        assert!(eval(&lane.facts()));
        // A later PR change does not move the time it opened.
        lane.change(
            EventKind::LaneUpdated,
            "pr_ref",
            json!("o/r#1"),
            json!("o/r#2"),
            T[3],
        );
        assert_eq!(lane.facts().get("pr.opened_at"), &Fact::Time(T[2].into()));

        // A skip before the PR does not time a gate that ran after it.
        let skip = Lane::new("code");
        skip.check(
            "quality_gate",
            "skipped",
            json!({"reason": "no_code"}),
            T[1],
        );
        skip.change(
            EventKind::LaneUpdated,
            "pr_ref",
            json!(null),
            json!("o/r#1"),
            T[2],
        );
        skip.check("quality_gate", "ran", json!({}), T[3]);
        assert_eq!(
            skip.facts().get("quality_gate.at"),
            &Fact::Time(T[3].into())
        );
        assert!(!eval(&skip.facts()));
    }

    #[test]
    fn verdict_smoke_and_kept_tests() {
        let lane = Lane::new("code");
        let f = lane.facts();
        for path in ["verdict.decision", "verdict.sha"] {
            assert_eq!(f.get(path), &Fact::Absent, "{path}");
        }
        assert_eq!(f.get("smoke.recorded"), &Fact::Bool(false));
        assert_eq!(f.get("kept_tests.recorded"), &Fact::Bool(false));
        // A skipped verdict is no verdict.
        lane.check("merge_verdict", "skipped", json!({"decision": "yes"}), T[1]);
        assert_eq!(lane.facts().get("verdict.decision"), &Fact::Absent);
        lane.check("merge_verdict", "ran", json!({"decision": "no"}), T[1]);
        lane.check("merge_verdict", "ran", json!({"decision": "yes"}), T[2]);
        lane.check("smoke", "skipped", json!({"reason": "nothing_runs"}), T[2]);
        lane.check("kept_tests", "skipped", json!({"reason": "none"}), T[2]);
        let f = lane.facts();
        assert_eq!(f.get("verdict.decision"), &s("yes"));
        assert_eq!(f.get("verdict.sha"), &s("abc1234"));
        assert_eq!(f.get("smoke.recorded"), &Fact::Bool(true));
        assert_eq!(f.get("kept_tests.recorded"), &Fact::Bool(true));
    }

    #[test]
    fn local_rounds_head_and_previous_round_closed() {
        let lane = Lane::new("code");
        let f = lane.facts();
        assert_eq!(f.get("review.local_rounds"), &Fact::Int(0));
        assert_eq!(f.get("review.local_head"), &Fact::Absent);
        assert_eq!(f.get("review.previous_round_closed"), &Fact::Absent);
        let finding = Finding {
            finding_key: "f1".into(),
            severity: "nit".into(),
            title: "t".into(),
            body: "b".into(),
            path: None,
            rest: Default::default(),
        };
        let round = lane.apply(
            EventKind::CheckFinished,
            Payload {
                check: Some("local_review".into()),
                outcome: Some("ran".into()),
                head_sha: Some("def5678".into()),
                findings: Some(Findings::List(vec![finding])),
                ..Payload::default()
            },
            T[1],
        );
        // A failed review is not a round and leaves the head alone.
        lane.check(
            "local_review",
            "skipped",
            json!({"reason": "failed", "job": 2}),
            T[2],
        );
        let f = lane.facts();
        assert_eq!(f.get("review.local_rounds"), &Fact::Int(1));
        assert_eq!(f.get("review.local_head"), &s("def5678"));
        assert_eq!(f.get("review.previous_round_closed"), &Fact::Bool(false));
        lane.apply(
            EventKind::FindingUpdated,
            Payload {
                check_event_id: Some(round.id),
                finding_key: Some("f1".into()),
                status: Some("fixed".into()),
                ..Payload::default()
            },
            T[3],
        );
        assert_eq!(
            lane.facts().get("review.previous_round_closed"),
            &Fact::Bool(true)
        );
    }

    #[test]
    fn daisy_review_state() {
        let lane = Lane::new("code");
        assert_eq!(lane.facts().get("daisy_review.state"), &s("absent"));
        let requested = |job: i64| {
            let mut p = Payload {
                check: Some("pr_review".into()),
                head_sha: Some("abc1234".into()),
                ..Payload::default()
            };
            p.rest.insert("job".into(), json!(job));
            lane.apply(EventKind::CheckRequested, p, T[1]);
        };
        requested(7);
        assert_eq!(lane.facts().get("daisy_review.state"), &s("running"));
        lane.check("pr_review", "ran", json!({"job": 7}), T[2]);
        assert_eq!(lane.facts().get("daisy_review.state"), &s("finished"));
        requested(8);
        assert_eq!(lane.facts().get("daisy_review.state"), &s("running"));
        lane.check(
            "local_review",
            "skipped",
            json!({"job": 8, "reason": "cancelled"}),
            T[3],
        );
        assert_eq!(lane.facts().get("daisy_review.state"), &s("failed"));
        // A request with no job never finishes.
        lane.apply(
            EventKind::CheckRequested,
            Payload {
                check: Some("local_review".into()),
                head_sha: Some("abc1234".into()),
                ..Payload::default()
            },
            T[3],
        );
        lane.check("local_review", "ran", json!({"job": 9}), T[3]);
        assert_eq!(lane.facts().get("daisy_review.state"), &s("running"));
    }

    /// Older jobs that finish after a newer one close their requests and
    /// count their rounds, but the lane stays on the newer job.
    #[test]
    fn older_job_finished_late_stays_behind() {
        let lane = Lane::new("code");
        let local = |job: i64, outcome: &str, head: &str, findings: Vec<Finding>| {
            let mut p = Payload {
                check: Some("local_review".into()),
                outcome: Some(outcome.into()),
                head_sha: Some(head.into()),
                findings: Some(Findings::List(findings)),
                ..Payload::default()
            };
            p.rest.insert("job".into(), json!(job));
            lane.apply(EventKind::CheckFinished, p, T[2]);
        };
        for job in [9, 10] {
            let mut p = Payload {
                check: Some("local_review".into()),
                head_sha: Some("abc1234".into()),
                ..Payload::default()
            };
            p.rest.insert("job".into(), json!(job));
            lane.apply(EventKind::CheckRequested, p, T[1]);
        }
        local(11, "ran", "def5678", vec![]);
        let open = Finding {
            finding_key: "f1".into(),
            severity: "nit".into(),
            title: "t".into(),
            body: "b".into(),
            path: None,
            rest: Default::default(),
        };
        local(9, "ran", "abc1234", vec![open]);
        local(10, "skipped", "abc1234", vec![]);
        let f = lane.facts();
        assert_eq!(f.get("daisy_review.state"), &s("finished"));
        assert_eq!(f.get("review.local_rounds"), &Fact::Int(2));
        assert_eq!(f.get("review.local_head"), &s("def5678"));
        assert_eq!(f.get("review.previous_round_closed"), &Fact::Bool(true));
    }
}
