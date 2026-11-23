//! A code lane on a harness pipeline (v20 on), for the tests of its gates:
//! harness events written through the core functions `check run` calls,
//! and the handoffs the gates judge.

// Each test binary uses part of this module.
#![allow(dead_code)]

use chaperone_core::checks::{Attached, CheckInput, FindingMove, Recorded, RunInput};
use chaperone_core::ledger::{
    Actor, Applied, EventKind, Finding, Findings, Ledger, Mutation, Payload, Refusal,
};
use chaperone_core::pipeline::{self, Fact, Pipeline};
use chaperone_core::work::{ClaimInput, CreateInput, HandoffInput};
use serde_json::{Map, Value, json};

pub const HEAD: &str = "abc1234abc1234abc1234abc1234abc1234abc12";

pub fn pair(agent: &str, session: &str) -> Actor {
    Actor {
        agent: Some(agent.into()),
        session: Some(session.into()),
    }
}

/// A new code lane, `id`, claimed by `lane`/`s1` on `stamp` (None: the
/// default).
pub fn lane(ledger: &Ledger, id: i64, stamp: Option<&str>) -> Result<(), Refusal> {
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

pub fn open() -> (tempfile::TempDir, Ledger) {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
    (dir, ledger)
}

pub fn refused(r: Result<impl std::fmt::Debug, Refusal>, says: &str) {
    match r {
        Err(Refusal::InvalidInput(m)) => assert!(m.contains(says), "{m}"),
        other => panic!("expected a refusal saying {says:?}, got {other:?}"),
    }
}

/// A `check.finished` `ran` on lane `work` from `producer`, as no command
/// writes it but `apply` takes it, written by `fresh`/`s2`.
pub fn raw(
    ledger: &Ledger,
    work: i64,
    check: &str,
    producer: &str,
    rest: Value,
) -> Result<Applied, Refusal> {
    raw_as(ledger, &pair("fresh", "s2"), work, check, producer, rest)
}

pub fn raw_as(
    ledger: &Ledger,
    actor: &Actor,
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
    match ledger.apply(actor, None, &at, event).unwrap() {
        Applied::Refused(r) => Err(r),
        applied => Ok(applied),
    }
}

/// Another commit, and one with the same tree as `HEAD` (an amend).
pub const OTHER: &str = "def5678def5678def5678def5678def5678def56";
pub const AMEND: &str = "0123456789abcdef0123456789abcdef01234567";
pub const THIRD: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// Lane 1 of `kind` on v20, claimed by the lane session `s1`; `owner` has
/// never held it, as the verifier or the owner.
pub struct Gated {
    pub _dir: tempfile::TempDir,
    pub ledger: Ledger,
    pub lane: Actor,
    pub owner: Actor,
}

impl Gated {
    pub fn new(kind: &str) -> Gated {
        Gated::on("v20", kind)
    }

    /// Lane 1 of `kind`, claimed on `pipeline`.
    pub fn on(pipeline: &str, kind: &str) -> Gated {
        let lane = Gated::unclaimed(kind);
        let claim = ClaimInput {
            pipeline: Some(pipeline.into()),
            ..ClaimInput::default()
        };
        lane.ledger
            .work_claim(&lane.lane, 1, &claim, None, false)
            .unwrap()
            .unwrap();
        lane
    }

    /// Lane 1 of `kind`, created by the lane session and not yet claimed.
    pub fn unclaimed(kind: &str) -> Gated {
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
        Gated {
            _dir: dir,
            ledger,
            lane: me,
            owner: pair("owner", "s3"),
        }
    }

    /// The pipeline lane 1 is stamped with.
    pub fn pipeline(&self) -> &'static Pipeline {
        let now = self.ledger.now().unwrap();
        let item = self.ledger.work_get(1, &now).unwrap().unwrap();
        pipeline::by_id(item.pipeline_id.as_deref().unwrap()).unwrap()
    }

    /// `finding list` marks as blocking exactly what the gate counts on
    /// HEAD, and lists `listed` findings (complete reviews' only).
    pub fn agrees(&self, listed: usize) {
        let found = self.ledger.lane_findings(1).unwrap();
        let blocks = found.iter().filter(|f| f.blocks).count();
        let facts = self.ledger.project(1, Some(HEAD)).unwrap();
        let open = facts.get("findings.open_blocking");
        assert_eq!((open, found.len()), (&Fact::Int(blocks as i64), listed));
    }

    /// The harness's `check` on `head`, as `check run` records it.
    pub fn run(&self, check: &str, head: &str, fields: Value) -> Recorded {
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
            input.profile = Some(self.pipeline().hash.clone());
        }
        self.ledger.check_run(&self.owner, &input).unwrap().unwrap()
    }

    pub fn diff(&self, head: &str, code_lines: i64, changes_code: bool) {
        let fields = json!({"code_lines": code_lines, "changes_code": changes_code,
            "uncounted_files": 0});
        self.run("diff", head, fields);
    }

    /// A harness verdict on `head`; each on its own facts, so none replays.
    pub fn verdict(&self, head: &str, decision: &str) {
        let fields = json!({"decision": decision, "facts_sha256": "f"});
        self.run("merge_verdict", head, fields);
    }

    /// A fresh session's smoke claim on `head`, its run and evidence file, as
    /// `check record --check smoke --run` writes it.
    pub fn claim(&self, head: &str) {
        let input = CheckInput {
            work: 1,
            check: "smoke".into(),
            head: head.into(),
            outcome: "ran".into(),
            run: Some(json!({"argv": ["bash", "s.sh"], "exit": 0})),
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
    pub fn amend(&self) -> RunInput {
        RunInput {
            work: 1,
            head: AMEND.into(),
            tree: tree(AMEND).into(),
            bin: "chaperone".into(),
            version: "0.1.0".into(),
            ..RunInput::default()
        }
    }

    pub fn rule(&self, who: &Actor, review: i64, key: &str, status: &str) {
        let input = FindingMove {
            check_event_id: review,
            finding_key: key.into(),
            status: status.into(),
            reason: Some("because".into()),
            waiver_kind: (status == "waived").then(|| "disproved".into()),
            waiver_ref: (status == "waived").then(|| "t.rs:1".into()),
            ..FindingMove::default()
        };
        self.ledger
            .finding_update(who, &input, false)
            .unwrap()
            .unwrap();
    }

    pub fn handoff(&self, commit: &str, status: &str) -> Result<i64, Refusal> {
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

    pub fn ready(&self, commit: &str) -> Result<i64, Refusal> {
        self.handoff(commit, "review")
    }

    /// The board's ready and its reason.
    pub fn board(&self) -> (bool, String) {
        let now = self.ledger.now().unwrap();
        let item = self.ledger.work_board_item(1, &now).unwrap().unwrap();
        (item.ready, item.ready_reason)
    }
}

/// An amend keeps the tree, so `AMEND` has `HEAD`'s.
pub fn tree(head: &str) -> &'static str {
    if head == OTHER {
        "tree-other"
    } else {
        "tree-head"
    }
}

pub fn denied(gate: &str, fact_paths: &[&str]) -> Result<i64, Refusal> {
    Err(Refusal::GateDenied {
        transition: "handoff".into(),
        gate: gate.into(),
        fact_paths: fact_paths.iter().map(|s| s.to_string()).collect(),
    })
}

pub const PLAN: [&str; 3] = [
    "commit.changes_code",
    "plan_check.outcome",
    "plan_check.reason",
];
pub const SMOKE: [&str; 3] = ["commit.changes_code", "smoke.passed", "smoke.claimed"];
pub const DONE: [&str; 4] = [
    "commit.changes_code",
    "verdict.decision",
    "verdict.sha",
    "commit.sha",
];
