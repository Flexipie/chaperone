//! Pipeline v20 on a lane: the claim that opts into it, and the write rule
//! that keeps a harness check the harness's on a lane stamped with it.

use chaperone_core::checks::CheckInput;
use chaperone_core::ledger::{Actor, Applied, EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::pipeline;
use chaperone_core::work::{ClaimInput, CreateInput};
use serde_json::{Value, json};

const HEAD: &str = "abc1234abc1234abc1234abc1234abc1234abc12";

fn pair(agent: &str, session: &str) -> Actor {
    Actor {
        agent: Some(agent.into()),
        session: Some(session.into()),
    }
}

/// A new code lane, `id`, claimed by `lane`/`s1` on `stamp` (None: the
/// default).
fn lane(ledger: &Ledger, id: i64, stamp: Option<&str>) -> Result<(), Refusal> {
    let create = CreateInput {
        title: format!("lane {id}"),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let lane = pair("lane", "s1");
    ledger
        .work_create(&lane, &create, None, false)
        .unwrap()
        .unwrap();
    let claim = ClaimInput {
        pipeline: stamp.map(String::from),
        ..ClaimInput::default()
    };
    ledger
        .work_claim(&lane, id, &claim, None, false)
        .unwrap()
        .map(|_| ())
}

fn open() -> (tempfile::TempDir, Ledger) {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
    (dir, ledger)
}

fn refused(r: Result<impl std::fmt::Debug, Refusal>, says: &str) {
    match r {
        Err(Refusal::InvalidInput(m)) => assert!(m.contains(says), "{m}"),
        other => panic!("expected a refusal saying {says:?}, got {other:?}"),
    }
}

/// A `check.finished` `ran` on lane `work` from `producer`, as no command
/// writes it but `apply` takes it.
fn raw(
    ledger: &Ledger,
    work: i64,
    check: &str,
    producer: &str,
    rest: Value,
) -> Result<Applied, Refusal> {
    let mut p = Payload {
        check: Some(check.into()),
        outcome: Some("ran".into()),
        head_sha: Some(HEAD.into()),
        ..Payload::default()
    };
    if let Value::Object(rest) = rest {
        p.rest = rest;
    }
    p.rest.insert("producer".into(), json!({"kind": producer}));
    let event = Mutation::Event {
        kind: EventKind::CheckFinished,
        lane_id: Some(work),
        spec_id: None,
        payload: p,
    };
    let at = ledger.now().unwrap();
    match ledger
        .apply(&pair("fresh", "s2"), None, &at, event)
        .unwrap()
    {
        Applied::Refused(r) => Err(r),
        applied => Ok(applied),
    }
}

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
        };
        ledger.check_record(&fresh, &input).unwrap()
    };
    let says = "harness runs";
    refused(record(1, "merge_verdict", "ran", None), says);
    refused(record(1, "plan_check", "ran", None), says);
    refused(record(1, "plan_check", "skipped", Some("docs")), says);
    record(1, "smoke", "ran", None).unwrap();
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
    let v19 = raw(&ledger, 2, "tests", "session", json!({"result": "pass"}));
    assert!(matches!(v19, Ok(Applied::Committed(_))), "{v19:?}");
}
