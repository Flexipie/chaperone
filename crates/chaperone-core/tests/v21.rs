//! Pipeline v21 on a lane: each gate of its ready and done rows both ways,
//! on harness events written through the core functions `check run` calls.

mod gated;

use chaperone_core::ledger::Refusal;
use chaperone_core::pipeline::{self, Denied, FactBundle, Phase, Route};
use gated::*;
use serde_json::{Value, json};

const DONE_V21: [&str; 5] = [
    "commit.changes_code",
    "verdict.decision",
    "verdict.sha",
    "commit.sha",
    "pr.merged",
];

/// A code lane climbs v21's ready row: each gate denies, with its id and
/// the facts it read, until the harness fact it needs is recorded. A diff
/// past `code_lines` and no tests run deny nothing: v21 asks the review what
/// to cut and leaves tests to CI.
#[test]
fn ready_climbs_v21_s_row() {
    let lane = Gated::on("v21", "code");
    let binary = json!({"code_lines": 1, "changes_code": true, "uncounted_files": 1});
    lane.run("diff", THIRD, binary);
    let uncounted = denied("no_uncounted_files", &["commit.uncounted_files"]);
    assert_eq!(lane.ready(THIRD), uncounted);
    lane.diff(HEAD, 500, true);
    let complete = denied("review_complete", &["review.complete"]);
    assert_eq!(lane.ready(HEAD), complete);
    lane.run("review", HEAD, json!({"complete": false}));
    assert_eq!(lane.ready(HEAD), complete);
    let review = lane.run("review", HEAD, json!({"complete": true})).event_id;
    let blocking = denied("no_blocking_findings", &["findings.open_blocking"]);
    assert_eq!(lane.ready(HEAD), blocking);
    lane.rule(&lane.owner, review, "correctness-blocker", "refuted");
    lane.rule(&lane.owner, review, "correctness-risk", "waived");
    assert_eq!(lane.ready(HEAD), denied("plan_or_no_code", &PLAN));
    lane.run("plan_check", OTHER, json!({}));
    assert_eq!(lane.ready(HEAD), denied("smoke_or_no_code", &SMOKE));
    lane.claim(HEAD);
    let yes = denied("verdict_is_yes", &["verdict.decision"]);
    assert_eq!(lane.ready(HEAD), yes);
    lane.verdict(AMEND, "yes");
    let on_head = denied("verdict_on_head", &["verdict.sha", "commit.sha"]);
    assert_eq!(lane.ready(HEAD), on_head);
    lane.verdict(HEAD, "yes");
    lane.ready(HEAD).unwrap();
    assert_eq!(lane.board(), (true, "the lane says ready".into()));
}

/// A code lane climbs v21's done row: the plan check, then a yes on the
/// head; a lane whose diff changes no code needs neither.
#[test]
fn done_climbs_v21_s_row() {
    let lane = Gated::on("v21", "code");
    lane.diff(HEAD, 5, true);
    assert_eq!(lane.handoff(HEAD, "done"), denied("plan_or_no_code", &PLAN));
    lane.run("plan_check", OTHER, json!({}));
    assert_eq!(
        lane.handoff(HEAD, "done"),
        denied("done_verdict", &DONE_V21)
    );
    lane.verdict(HEAD, "yes");
    lane.handoff(HEAD, "done").unwrap();

    let docs = Gated::on("v21", "code");
    docs.diff(HEAD, 0, false);
    docs.handoff(HEAD, "done").unwrap();
}

/// `stamp_ok` holds on every v20 lane (the climb above); its denial is the
/// stamp of another pipeline, which no v20 lane can carry. v21 is v20's
/// rules with its own id: its own stamp passes `stamp_ok`, v20's does not.
#[test]
fn stamp_ok_denies_another_stamp() {
    let expected = Denied {
        gate: "stamp_ok".into(),
        fact_paths: vec!["pipeline.id".into()],
    };
    for p in pipeline::newer() {
        for (stamp, passes) in [
            ("v19", false),
            ("v20", p.id == "v20"),
            ("v21", p.id == "v21"),
        ] {
            let mut facts = FactBundle::default();
            facts.set("pipeline.id", stamp);
            let denial = p.evaluate("handoff", Route::CodePr, Phase::Ready, &facts);
            assert_eq!(
                denial.err() == Some(expected.clone()),
                !passes,
                "{} {stamp}",
                p.id
            );
        }
    }
}

/// v21: once the lane's PR is recorded merged, done needs no verdict, and
/// the done event says the merge had none (not when it had a yes); v20
/// still needs its verdict.
#[test]
fn a_v21_lane_whose_pr_merged_is_done_without_a_verdict() {
    for (pipeline, yes) in [("v20", false), ("v21", false), ("v21", true)] {
        let lane = Gated::on(pipeline, "code");
        lane.run("plan_check", OTHER, json!({}));
        lane.diff(HEAD, 5, true);
        let update = chaperone_core::work::UpdateInput {
            pr_ref: Some("o/r#1".into()),
            ..Default::default()
        };
        let l = &lane.ledger;
        l.work_update(&lane.lane, 1, &update, None, None, false)
            .unwrap()
            .unwrap();
        let unmerged = lane.handoff(HEAD, "done");
        assert!(
            matches!(&unmerged, Err(Refusal::GateDenied { gate, .. }) if gate == "done_verdict"),
            "{pipeline}: {unmerged:?}"
        );
        let merged = chaperone_core::checks::MergedInput {
            work: 1,
            pr: "o/r#1".into(),
            merge_sha: "abcdef1".into(),
            head: "1234567".into(),
            merged_at: "2026-11-05T10:00:00.000Z".into(),
            ..Default::default()
        };
        l.work_merged(&lane.owner, &merged).unwrap().unwrap();
        if yes {
            lane.verdict(HEAD, "yes");
        }
        let done = lane.handoff(HEAD, "done");
        if pipeline == "v20" {
            assert!(
                matches!(&done, Err(Refusal::GateDenied { gate, .. }) if gate == "done_verdict")
            );
            continue;
        }
        done.unwrap();
        let events = l.work_events(1).unwrap();
        let done = events.iter().rfind(|e| e.action == "handoff").unwrap();
        let detail: Value = serde_json::from_str(&done.detail_json).unwrap();
        let flag = if yes { Value::Null } else { json!(true) };
        assert_eq!(detail["merged_without_verdict"], flag, "{detail}");
    }
}
