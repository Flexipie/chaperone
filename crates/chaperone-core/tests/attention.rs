//! `attention record`: one `attention.recorded` per intervention, a retry
//! replays, a real repeat with its own key appends, notes are redacted at
//! write, and every refusal writes nothing.

mod common;

use chaperone_core::checks::AttentionInput;
use chaperone_core::ledger::{Applied, EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::work::{ClaimInput, CreateInput};
use common::{actor, count, read_only};

const ATTENTION: &str = "SELECT count(*) FROM events WHERE kind = 'attention.recorded'";

fn lane(ledger: &Ledger) -> i64 {
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
    ledger
        .work_claim(&a, id, &ClaimInput::default(), None, false)
        .unwrap()
        .unwrap();
    id
}

fn record(work: i64, kind: &str) -> AttentionInput {
    AttentionInput {
        work,
        kind: kind.into(),
        by: "orchestrator".into(),
        ..AttentionInput::default()
    }
}

#[test]
fn each_intervention_records_once() {
    let (_dir, path, ledger) = common::ledger();
    let o = actor("9");
    let id = lane(&ledger);
    let before = ledger.work_get(id, common::AT).unwrap().unwrap().updated_at;
    let lane_events = ledger.work_events(id).unwrap().len();
    let asked = ledger
        .attention_record(
            &o,
            &AttentionInput {
                by: "owner".into(),
                reason: Some("question.size".into()),
                ..record(id, "asked")
            },
        )
        .unwrap()
        .unwrap();
    let token = format!("ghp_{}", "a".repeat(36));
    let answer = AttentionInput {
        by: "owner".into(),
        answers: Some(asked.event_id),
        note: Some(format!("split it in three, key {token}")),
        ..record(id, "answered")
    };
    ledger.attention_record(&o, &answer).unwrap().unwrap();
    // A retry with nothing new on the lane replays.
    assert!(
        ledger
            .attention_record(&o, &answer)
            .unwrap()
            .unwrap()
            .replayed
    );
    for input in [
        AttentionInput {
            round: Some(2),
            ..record(id, "sent_back")
        },
        record(id, "nudged"),
        record(id, "revived"),
    ] {
        ledger.attention_record(&o, &input).unwrap().unwrap();
    }
    // A second nudge on a stalled lane is a real repeat: its own key appends.
    let again = AttentionInput {
        idempotency_key: Some("nudge-2".into()),
        ..record(id, "nudged")
    };
    assert!(
        !ledger
            .attention_record(&o, &again)
            .unwrap()
            .unwrap()
            .replayed
    );
    let db = read_only(&path);
    assert_eq!(count(&db, ATTENTION), 6);
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM events WHERE payload_json LIKE '%ghp_a%'"
        ),
        0
    );
    // Not lane work: the CAS stamp and work events are untouched.
    let after = ledger.work_get(id, common::AT).unwrap().unwrap().updated_at;
    assert_eq!(after, before);
    assert_eq!(ledger.work_events(id).unwrap().len(), lane_events);
    common::assert_consistent(&path);
}

#[test]
fn refusals_write_nothing() {
    let (_dir, path, ledger) = common::ledger();
    let o = actor("9");
    let id = lane(&ledger);
    let other = lane(&ledger);
    let refused = |input: AttentionInput, says: &str| {
        let r = ledger.attention_record(&o, &input).unwrap();
        assert!(
            matches!(&r, Err(Refusal::InvalidInput(m)) if m.contains(says)),
            "{says}: {r:?}"
        );
    };
    refused(record(id, "pinged"), "--type");
    refused(
        AttentionInput {
            by: "agent".into(),
            ..record(id, "nudged")
        },
        "--by",
    );
    refused(record(id, "asked"), "asked needs --reason");
    refused(
        AttentionInput {
            reason: Some("question.scope".into()),
            ..record(id, "nudged")
        },
        "--reason goes with asked",
    );
    refused(record(id, "sent_back"), "--round");
    refused(
        AttentionInput {
            round: Some(1),
            ..record(id, "nudged")
        },
        "--round",
    );
    refused(
        AttentionInput {
            round: Some(0),
            ..record(id, "nudged")
        },
        "--round",
    );
    refused(record(id, "answered"), "answered needs");
    refused(
        AttentionInput {
            answers: Some(1),
            ..record(id, "nudged")
        },
        "answered only",
    );
    refused(
        AttentionInput {
            note: Some("two\nlines".into()),
            ..record(id, "nudged")
        },
        "one line",
    );
    // An answer must point at a question asked on the same lane.
    let asked = ledger
        .attention_record(
            &o,
            &AttentionInput {
                reason: Some("question.scope".into()),
                ..record(other, "asked")
            },
        )
        .unwrap()
        .unwrap();
    refused(
        AttentionInput {
            answers: Some(asked.event_id),
            note: Some("cut it".into()),
            ..record(id, "answered")
        },
        "not a question asked",
    );
    assert!(matches!(
        ledger.attention_record(&o, &record(99, "nudged")).unwrap(),
        Err(Refusal::NotFound { .. })
    ));
    // The write itself refuses a record without its type and handler.
    let bare = Mutation::Event {
        kind: EventKind::AttentionRecorded,
        lane_id: Some(id),
        spec_id: None,
        payload: Payload::default(),
    };
    assert!(matches!(
        ledger.apply(&o, None, common::AT, bare).unwrap(),
        Applied::Refused(Refusal::InvalidInput(m)) if m.contains("attention.recorded needs")
    ));
    assert_eq!(count(&read_only(&path), ATTENTION), 1);
}
