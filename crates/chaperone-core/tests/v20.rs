//! Pipeline v20 on a lane: the claim that opts into it, the write rule
//! that keeps a harness check the harness's on a lane stamped with it, and
//! each of its gates both ways, on harness events written through the core
//! functions `check run` calls.

mod gated;

use chaperone_core::checks::{Attached, CheckInput};
use chaperone_core::ledger::{Actor, Applied};
use chaperone_core::pipeline::{self, Fact};
use chaperone_core::review;
use chaperone_core::verdict::{self, Harness};
use chaperone_core::work::ClaimInput;
use gated::*;
use serde_json::{Value, json};

#[test]
fn a_first_claim_may_stamp_a_newer_pipeline_only() {
    let (_dir, ledger) = open();
    lane(&ledger, 1, None).unwrap();
    let item = ledger.work_get(1, &ledger.now().unwrap()).unwrap().unwrap();
    assert_eq!(item.pipeline_id.as_deref(), Some("v19"));
    // The default, an older one and an unknown one are not newer.
    for (id, older) in [(2, "v19"), (3, "v17"), (4, "v18")] {
        refused(lane(&ledger, id, Some(older)), "newer than the default v19");
    }
    let v20 = &pipeline::newer()[0];
    lane(&ledger, 5, Some("v20")).unwrap();
    let item = ledger.work_get(5, &ledger.now().unwrap()).unwrap().unwrap();
    let stamp = (item.pipeline_id.as_deref(), item.pipeline_hash.as_deref());
    assert_eq!(stamp, (Some("v20"), Some(v20.hash.as_str())));
    assert_eq!(item.route.as_deref(), Some("code_pr"));
    // The stamp is the first claim's: a renewal or a later claim may not move it.
    let me = pair("lane", "s1");
    let again = ClaimInput {
        pipeline: Some("v20".into()),
        ..ClaimInput::default()
    };
    refused(
        ledger.work_claim(&me, 5, &again, None, false).unwrap(),
        "held",
    );
    ledger.work_release(&me, 5, None, false).unwrap().unwrap();
    refused(
        ledger.work_claim(&me, 5, &again, None, false).unwrap(),
        "is stamped v20",
    );
}

/// On a v20 lane only the harness writes `diff`, `tests`, `review`,
/// `merge_verdict` and `plan_check`; the owner's waiver and the lane's smoke
/// claim stay writable. A v19 lane keeps `check record` as it was.
#[test]
fn harness_checks_are_the_harness_s_on_a_v20_lane() {
    let (_dir, ledger) = open();
    lane(&ledger, 1, Some("v20")).unwrap();
    lane(&ledger, 2, None).unwrap();
    let fresh = pair("fresh", "s2");
    let record = |work: i64, check: &str, outcome: &str, reason: Option<&str>| {
        let input = CheckInput {
            work,
            check: check.into(),
            head: HEAD.into(),
            outcome: outcome.into(),
            reason: reason.map(String::from),
            decision: (check == "merge_verdict").then(|| "yes".into()),
            findings: (check == "plan_check" && outcome == "ran").then_some(0),
            run: None,
        };
        ledger.check_record(&fresh, &input).unwrap()
    };
    let says = "harness runs";
    refused(record(1, "merge_verdict", "ran", None), says);
    refused(record(1, "plan_check", "ran", None), says);
    refused(record(1, "plan_check", "skipped", Some("docs")), says);
    // A smoke claim that ran carries the run of its command.
    refused(record(1, "smoke", "ran", None), "runs its command");
    record(1, "smoke", "skipped", Some("nothing_runs")).unwrap();
    record(1, "quality_gate", "skipped", Some("no_code")).unwrap();
    ledger
        .plan_waive(&fresh, 1, HEAD, "coded first")
        .unwrap()
        .unwrap();
    for check in ["diff", "tests", "review"] {
        refused(raw(&ledger, 1, check, "session", json!({})), says);
        let harness = raw(&ledger, 1, check, "harness", json!({}));
        assert!(matches!(harness, Ok(Applied::Committed(_))), "{check}");
    }
    // A session's smoke is a claim: it may not carry the harness's result.
    let claim = raw(&ledger, 1, "smoke", "session", json!({"result": "pass"}));
    refused(claim, says);

    record(2, "merge_verdict", "ran", None).unwrap();
    for check in ["tests", "review"] {
        let v19 = raw(&ledger, 2, check, "session", json!({"result": "pass"}));
        assert!(matches!(v19, Ok(Applied::Committed(_))), "{check}: {v19:?}");
    }
}

