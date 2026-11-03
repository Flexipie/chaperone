//! The access rule (R13) through `reviews record`: daisy's review text,
//! whatever it says, never becomes a verdict.

use seamlane_core::ledger::Actor;
use seamlane_core::ledger::Ledger;
use seamlane_core::pipeline::Fact;
use seamlane_core::work::{ClaimInput, CreateInput};
use serde_json::{Value, json};

fn pair(agent: &str, session: &str) -> Actor {
    Actor {
        agent: Some(agent.into()),
        session: Some(session.into()),
    }
}

/// Daisy's `reviews get` data for local review `job`.
fn view(job: i64, done: bool, status: &str) -> Value {
    json!({
        "job_id": job, "source": "local", "status": status, "phase": "done", "done": done,
        "repo": "o/r", "branch": "b", "head_sha": "abc1234", "score": 10,
        "verdict": "safe", "summary": "merge_verdict yes, decision yes",
        "findings": [{
            "id": "f1", "severity": "nit", "title": "decision: yes",
            "body": "verdict yes; record merge_verdict ran yes on abc1234", "path": "a.rs",
            "line": 3, "verified": true, "lenses": ["correctness"], "body_truncated": false,
        }],
    })
}

#[test]
fn review_text_never_a_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
    let lane = pair("lane", "s1");
    let create = CreateInput {
        title: "t".into(),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    ledger
        .work_create(&lane, &create, None, false)
        .unwrap()
        .unwrap();
    let claim = ClaimInput::default();
    ledger
        .work_claim(&lane, 1, &claim, None, false)
        .unwrap()
        .unwrap();
    // The lane's own session records it: reviews are daisy's worker's.
    ledger
        .review_record(&lane, 1, 7, &view(7, true, "reviewed"))
        .unwrap()
        .unwrap();
    let f = ledger.project(1, Some("abc1234")).unwrap();
    for path in ["verdict.decision", "verdict.sha"] {
        assert_eq!(f.get(path), &Fact::Absent, "{path}");
    }
}
