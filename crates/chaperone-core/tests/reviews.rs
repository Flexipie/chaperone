//! The access rule (R13) through `reviews record`: daisy's review text,
//! whatever it says, never becomes a verdict.

use chaperone_core::ledger::Actor;
use chaperone_core::ledger::{Ledger, Refusal};
use chaperone_core::pipeline::Fact;
use chaperone_core::work::{ClaimInput, CreateInput};
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

/// Lane 1, repo `O/R` on branch `b`, claimed by `lane`/`s1`.
fn lane(dir: &tempfile::TempDir) -> (Ledger, Actor) {
    let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
    let lane = pair("lane", "s1");
    let create = CreateInput {
        title: "t".into(),
        repo: Some("O/R".into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    ledger
        .work_create(&lane, &create, None, false)
        .unwrap()
        .unwrap();
    let claim = ClaimInput {
        branch: Some("b".into()),
        ..ClaimInput::default()
    };
    ledger
        .work_claim(&lane, 1, &claim, None, false)
        .unwrap()
        .unwrap();
    (ledger, lane)
}

#[test]
fn review_text_never_a_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let (ledger, lane) = lane(&dir);
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

#[test]
fn review_from_another_lane_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ledger, lane) = lane(&dir);
    let with = |job: i64, k: &str, v: Value| {
        let mut v2 = view(job, true, "reviewed");
        v2[k] = v;
        v2
    };
    for (job, bad) in [
        (8, with(8, "repo", json!("x/y"))),
        (9, with(9, "branch", json!("c"))),
    ] {
        let r = ledger.review_record(&lane, 1, job, &bad).unwrap();
        assert!(matches!(r, Err(Refusal::InvalidInput(_))), "{r:?}");
        assert!(matches!(
            ledger.review_get(job).unwrap(),
            Err(Refusal::NotFound { .. })
        ));
    }
    // Repo ignores case; a PR job has no branch and the lane no pr_ref yet.
    let mut pr = with(10, "source", json!("pr"));
    pr["branch"] = Value::Null;
    pr["pr_ref"] = json!("o/r#5");
    for (job, good) in [(7, view(7, true, "reviewed")), (10, pr)] {
        ledger.review_record(&lane, 1, job, &good).unwrap().unwrap();
    }
}
