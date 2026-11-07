//! `stats`: R11's lane and cohort numbers folded from the log, on a temp
//! ledger. A number nothing records yet is null and named in `missing`.

mod common;

use std::time::Duration;

use chaperone_core::checks::{AttentionInput, CheckInput, MergedInput};
use chaperone_core::ledger::{EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::stats::Cohort;
use chaperone_core::work::{ClaimInput, CreateInput, HandoffInput, UpdateInput};
use common::{AT, actor, committed};
use serde_json::{Value, json};

const HEAD: &str = "abc1234def";

/// A claimed lane on repo `o/r`, branch `b`, holding PR `o/r#<id>`.
fn lane(ledger: &Ledger) -> i64 {
    let a = actor("1");
    let input = CreateInput {
        title: "t".into(),
        repo: Some("o/r".into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let id = ledger
        .work_create(&a, &input, None, false)
        .unwrap()
        .unwrap()
        .id;
    let claim = ClaimInput {
        branch: Some("b".into()),
        ..ClaimInput::default()
    };
    let item = ledger
        .work_claim(&a, id, &claim, None, false)
        .unwrap()
        .unwrap();
    let update = UpdateInput {
        pr_ref: Some(format!("o/r#{id}")),
        ..UpdateInput::default()
    };
    ledger
        .work_update(&a, id, &update, Some(&item.updated_at), None, false)
        .unwrap()
        .unwrap();
    id
}

fn check(ledger: &Ledger, work: i64, check: &str, decision: Option<&str>, findings: Option<u64>) {
    let input = CheckInput {
        work,
        check: check.into(),
        head: HEAD[..7].into(),
        outcome: "ran".into(),
        decision: decision.map(Into::into),
        findings,
        ..CheckInput::default()
    };
    ledger
        .check_record(&actor("check"), &input)
        .unwrap()
        .unwrap();
}

fn merged(ledger: &Ledger, work: i64, merged_at: &str) {
    let input = MergedInput {
        work,
        pr: format!("o/r#{work}"),
        merge_sha: "fedcba9".into(),
        head: HEAD.into(),
        merged_at: merged_at.into(),
        patch_id: None,
    };
    ledger.work_merged(&actor("9"), &input).unwrap().unwrap();
}

fn usage(ledger: &Ledger, work: i64, id: &str, source: &str, input_total: i64) {
    let payload = Payload {
        source: Some(source.into()),
        rest: json!({
            "message_id": id, "input_total": input_total, "cache_read": input_total - 10,
            "cache_write": 5, "output_total": 3,
        })
        .as_object()
        .unwrap()
        .clone(),
        ..Payload::default()
    };
    let event = Mutation::Event {
        kind: EventKind::UsageReported,
        lane_id: Some(work),
        spec_id: None,
        payload,
    };
    let key = format!("usage:{work}:{id}");
    committed(ledger.apply(&actor("1"), Some(&key), AT, event).unwrap());
}

/// Daisy's view of local review `job` with one verified finding `title`.
fn review(ledger: &Ledger, work: i64, job: i64, score: i64, head: &str, title: &str) {
    let view = json!({
        "job_id": job, "source": "local", "status": "reviewed", "phase": "done", "done": true,
        "repo": "o/r", "branch": "b", "head_sha": head, "score": score, "verdict": "safe",
        "findings": [{
            "id": "f1", "severity": "risk", "title": title, "body": "b",
            "path": "a.rs", "line": 1, "verified": true, "lenses": [], "body_truncated": false,
        }],
    });
    ledger
        .review_record(&actor("1"), work, job, &view)
        .unwrap()
        .unwrap();
}

fn stats(ledger: &Ledger, cohort: Cohort) -> Value {
    ledger.stats(&cohort).unwrap().unwrap()
}

#[test]
fn a_lane_folds_from_its_events() {
    let (_dir, _path, ledger) = common::ledger();
    let id = lane(&ledger);
    usage(&ledger, id, "m1", "parent", 100);
    usage(&ledger, id, "m2", "parent", 300);
    usage(&ledger, id, "m3", "subagent", 900);
    check(&ledger, id, "plan_check", None, Some(2));
    check(&ledger, id, "quality_gate", None, Some(0));
    check(&ledger, id, "merge_verdict", Some("yes"), None);
    for (kind, round) in [
        ("nudged", None),
        ("sent_back", Some(1)),
        ("sent_back", Some(2)),
    ] {
        let input = AttentionInput {
            work: id,
            kind: kind.into(),
            by: "orchestrator".into(),
            round,
            idempotency_key: Some(format!("{kind}{round:?}")),
            ..AttentionInput::default()
        };
        ledger
            .attention_record(&actor("o"), &input)
            .unwrap()
            .unwrap();
    }
    // Recorded out of order: job 8 (score 6) first, then job 7 (score 8).
    // In daisy's job order the score fell, so one flat round.
    review(&ledger, id, 8, 6, HEAD, "Same Bug");
    let handoff = HandoffInput {
        summary: "s".into(),
        next_action: Some("needs you: look".into()),
        ..HandoffInput::default()
    };
    ledger
        .work_handoff(&actor("1"), id, &handoff, None, false)
        .unwrap()
        .unwrap();
    std::thread::sleep(Duration::from_millis(5));
    review(&ledger, id, 7, 8, "1111111", " same bug");
    merged(&ledger, id, "2099-01-01T00:00:00.000Z");

    let out = stats(&ledger, Cohort::Work(vec![id, id]));
    let lane = &out["lanes"][0];
    assert_eq!(
        out["lanes"].as_array().unwrap().len(),
        1,
        "a repeated id counts once"
    );
    assert_eq!(lane["merged"], true);
    assert!(lane["claim_to_merge_h"].as_f64().unwrap() > 0.0);
    assert_eq!(
        lane["tokens"]["parent"],
        json!({"input_total": 400, "cache_read": 380, "cache_write": 10, "output_total": 6})
    );
    assert_eq!(lane["tokens"]["subagent"]["input_total"], 900);
    // The subagent's 900 does not raise the parent peak.
    assert_eq!(lane["peak_context"], 300);
    assert_eq!(lane["attention"], json!({"nudged": 1, "sent_back": 2}));
    assert_eq!(lane["send_back_rounds"], 2);
    assert_eq!(lane["plan_check"]["findings"], 2);
    assert_eq!(lane["quality_gate"]["outcome"], "ran");
    // The verdict's 7-digit sha is a prefix of the merged head.
    assert_eq!(lane["verdict_on_head"], true);
    assert_eq!(lane["head_reviewed"], true);
    assert_eq!(lane["flat_rounds"], 1);
    assert_eq!(lane["reraised_findings"], 1);
    // One finding (same key in both jobs), counted once; its latest record
    // in job order is job 8's, recorded before the handoff.
    assert_eq!(lane["verified_findings"], 1);
    assert_eq!(lane["verified_findings_after_handoff"], 0);

    let cohort = &out["cohort"];
    assert_eq!(cohort["lanes"], 1);
    assert_eq!(cohort["too_few_lanes"], true);
    assert_eq!(cohort["peak_context_median"], 300.0);
    assert_eq!(cohort["context_tokens_per_merged_pr_median"], 400.0);
    assert_eq!(cohort["lanes_plan_checked"], 1);
    assert_eq!(cohort["lanes_quality_gated"], 1);
    assert_eq!(cohort["lanes_verdict_on_head"], 1);
    assert_eq!(cohort["lanes_head_reviewed"], 1);
    assert_eq!(cohort["attention"]["sent_back"], 2);
    assert_eq!(cohort["attention"]["revived"], 0);
    let missing: Vec<&String> = cohort["missing"].as_object().unwrap().keys().collect();
    assert_eq!(missing, ["lanes_with_commits_after_handoff"]);
    assert_eq!(cohort["lanes_reviewed"], 1);
}

#[test]
fn without_reviews_review_numbers_are_null_and_named() {
    let (_dir, _path, ledger) = common::ledger();
    let id = lane(&ledger);
    check(&ledger, id, "merge_verdict", Some("no"), None);
    merged(&ledger, id, "2026-11-06T10:00:00.000Z");
    let out = stats(&ledger, Cohort::MergedSince("2026-11-06".into()));
    let lane = &out["lanes"][0];
    // A NO is the latest verdict on the merged head.
    assert_eq!(lane["verdict_on_head"], false);
    // No usage: unknown, never zero.
    assert_eq!(lane["tokens"], Value::Null);
    assert_eq!(out["cohort"]["lanes_with_usage"], 0);
    assert_eq!(out["cohort"]["lanes_peak_over_500k"], Value::Null);
    assert_eq!(
        out["cohort"]["missing"]["lanes_peak_over_500k"],
        "usage.reported"
    );
    assert_eq!(out["cohort"]["peak_context_median"], Value::Null);
    assert_eq!(lane["head_reviewed"], Value::Null);
    for field in [
        "lanes_head_reviewed",
        "flat_rounds",
        "verified_findings_after_handoff",
    ] {
        assert_eq!(out["cohort"][field], Value::Null, "{field}");
        assert_eq!(
            out["cohort"]["missing"][field],
            "check.finished local_review or pr_review"
        );
    }
    assert_eq!(out["cohort"]["flat_rounds"], Value::Null);
}

#[test]
fn the_cohort_filter() {
    let (_dir, _path, ledger) = common::ledger();
    let early = lane(&ledger);
    let late = lane(&ledger);
    let open = lane(&ledger);
    merged(&ledger, early, "2026-11-05T23:59:59.999Z");
    merged(&ledger, late, "2026-11-06T00:00:00.000Z");
    let ids = |v: Value| -> Vec<i64> {
        v["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["id"].as_i64().unwrap())
            .collect()
    };
    let since = |s: &str| ledger.stats(&Cohort::MergedSince(s.into())).unwrap();
    assert_eq!(ids(since("2026-11-06").unwrap()), [late]);
    assert_eq!(
        ids(since("2026-11-05T12:00:00.000Z").unwrap()),
        [early, late]
    );
    let none = since("2027-01-01").unwrap();
    assert_eq!(none["cohort"]["lanes"], 0);
    assert_eq!(none["cohort"]["claim_to_merge_h_median"], Value::Null);
    for bad in [
        "2026-11-6",
        "2026-11-06 10:00",
        "2026-11-06T10:00:00Z",
        "2026-13-01",
    ] {
        assert!(matches!(since(bad), Err(Refusal::InvalidInput(_))), "{bad}");
    }
    let only_open = stats(&ledger, Cohort::Work(vec![open]));
    // A PR still open has a head nobody can check: null, not true.
    assert_eq!(only_open["lanes"][0]["verdict_on_head"], Value::Null);
    assert_eq!(ids(only_open), [open]);
    assert!(matches!(
        ledger.stats(&Cohort::Work(vec![open, 99])).unwrap(),
        Err(Refusal::NotFound { id: 99, .. })
    ));
}
