//! `work merged`: one `pr.merged` per PR the lane has held, a retry
//! replays, and a PR the lane never held, or one merged on another lane,
//! is refused with nothing written.

mod common;

use chaperone_core::checks::MergedInput;
use chaperone_core::ledger::{Applied, EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::work::{ClaimInput, CreateInput, UpdateInput};
use common::{actor, count, read_only};

const MERGED: &str = "SELECT count(*) FROM events WHERE kind = 'pr.merged'";

/// A claimed lane that has held each PR in `prs`, in turn.
fn lane(ledger: &Ledger, prs: &[&str]) -> i64 {
    let a = actor("1");
    let input = CreateInput {
        title: "t".into(),
        source: Some("manual".into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let id = ledger
        .work_create(&a, &input, None, false)
        .unwrap()
        .unwrap()
        .id;
    let mut item = ledger
        .work_claim(&a, id, &ClaimInput::default(), None, false)
        .unwrap()
        .unwrap();
    for pr in prs {
        let update = UpdateInput {
            pr_ref: Some(pr.to_string()),
            ..UpdateInput::default()
        };
        item = ledger
            .work_update(&a, id, &update, Some(&item.updated_at), None, false)
            .unwrap()
            .unwrap();
    }
    id
}

fn merged(work: i64, pr: &str) -> MergedInput {
    MergedInput {
        work,
        pr: pr.into(),
        merge_sha: "abcdef1".into(),
        head: "1234567".into(),
        merged_at: "2026-11-05T10:00:00.000Z".into(),
        patch_id: None,
    }
}

#[test]
fn each_held_pr_merges_once() {
    let (_dir, path, ledger) = common::ledger();
    let id = lane(&ledger, &["o/r#1", "o/r#2"]);
    let before = ledger.work_get(id, common::AT).unwrap().unwrap().updated_at;
    let lane_events = ledger.work_events(id).unwrap().len();
    // The first PR merges after the second was set: still the lane's.
    let first = ledger
        .work_merged(&actor("9"), &merged(id, "O/R#1"))
        .unwrap()
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(first.payload.rest["merged_at"], "2026-11-05T10:00:00.000Z");
    assert_eq!(first.payload.head_sha.as_deref(), Some("1234567"));
    let again = ledger
        .work_merged(&actor("9"), &merged(id, "o/r#1"))
        .unwrap()
        .unwrap();
    assert!(again.replayed);
    ledger
        .work_merged(&actor("9"), &merged(id, "o/r#2"))
        .unwrap()
        .unwrap();
    assert_eq!(count(&read_only(&path), MERGED), 2);
    // A merge is not lane work and keeps the lane's CAS stamp.
    let after = ledger.work_get(id, common::AT).unwrap().unwrap().updated_at;
    assert_eq!(after, before);
    assert_eq!(ledger.work_events(id).unwrap().len(), lane_events);
    common::assert_consistent(&path);
}

#[test]
fn refusals_write_nothing() {
    let (_dir, path, ledger) = common::ledger();
    let a = actor("9");
    let id = lane(&ledger, &["o/r#1"]);
    let other = lane(&ledger, &["o/r#1"]);
    let refused = |input: MergedInput, says: &str| {
        let r = ledger.work_merged(&a, &input).unwrap();
        assert!(
            matches!(&r, Err(Refusal::InvalidInput(m)) if m.contains(says)),
            "{says}: {r:?}"
        );
    };
    refused(merged(id, "o/r#7"), "never held");
    refused(merged(id, "not a pr"), "owner/repo#N");
    refused(
        MergedInput {
            head: "XYZ".into(),
            ..merged(id, "o/r#1")
        },
        "--head",
    );
    refused(
        MergedInput {
            merged_at: "2026-11-05 10:00".into(),
            ..merged(id, "o/r#1")
        },
        "--merged-at",
    );
    assert!(matches!(
        ledger.work_merged(&a, &merged(99, "o/r#1")).unwrap(),
        Err(Refusal::NotFound { .. })
    ));
    ledger
        .work_merged(&a, &merged(id, "o/r#1"))
        .unwrap()
        .unwrap();
    refused(merged(other, "o/r#1"), "another work item");
    // The write itself refuses a merge without its fields.
    for head_sha in [None, Some("1234567".to_string())] {
        let bare = Mutation::Event {
            kind: EventKind::PrMerged,
            lane_id: Some(id),
            spec_id: None,
            payload: Payload {
                head_sha,
                ..Payload::default()
            },
        };
        assert!(matches!(
            ledger.apply(&a, None, common::AT, bare).unwrap(),
            Applied::Refused(Refusal::InvalidInput(m)) if m.contains("pr.merged needs")
        ));
    }
    assert_eq!(count(&read_only(&path), MERGED), 1);
}
