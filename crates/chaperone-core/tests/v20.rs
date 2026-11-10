//! Pipeline v20 on a lane: the claim that opts into it, the write rule
//! that keeps a harness check the harness's on a lane stamped with it, and
//! each of its gates both ways, on harness events written through the core
//! functions `check run` calls.

use chaperone_core::checks::{Attached, CheckInput, FindingMove, Recorded, RunInput};
use chaperone_core::ledger::{
    Actor, Applied, EventKind, Finding, Findings, Ledger, Mutation, Payload, Refusal,
};
use chaperone_core::pipeline::{self, Denied, FactBundle, Phase, Route};
use chaperone_core::review;
use chaperone_core::verdict::{self, Harness};
use chaperone_core::work::{ClaimInput, CreateInput, HandoffInput};
use serde_json::{Map, Value, json};

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
    for check in ["tests", "review"] {
        let v19 = raw(&ledger, 2, check, "session", json!({"result": "pass"}));
        assert!(matches!(v19, Ok(Applied::Committed(_))), "{check}: {v19:?}");
    }
}

/// Another commit, and one with the same tree as `HEAD` (an amend).
const OTHER: &str = "def5678def5678def5678def5678def5678def56";
const AMEND: &str = "0123456789abcdef0123456789abcdef01234567";
const THIRD: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// Lane 1 of `kind` on v20, claimed by the lane session `s1`; `owner` has
/// never held it, as the verifier or the owner.
struct V20 {
    _dir: tempfile::TempDir,
    ledger: Ledger,
    lane: Actor,
    owner: Actor,
}

impl V20 {
    fn new(kind: &str) -> V20 {
        let (dir, ledger) = open();
        let create = CreateInput {
            title: "t".into(),
            kind: kind.into(),
            pr_flow: "pr".into(),
            ..CreateInput::default()
        };
        let me = pair("lane", "s1");
        ledger
            .work_create(&me, &create, None, false)
            .unwrap()
            .unwrap();
        let claim = ClaimInput {
            pipeline: Some("v20".into()),
            ..ClaimInput::default()
        };
        ledger
            .work_claim(&me, 1, &claim, None, false)
            .unwrap()
            .unwrap();
        V20 {
            _dir: dir,
            ledger,
            lane: me,
            owner: pair("owner", "s3"),
        }
    }

    /// The harness's `check` on `head`, as `check run` records it.
    fn run(&self, check: &str, head: &str, fields: Value) -> Recorded {
        let mut input = RunInput {
            work: 1,
            check: check.into(),
            head: head.into(),
            tree: tree(head).into(),
            fields: fields.as_object().cloned().unwrap_or_default(),
            bin: "chaperone".into(),
            version: "0.1.0".into(),
            ..RunInput::default()
        };
        if check == "review" && !input.fields.contains_key("carried_from") {
            let found = ["blocker", "risk", "nit"].map(|severity| Finding {
                finding_key: format!("correctness-{severity}"),
                severity: severity.into(),
                title: "t".into(),
                body: "b".into(),
                path: Some("a.rs".into()),
                rest: Map::new(),
            });
            input.findings = Some(Findings::List(found.into()));
        }
        // As `check run review` and `verdict` key them: by the pipeline.
        if matches!(check, "review" | "merge_verdict") {
            input.profile = Some(pipeline::newer()[0].hash.clone());
        }
        self.ledger.check_run(&self.owner, &input).unwrap().unwrap()
    }

    fn diff(&self, head: &str, code_lines: i64, changes_code: bool) {
        let fields = json!({"code_lines": code_lines, "changes_code": changes_code});
        self.run("diff", head, fields);
    }

    /// A harness verdict on `head`; each on its own facts, so none replays.
    fn verdict(&self, head: &str, decision: &str) {
        let fields = json!({"decision": decision, "facts_sha256": "f"});
        self.run("merge_verdict", head, fields);
    }

    /// A fresh session's smoke claim on `head`, with an evidence file, as
    /// `check record --check smoke --evidence` writes it.
    fn claim(&self, head: &str) {
        let input = CheckInput {
            work: 1,
            check: "smoke".into(),
            head: head.into(),
            outcome: "ran".into(),
            ..CheckInput::default()
        };
        let file = Attached {
            path: "out.txt".into(),
            sha256: "0".into(),
            bytes: 1,
            truncated: false,
            redactions: 0,
        };
        let fresh = pair("fresh", "s2");
        let r = self
            .ledger
            .check_record_with(&fresh, &input, &[file], false);
        r.unwrap().unwrap();
    }

    /// `check run`'s input on the amend, before the check names itself.
    fn amend(&self) -> RunInput {
        RunInput {
            work: 1,
            head: AMEND.into(),
            tree: tree(AMEND).into(),
            bin: "chaperone".into(),
            version: "0.1.0".into(),
            ..RunInput::default()
        }
    }

