//! `pipeline_history`: the pipeline changelog parsed from a fixture
//! dotfiles dir (never the real one), and lanes grouped by version with
//! `stats`' cohort numbers, on a temp ledger.

mod common;

use std::path::PathBuf;

use chaperone_core::checks::MergedInput;
use chaperone_core::ledger::{Ledger, Refusal};
use chaperone_core::stats::{Cohort, Version, changelog, parse_changelog};
use chaperone_core::work::{ClaimInput, CreateInput, HandoffInput, UpdateInput};
use common::actor;
use serde_json::{Value, json};

fn dotfiles() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotfiles")
}

/// A lane on `repo`, claimed at `claimed` when given, holding PR
/// `<repo>#<id>`, with one handoff per summary.
fn lane(ledger: &Ledger, repo: &str, claimed: Option<&str>, summaries: &[&str]) -> i64 {
    let a = actor("1");
    let input = CreateInput {
        title: "t".into(),
        repo: Some(repo.into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let id = ledger
        .work_create(&a, &input, None, false)
        .unwrap()
        .unwrap()
        .id;
    let Some(at) = claimed else { return id };
    let claim = ClaimInput {
        branch: Some(format!("b{id}")),
        ..ClaimInput::default()
    };
    let item = ledger
        .work_claim(&a, id, &claim, Some(at), false)
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
    for summary in summaries {
        let handoff = HandoffInput {
            summary: (*summary).into(),
            next_action: Some("needs you: look".into()),
            ..HandoffInput::default()
        };
        ledger
            .work_handoff(&a, id, &handoff, None, false)
            .unwrap()
            .unwrap();
    }
    id
}

fn merged(ledger: &Ledger, work: i64, repo: &str, merged_at: &str) {
    let input = MergedInput {
        work,
        pr: format!("{repo}#{work}"),
        merge_sha: "fedcba9".into(),
        head: "abc1234".into(),
        merged_at: merged_at.into(),
        patch_id: None,
    };
    ledger.work_merged(&actor("9"), &input).unwrap().unwrap();
}

fn history(ledger: &Ledger, versions: &[Version], repo: Option<&str>) -> Vec<Value> {
    let out = ledger.pipeline_history(versions, repo).unwrap().unwrap();
    out["versions"].as_array().unwrap().clone()
}

#[test]
fn the_fixture_changelog_parses_in_file_order() {
    let versions = changelog(&dotfiles()).unwrap();
    let ids: Vec<(&str, &str)> = versions
        .iter()
        .map(|v| (v.id.as_str(), v.at.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("v1", "2026-10-01T00:00:00.000Z"),
            ("v2", "2026-10-10T12:30:00.000Z"),
            ("v3", "2026-11-01T00:00:00.000Z"),
        ]
    );
    assert_eq!(versions[1].entry, "- second.\n  continued.");
    // `## Next version` ends v3's entry.
    assert_eq!(versions[2].entry, "- third.");
}

#[test]
fn a_bad_or_unordered_heading_fails_with_its_line() {
    for (text, line) in [
        ("## v2 2026-10-01T00:00Z\n## v1 2026-10-02T00:00Z", 2),
        ("## v1 2026-10-02T00:00Z\n## v2 2026-10-01T00:00Z", 2),
        ("## v1 2026-10-01T00:00Z\n## v1 2026-10-02T00:00Z", 2),
        ("x\n## v1 2026-10-01T00:00Z tomorrow", 2),
        ("## v1 2026-10-01", 1),
        ("## v1x 2026-10-01T00:00Z", 1),
    ] {
        let err = parse_changelog(text).unwrap_err();
        assert!(err.contains(&format!(":{line}:")), "{text:?}: {err}");
    }
    let Err(Refusal::InvalidInput(why)) = changelog(&dotfiles().join("missing")) else {
        panic!("a missing changelog is refused");
    };
    assert!(
        why.contains("missing/claude/pipeline-changelog.md"),
        "{why}"
    );
    // The shape passes, the time does not: refused before any lane is read.
    let (_dir, _path, ledger) = common::ledger();
    let versions = parse_changelog("## v1 2026-13-40T25:61Z").unwrap();
    let out = ledger.pipeline_history(&versions, None).unwrap();
    assert!(matches!(out, Err(Refusal::InvalidInput(_))), "{out:?}");
}

#[test]
fn lanes_group_by_their_first_stamp_else_their_claim() {
    let (_dir, _path, ledger) = common::ledger();
    let versions = changelog(&dotfiles()).unwrap();
    let by_claim = lane(&ledger, "o/r", Some("2026-10-11T00:00:00.000Z"), &[]);
    // The first stamp wins over a later one and over the claim (v2's);
    // `pipeline v1.5` names v1 as lane-stats reads it, `pipeline v3x` is no stamp.
    let stamped = lane(
        &ledger,
        "o/r",
        Some("2026-10-11T00:00:00.000Z"),
        &[
            "ran pipeline v3x, xpipeline v2",
            "pipeline v1.5 rules",
            "pipeline v3",
        ],
    );
    let unknown = lane(
        &ledger,
        "O/R",
        Some("2026-11-02T00:00:00.000Z"),
        &["pipeline v9"],
    );
    let early = lane(&ledger, "o/r", Some("2026-09-01T00:00:00.000Z"), &[]);
    let other_repo = lane(&ledger, "o/other", Some("2026-11-02T00:00:00.000Z"), &[]);
    let _never_claimed = lane(&ledger, "o/r", None, &["pipeline v3"]);
    let cancelled = lane(&ledger, "o/r", Some("2026-11-02T00:00:00.000Z"), &[]);
    ledger
        .work_cancel(&actor("1"), cancelled, None, None, None, false)
        .unwrap()
        .unwrap();

    let rows = history(&ledger, &versions, Some("o/R"));
    let got: Vec<(&str, Value, Value)> = rows
        .iter()
        .map(|r| {
            (
                r["version"].as_str().unwrap(),
                r["lane_ids"].clone(),
                r["version_from"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("v3", json!([]), json!({"stamp": 0, "claim": 0})),
            ("v2", json!([by_claim]), json!({"stamp": 0, "claim": 1})),
            ("v1", json!([stamped]), json!({"stamp": 1, "claim": 0})),
            ("v9", json!([unknown]), json!({"stamp": 1, "claim": 0})),
            ("manual", json!([early]), json!({"stamp": 0, "claim": 1})),
        ]
    );
    assert_eq!(rows[0]["entry"], "- third.");
    assert_eq!(rows[0]["started_at"], "2026-11-01T00:00:00.000Z");
    assert_eq!(rows[0]["cohort"]["lanes"], 0);
    assert_eq!(rows[3]["entry"], Value::Null);
    // The engine id stamped at claim sits next to the changelog version.
    let engine = chaperone_core::pipeline::current().id.clone();
    assert_eq!(rows[1]["engine_ids"], json!([engine]));

    let all = history(&ledger, &versions, None);
    let v3: &Value = all.iter().find(|r| r["version"] == "v3").unwrap();
    assert_eq!(v3["lane_ids"], json!([other_repo]));
}

#[test]
fn a_versions_median_is_stats_cohort_median() {
    let (_dir, _path, ledger) = common::ledger();
    let versions = changelog(&dotfiles()).unwrap();
    let a = lane(&ledger, "o/r", Some("2026-10-11T00:00:00.000Z"), &[]);
    let b = lane(
        &ledger,
        "o/r",
        Some("2026-10-12T00:00:00.000Z"),
        &["pipeline v2"],
    );
    let open = lane(&ledger, "o/r", Some("2026-10-13T00:00:00.000Z"), &[]);
    merged(&ledger, a, "o/r", "2026-10-11T10:00:00.000Z");
    merged(&ledger, b, "o/r", "2026-10-12T04:00:00.000Z");

    let rows = history(&ledger, &versions, None);
    let v2 = rows.iter().find(|r| r["version"] == "v2").unwrap();
    assert_eq!(v2["lane_ids"], json!([a, b, open]));
    assert_eq!(v2["cohort"]["claim_to_merge_h_median"], 7.0);
    let stats = ledger
        .stats(&Cohort::Work(vec![a, b, open]))
        .unwrap()
        .unwrap();
    assert_eq!(v2["cohort"], stats["cohort"]);
}