/// On a v20 lane the lane's own session records its smoke, which is only
/// a claim: claimed, never passed. A v19 lane still refuses that session.
#[test]
fn the_lane_records_its_own_smoke_claim_on_v20_only() {
    let (_dir, ledger) = open();
    lane(&ledger, 1, Some("v20")).unwrap();
    lane(&ledger, 2, None).unwrap();
    let file = Attached {
        path: "out.txt".into(),
        sha256: "0".into(),
        bytes: 1,
        truncated: false,
        redactions: 0,
    };
    let smoke = |work| {
        let input = CheckInput {
            work,
            check: "smoke".into(),
            head: HEAD.into(),
            outcome: "ran".into(),
            run: Some(json!({"argv": ["bash", "s.sh"], "exit": 0})),
            ..CheckInput::default()
        };
        let lane = pair("lane", "s1");
        ledger.check_record_with(&lane, &input, std::slice::from_ref(&file), false)
    };
    smoke(1).unwrap().unwrap();
    let facts = ledger.project(1, Some(HEAD)).unwrap();
    assert_eq!(facts.get("smoke.claimed"), &Fact::Bool(true));
    assert_eq!(facts.get("smoke.passed"), &Fact::Absent);
    let result = json!({"result": "pass"});
    let with_result = raw_as(&ledger, &pair("lane", "s1"), 1, "smoke", "session", result);
    refused(with_result, "harness runs");
    refused(smoke(2).unwrap(), "has held lane 2");
}

/// A code lane climbs the ready row: each gate denies, with its id and the
/// facts it read, until the harness fact it needs is recorded, then the
/// next one denies, and the last fact passes the handoff. A harness run on
/// a commit it already passed on replays, so each retry is on what a lane
/// would really record next.
#[test]
fn ready_on_a_code_lane_needs_each_harness_fact_in_turn() {
    let lane = Gated::new("code");
    let binary = json!({"code_lines": 1, "changes_code": true, "uncounted_files": 1});
    lane.run("diff", THIRD, binary);
    let uncounted = denied("no_uncounted_files", &["commit.uncounted_files"]);
    assert_eq!(lane.ready(THIRD), uncounted);
    lane.diff(OTHER, 401, true);
    let lines = denied(
        "code_lines_at_most",
        &["commit.code_lines", "commit.size_waived"],
    );
    assert_eq!(lane.ready(OTHER), lines);
    lane.diff(HEAD, 400, true);
    let tests = denied("tests_passed", &["tests.passed"]);
    assert_eq!(lane.ready(HEAD), tests);
    lane.run("tests", HEAD, json!({"result": "fail"}));
    assert_eq!(lane.ready(HEAD), tests);
    lane.run("tests", HEAD, json!({"result": "pass"}));
    let complete = denied("review_complete", &["review.complete"]);
    assert_eq!(lane.ready(HEAD), complete);
    lane.run("review", HEAD, json!({"complete": false}));
    let earlier = lane.run("review", OTHER, json!({"complete": true}));
    assert_eq!(lane.ready(HEAD), complete);
    let review = lane.run("review", HEAD, json!({"complete": true})).event_id;
    let blocking = denied("no_blocking_findings", &["findings.open_blocking"]);
    assert_eq!(lane.ready(HEAD), blocking);
    lane.agrees(6);
    // Refuted stops blocking; disputed still blocks; waived stops.
    lane.rule(&lane.owner, review, "correctness-blocker", "refuted");
    lane.rule(&lane.lane, review, "correctness-risk", "disputed");
    assert_eq!(lane.ready(HEAD), blocking);
    lane.agrees(6);
    lane.rule(&lane.owner, review, "correctness-risk", "waived");
    // Another tree's review's blockers, not raised again here, still block.
    assert_eq!(lane.ready(HEAD), blocking);
    lane.agrees(6);
    lane.rule(
        &lane.owner,
        earlier.event_id,
        "correctness-blocker",
        "refuted",
    );
    lane.rule(&lane.owner, earlier.event_id, "correctness-risk", "waived");
    lane.agrees(6);
    assert_eq!(lane.ready(HEAD), denied("plan_or_no_code", &PLAN));
    lane.run("plan_check", OTHER, json!({}));
    let smoke = denied("smoke_or_no_code", &SMOKE);
    assert_eq!(lane.ready(HEAD), smoke);
    // A harness smoke that failed is not outvoted by the lane's claim.
    lane.run("smoke", HEAD, json!({"result": "fail"}));
    lane.claim(HEAD);
    assert_eq!(lane.ready(HEAD), smoke);
    lane.run("smoke", HEAD, json!({"result": "pass"}));
    let yes = denied("verdict_is_yes", &["verdict.decision"]);
    assert_eq!(lane.ready(HEAD), yes);
    lane.verdict(OTHER, "no");
    assert_eq!(lane.ready(HEAD), yes);
    lane.verdict(AMEND, "yes");
    let on_head = denied("verdict_on_head", &["verdict.sha", "commit.sha"]);
    assert_eq!(lane.ready(HEAD), on_head);
    lane.verdict(HEAD, "yes");
    lane.ready(&HEAD[..12]).unwrap();
    assert_eq!(lane.board(), (true, "the lane says ready".into()));
    // Ready is derived: a later NO takes it back, with no write.
    lane.verdict(THIRD, "no");
    let lost = "the lane said ready, but verdict_is_yes no longer holds";
    assert_eq!(lane.board(), (false, lost.into()));
}

