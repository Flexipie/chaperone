//! The lane page's data (#111): `lane_page` (timeline, decisions with who
//! made them, review rounds, gates required vs passed), the per-repo
//! median (`repo_median`), and `git::lane_files` on a temp repo.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::checks::{AttentionInput, CheckInput, MergedInput};
use chaperone_core::git::{git_bin, lane_files};
use chaperone_core::ledger::Ledger;
use chaperone_core::pipeline::Phase;
use chaperone_core::work::{Brief, ClaimInput, CreateInput, HandoffInput, UpdateInput};
use common::actor;
use serde_json::{Value, json};

const OLD: &str = "2026-01-01T00:00:00.000Z";
const PRE: &str = "2025-12-31T00:00:00.000Z";

/// A claimed lane on `repo` holding PR `<repo>#<id>`, created at `created`
/// and claimed at `at` (None: now).
fn lane(ledger: &Ledger, repo: &str, created: Option<&str>, at: Option<&str>) -> i64 {
    let a = actor("1");
    let input = CreateInput {
        title: "t".into(),
        repo: Some(repo.into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let id = ledger
        .work_create(&a, &input, created, false)
        .unwrap()
        .unwrap()
        .id;
    let claim = ClaimInput {
        branch: Some(format!("b{id}")),
        ..ClaimInput::default()
    };
    let item = ledger
        .work_claim(&a, id, &claim, at, false)
        .unwrap()
        .unwrap();
    let update = UpdateInput {
        pr_ref: Some(format!("{repo}#{id}")),
        ..UpdateInput::default()
    };
    ledger
        .work_update(&a, id, &update, Some(&item.updated_at), None, false)
        .unwrap()
        .unwrap();
    id
}

fn check(ledger: &Ledger, work: i64, check: &str) {
    let input = CheckInput {
        work,
        check: check.into(),
        head: "abc1234".into(),
        outcome: "ran".into(),
        findings: (check == "plan_check").then_some(0),
        ..CheckInput::default()
    };
    ledger
        .check_record(&actor("check"), &input)
        .unwrap()
        .unwrap();
}

fn attention(
    ledger: &Ledger,
    work: i64,
    kind: &str,
    by: &str,
    answers: Option<i64>,
    note: &str,
) -> i64 {
    let input = AttentionInput {
        work,
        kind: kind.into(),
        by: by.into(),
        reason: (kind == "asked").then(|| "question.scope".into()),
        answers,
        note: Some(note.into()),
        ..AttentionInput::default()
    };
    ledger
        .attention_record(&actor("orch"), &input)
        .unwrap()
        .unwrap()
        .event_id
}

#[test]
fn lane_page_reads_timeline_decisions_rounds_and_gates() {
    let (_dir, _path, ledger) = common::ledger();
    let body = "# P\n\n## slice 1: the slice  (O/R#2)\n\ndone when: x\n\n## decisions\n\n\
                - [slice 1] Q: a? A: b. (agreed: in chat, 2026-11-08)\n- [slice 2] Q: c? A: d.\n";
    ledger
        .spec_create(&actor("1"), "o/r#1", None, body, common::AT, false)
        .unwrap()
        .unwrap();
    // Claimed in January, its lease renewed now: minutes count from the
    // first claim, not from the renewal.
    // Created the day before it is claimed: that row has no minutes.
    let id = lane(&ledger, "o/r", Some(PRE), Some(OLD));
    ledger
        .work_claim(&actor("1"), id, &ClaimInput::default(), None, false)
        .unwrap()
        .unwrap();
    check(&ledger, id, "plan_check");
    check(&ledger, id, "review_round");
    let asked = attention(&ledger, id, "asked", "owner", None, "which way?");
    attention(
        &ledger,
        id,
        "answered",
        "owner",
        Some(asked),
        "the left one",
    );
    let handoff = HandoffInput {
        summary: "built".into(),
        commit: Some("abc1234".into()),
        brief: Some(Brief {
            decisions: vec!["files are their own call".into()],
            ..Brief::default()
        }),
        ..HandoffInput::default()
    };
    ledger
        .work_handoff(&actor("1"), id, &handoff, None, false)
        .unwrap()
        .unwrap();

    let page = ledger.lane_page(id, Some("o/r#2")).unwrap();
    let kinds: Vec<(&str, Option<&str>)> = page
        .timeline
        .iter()
        .map(|t| (t.kind.as_str(), t.what.as_deref()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("lane.handed_off", None),
            ("attention.recorded", Some("answered by owner")),
            ("attention.recorded", Some("asked to owner")),
            ("check.finished", Some("review_round ran")),
            ("check.finished", Some("plan_check ran")),
            ("pr.opened", Some("o/r#1")),
            ("lane.claimed", None),
            ("lane.created", None),
        ]
    );
    for t in &page.timeline {
        let before_claim = t.kind == "lane.created";
        assert!(
            t.minutes.is_none() == before_claim && t.minutes.is_none_or(|m| m >= 0),
            "{t:?}"
        );
    }
    let claimed = page.timeline.iter().find(|t| t.kind == "lane.claimed");
    assert_eq!(claimed.unwrap().minutes, Some(0));

    let who: Vec<(Option<&str>, &str)> = page
        .decisions
        .iter()
        .map(|d| (d.by.as_deref(), d.text.as_str()))
        .collect();
    assert_eq!(
        who,
        [
            (None, "[slice 1] Q: a? A: b. (agreed: in chat, 2026-11-08)"),
            (Some("a1"), "files are their own call"),
            (Some("owner"), "the left one"),
        ]
    );

    assert_eq!(page.review_rounds, 1);
    let gates = page.gates.unwrap();
    assert_eq!(gates.to, Phase::Ready);
    assert_eq!(gates.head.as_deref(), Some("abc1234"));
    let passed: Vec<(&str, bool)> = gates
        .gates
        .iter()
        .map(|g| (g.gate.as_str(), g.passed))
        .collect();
    assert_eq!(
        passed,
        [
            ("stamp_ok", true),
            ("plan_check_recorded", true),
            ("quality_gate_recorded", false),
            ("verdict_is_yes", false),
            ("verdict_on_head", false),
            ("smoke_ok", false),
            ("kept_tests_ok", false),
            ("rounds_at_most", true),
        ]
    );

    // An unknown lane: no timeline, no gates (never "0 of 0 passed").
    let none = ledger.lane_page(99, None).unwrap();
    assert!(none.timeline.is_empty() && none.gates.is_none());
}

/// Daisy's view of local review `job` on `head`, for lane `id`'s branch.
fn review(ledger: &Ledger, id: i64, job: i64, head: &str, status: &str, score: f64) {
    let view = json!({
        "job_id": job, "source": "local", "status": status, "phase": "done", "done": true,
        "repo": "o/r", "branch": format!("b{id}"), "head_sha": head, "score": score,
        "verdict": "safe", "summary": "s", "findings": [],
    });
    ledger
        .review_record(&actor("daisy"), id, job, &view)
        .unwrap()
        .unwrap();
}

/// Reviews newest job first, a late-recorded older job after it; the head
/// is the engine's (the newest job's), never the newest record's.
#[test]
fn lane_page_reviews_by_job_with_the_engine_head() {
    let (_dir, _path, ledger) = common::ledger();
    let id = lane(&ledger, "o/r", None, None);
    review(&ledger, id, 8, "bbb2222", "reviewed", 8.0);
    review(&ledger, id, 7, "aaa1111", "reviewed", 6.5);
    review(&ledger, id, 9, "ccc3333", "failed", 0.0);
    let page = ledger.lane_page(id, None).unwrap();
    let got: Vec<(&str, Option<&str>, Option<f64>)> = page
        .reviews
        .iter()
        .map(|r| (r.outcome.as_str(), r.head.as_deref(), r.score))
        .collect();
    assert_eq!(
        got,
        [
            ("skipped", Some("ccc3333"), None),
            ("ran", Some("bbb2222"), Some(8.0)),
            ("ran", Some("aaa1111"), Some(6.5)),
        ]
    );
    assert_eq!(page.review_head.as_deref(), Some("bbb2222"));
    assert_eq!(page.review_rounds, 2);
}

fn merged(ledger: &Ledger, work: i64, repo: &str, merged_at: &str) {
    let input = MergedInput {
        work,
        pr: format!("{repo}#{work}"),
        merge_sha: "fedcba9".into(),
        head: "abc1234def".into(),
        merged_at: merged_at.into(),
        patch_id: None,
    };
    ledger.work_merged(&actor("9"), &input).unwrap().unwrap();
}

/// Per repo, over its lanes merged in the last 30 days only: another
/// repo's lanes and older merges are not in it, and the slug's case is not.
#[test]
fn repo_median_is_per_repo_over_30_days() {
    let (_dir, _path, ledger) = common::ledger();
    let now = ledger.now().unwrap();
    let fast = lane(&ledger, "O/R", None, None);
    merged(&ledger, fast, "O/R", &now);
    // Claimed in January and merged now: hundreds of hours, another repo.
    let slow = lane(&ledger, "x/y", Some(OLD), Some(OLD));
    merged(&ledger, slow, "x/y", &now);
    // Merged before the window.
    let old = lane(&ledger, "o/r", Some(OLD), Some(OLD));
    merged(&ledger, old, "o/r", "2026-01-02T00:00:00.000Z");
    lane(&ledger, "z/w", None, None);

    let median = |repo: &str| ledger.repo_median(repo).unwrap();
    assert_eq!(
        median("o/r"),
        json!({"claim_to_merge_h_median": 0.0, "lanes": 1, "too_few_lanes": true})
    );
    assert!(
        median("x/y")["claim_to_merge_h_median"]
            .as_f64()
            .is_some_and(|h| h > 24.0)
    );
    assert_eq!(median("Z/W")["claim_to_merge_h_median"], Value::Null);
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// Committed, uncommitted and untracked files against the merge base,
/// each with its area and +/-; a binary file has no counts.
#[test]
fn lane_files_against_the_merge_base() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path();
    git(wt, &["init", "-q", "-b", "main"]);
    std::fs::write(wt.join("a.rs"), "1\n2\n3\n").unwrap();
    std::fs::write(wt.join("gone.md"), "x\n").unwrap();
    git(wt, &["add", "."]);
    git(wt, &["commit", "-qm", "base"]);
    let base = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(wt)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::fs::write(wt.join("a.rs"), "1\nnew\n3\n4\n").unwrap();
    git(wt, &["rm", "-q", "gone.md"]);
    git(wt, &["add", "."]);
    git(wt, &["commit", "-qm", "lane"]);
    std::fs::write(wt.join("a.rs"), "1\nnew\n3\n4\n5\n").unwrap();
    std::fs::write(wt.join("new.rs"), "a\nb").unwrap();
    std::fs::write(wt.join("blob.bin"), [0u8, 1, 2]).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let area = |p: &str| p.ends_with(".rs").then(|| "core".to_string());
    let (got_base, files) = lane_files(
        &git_bin(deadline),
        wt,
        Some(base.trim()),
        deadline,
        1 << 20,
        area,
    )
    .unwrap();
    assert_eq!(got_base, base.trim());
    let rows: Vec<String> = files
        .iter()
        .map(|f| {
            let n = |c: Option<u64>| c.map_or("-".into(), |c| c.to_string());
            let area = f.area.as_deref().unwrap_or("none");
            let tracked = if f.untracked { "untracked" } else { "tracked" };
            format!(
                "{} {area} +{} -{} {tracked}",
                f.path,
                n(f.added),
                n(f.removed)
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            "a.rs core +3 -1 tracked",
            "gone.md none +0 -1 tracked",
            "blob.bin none +- -- untracked",
            "new.rs core +2 -0 untracked",
        ]
    );
}