    fn rule(&self, who: &Actor, review: i64, key: &str, status: &str) {
        let input = FindingMove {
            check_event_id: review,
            finding_key: key.into(),
            status: status.into(),
            reason: Some("because".into()),
            ..FindingMove::default()
        };
        self.ledger
            .finding_update(who, &input, false)
            .unwrap()
            .unwrap();
    }

    fn handoff(&self, commit: &str, status: &str) -> Result<i64, Refusal> {
        let input = HandoffInput {
            summary: "s".into(),
            status: Some(status.into()),
            commit: Some(commit.into()),
            ready: (status == "review").then_some(true),
            ..HandoffInput::default()
        };
        let h = self
            .ledger
            .work_handoff(&self.lane, 1, &input, None, None, false);
        h.unwrap().map(|h| h.id)
    }

    fn ready(&self, commit: &str) -> Result<i64, Refusal> {
        self.handoff(commit, "review")
    }

    /// The board's ready and its reason.
    fn board(&self) -> (bool, String) {
        let now = self.ledger.now().unwrap();
        let item = self.ledger.work_board_item(1, &now).unwrap().unwrap();
        (item.ready, item.ready_reason)
    }
}

/// An amend keeps the tree, so `AMEND` has `HEAD`'s.
fn tree(head: &str) -> &'static str {
    if head == OTHER {
        "tree-other"
    } else {
        "tree-head"
    }
}

fn denied(gate: &str, fact_paths: &[&str]) -> Result<i64, Refusal> {
    Err(Refusal::GateDenied {
        transition: "handoff".into(),
        gate: gate.into(),
        fact_paths: fact_paths.iter().map(|s| s.to_string()).collect(),
    })
}

const PLAN: [&str; 3] = [
    "commit.changes_code",
    "plan_check.outcome",
    "plan_check.reason",
];
const SMOKE: [&str; 3] = ["commit.changes_code", "smoke.passed", "smoke.claimed"];

/// A code lane climbs the ready row: each gate denies, with its id and the
/// facts it read, until the harness fact it needs is recorded, then the
/// next one denies, and the last fact passes the handoff. A harness run on
/// a commit it already passed on replays, so each retry is on what a lane
/// would really record next.
#[test]
fn ready_on_a_code_lane_needs_each_harness_fact_in_turn() {
    let lane = V20::new("code");
    let lines = denied("code_lines_at_most", &["commit.code_lines"]);
    assert_eq!(lane.ready(HEAD), lines);
    lane.diff(OTHER, 401, true);
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
    assert_eq!(lane.ready(HEAD), complete);
    let review = lane.run("review", HEAD, json!({"complete": true})).event_id;
    let blocking = denied("no_blocking_findings", &["findings.open_blocking"]);
    assert_eq!(lane.ready(HEAD), blocking);
    // Refuted stops blocking; disputed still blocks; waived stops.
    lane.rule(&lane.owner, review, "correctness-blocker", "refuted");
    lane.rule(&lane.lane, review, "correctness-risk", "disputed");
    assert_eq!(lane.ready(HEAD), blocking);
    lane.rule(&lane.owner, review, "correctness-risk", "waived");
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

/// An amend with the same tree: the review and the verdict carried to it
/// pass, the review's findings read as the original's; the owner's waiver
/// stands for the plan check, and the lane's claim with evidence for the
/// smoke when the harness ran none.
#[test]
fn a_carried_review_and_verdict_pass_on_the_amend() {
    let lane = V20::new("code");
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
/// decide that, the diff does.
#[test]
fn a_lane_that_changes_no_code_skips_the_plan_and_the_smoke() {
    let lane = V20::new("code");
    lane.run(
        "diff",
        HEAD,
        json!({"code_lines": 0, "changes_code": false}),
    );
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

    // Declared docs, but its diff changes code: the full row applies.
    let docs = V20::new("docs");
    docs.run("diff", HEAD, json!({"code_lines": 5, "changes_code": true}));
    let done = [
        "commit.changes_code",
        "verdict.decision",
        "verdict.sha",
        "commit.sha",
    ];
    assert_eq!(docs.handoff(HEAD, "done"), denied("plan_or_no_code", &PLAN));
    docs.run("plan_check", OTHER, json!({}));
    assert_eq!(docs.handoff(HEAD, "done"), denied("done_verdict", &done));
    docs.run("merge_verdict", HEAD, json!({"decision": "yes"}));
    docs.handoff(HEAD, "done").unwrap();
}

/// `stamp_ok` holds on every v20 lane (the climb above); its denial is the
/// stamp of another pipeline, which no v20 lane can carry.
#[test]
fn stamp_ok_denies_another_stamp() {
    let mut facts = FactBundle::default();
    facts.set("pipeline.id", "v19");
    let v20 = &pipeline::newer()[0];
    let denial = v20.evaluate("handoff", Route::CodePr, Phase::Ready, &facts);
    let expected = Denied {
        gate: "stamp_ok".into(),
        fact_paths: vec!["pipeline.id".into()],
    };
    assert_eq!(denial, Err(expected));
}