/// The cap counts code lines, so test lines past it pass. The owner's size
/// waiver, refused from the lane's session and without lines above 0,
/// raises this lane's cap alone, up to its lines; a harness-produced one
/// counts for nothing.
#[test]
fn the_cap_counts_code_and_only_the_owner_raises_it() {
    let v = Gated::new("code");
    let tests = denied("tests_passed", &["tests.passed"]);
    let fields = json!({"code_lines": 400, "test_lines": 401, "changes_code": true,
        "uncounted_files": 0});
    v.run("diff", HEAD, fields);
    assert_eq!(v.ready(HEAD), tests);
    v.diff(OTHER, 450, true);
    let over = denied(
        "code_lines_at_most",
        &["commit.code_lines", "commit.size_waived"],
    );
    assert_eq!(v.ready(OTHER), over);
    let waive = |who: &Actor, lines| v.ledger.size_waive(who, 1, OTHER, lines, "one piece");
    refused(waive(&v.lane, 500).unwrap(), "has held lane 1");
    refused(waive(&v.owner, 0).unwrap(), "lines, an integer above 0");
    refused(
        raw(&v.ledger, 1, "size_waiver", "session", json!({})),
        "above 0",
    );
    // Only a session's waiver counts: the harness never writes one.
    let harness = json!({"lines": 999});
    raw_as(&v.ledger, &v.lane, 1, "size_waiver", "harness", harness).unwrap();
    assert_eq!(v.ready(OTHER), over);
    waive(&v.owner, 500).unwrap().unwrap();
    assert_eq!(v.ready(OTHER), tests);
    v.diff(THIRD, 501, true);
    assert_eq!(v.ready(THIRD), over);
    lane(&v.ledger, 2, Some("v20")).unwrap();
    let diff = json!({"code_lines": 450, "uncounted_files": 0});
    raw(&v.ledger, 2, "diff", "harness", diff).unwrap();
    let facts = v.ledger.project(2, Some(HEAD)).unwrap();
    assert_eq!(facts.get("commit.size_waived"), &Fact::Bool(false));
}

/// An amend with the same tree: the review and the verdict carried to it
/// pass, the review's findings read as the original's; the owner's waiver
/// stands for the plan check, and the lane's claim with evidence for the
/// smoke when the harness ran none.
#[test]
fn a_carried_review_and_verdict_pass_on_the_amend() {
    let lane = Gated::new("code");
    let review = lane.run("review", HEAD, json!({"complete": true})).event_id;
    lane.rule(&lane.owner, review, "correctness-blocker", "refuted");
    lane.diff(AMEND, 10, true);
    lane.run("tests", AMEND, json!({"result": "pass"}));
    // Carried as `check run review` carries a complete review of the tree.
    let v20 = &pipeline::newer()[0];
    let plan = review::plan(v20).unwrap();
    let carried = review::replay(&lane.ledger, &lane.owner, &mut lane.amend(), &plan);
    let carried = carried.unwrap().unwrap().unwrap();
    assert_eq!(carried.payload.rest["carried_from"], json!(review));
    let blocking = denied("no_blocking_findings", &["findings.open_blocking"]);
    assert_eq!(lane.ready(AMEND), blocking);
    lane.rule(&lane.owner, review, "correctness-risk", "verified");
    assert_eq!(lane.ready(AMEND), blocking);
    lane.rule(&lane.owner, review, "correctness-risk", "waived");
    assert_eq!(lane.ready(AMEND), denied("plan_or_no_code", &PLAN));
    let waive = lane.ledger.plan_waive(&lane.owner, 1, HEAD, "coded first");
    waive.unwrap().unwrap();
    assert_eq!(lane.ready(AMEND), denied("smoke_or_no_code", &SMOKE));
    lane.claim(AMEND);
    assert_eq!(
        lane.ready(AMEND),
        denied("verdict_is_yes", &["verdict.decision"])
    );
    // The verdict on HEAD's tree, carried to the amend as `check run
    // verdict` carries it.
    lane.verdict(HEAD, "yes");
    let facts = Harness {
        shown: Value::Null,
        sha256: "f".into(),
        read: Value::Null,
    };
    let limits = (v20.hash.as_str(), 2);
    let input = &mut lane.amend();
    let carried = verdict::replay(&lane.ledger, &lane.owner, input, limits, &facts);
    let carried = carried.unwrap().unwrap().unwrap();
    assert!(!carried.replayed);
    assert!(carried.payload.rest.contains_key("carried_from"));
    lane.ready(AMEND).unwrap();
}

/// A lane whose harness diff changes no code needs no plan check and no
/// smoke for ready, and no verdict for done; its declared kind does not
/// decide that, the diff does, and without one done needs both.
#[test]
fn a_lane_that_changes_no_code_skips_the_plan_and_the_smoke() {
    let lane = Gated::new("code");
    lane.diff(HEAD, 0, false);
    lane.run("tests", HEAD, json!({"result": "pass"}));
    let review = lane.run("review", HEAD, json!({"complete": true})).event_id;
    for key in ["correctness-blocker", "correctness-risk"] {
        lane.rule(&lane.owner, review, key, "refuted");
    }
    assert_eq!(
        lane.ready(HEAD),
        denied("verdict_is_yes", &["verdict.decision"])
    );
    lane.handoff(HEAD, "done").unwrap();

    // Declared docs, with no harness diff and then one that changes code:
    // the full row applies.
    let docs = Gated::new("docs");
    assert_eq!(docs.handoff(HEAD, "done"), denied("plan_or_no_code", &PLAN));
    docs.run("plan_check", OTHER, json!({}));
    assert_eq!(docs.handoff(HEAD, "done"), denied("done_verdict", &DONE));
    docs.diff(HEAD, 5, true);
    assert_eq!(docs.handoff(HEAD, "done"), denied("done_verdict", &DONE));
    docs.run("merge_verdict", HEAD, json!({"decision": "yes"}));
    docs.handoff(HEAD, "done").unwrap();
}

/// A plan check and a verdict a session wrote before the v20 claim, when
/// no rule refused them, satisfy no gate; the owner's waiver still does.
#[test]
fn a_session_plan_check_or_verdict_is_no_harness_fact() {
    let lane = Gated::unclaimed("code");
    let fresh = pair("fresh", "s2");
    for (check, decision, findings) in [
        ("plan_check", None, Some(0)),
        ("merge_verdict", Some("yes"), None),
    ] {
        let input = CheckInput {
            work: 1,
            check: check.into(),
            head: HEAD.into(),
            outcome: "ran".into(),
            decision: decision.map(String::from),
            findings,
            ..CheckInput::default()
        };
        lane.ledger.check_record(&fresh, &input).unwrap().unwrap();
    }
    let claim = ClaimInput {
        pipeline: Some("v20".into()),
        ..ClaimInput::default()
    };
    let claimed = lane.ledger.work_claim(&lane.lane, 1, &claim, None, false);
    claimed.unwrap().unwrap();
    lane.diff(HEAD, 10, true);
    lane.run("tests", HEAD, json!({"result": "pass"}));
    let review = lane.run("review", HEAD, json!({"complete": true})).event_id;
    for key in ["correctness-blocker", "correctness-risk"] {
        lane.rule(&lane.owner, review, key, "refuted");
    }
    assert_eq!(lane.ready(HEAD), denied("plan_or_no_code", &PLAN));
    let waive = lane.ledger.plan_waive(&lane.owner, 1, HEAD, "coded first");
    waive.unwrap().unwrap();
    lane.claim(HEAD);
    let yes = denied("verdict_is_yes", &["verdict.decision"]);
    assert_eq!(lane.ready(HEAD), yes);
    assert_eq!(lane.handoff(HEAD, "done"), denied("done_verdict", &DONE));
}
