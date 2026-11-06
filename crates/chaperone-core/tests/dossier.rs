//! `dossier note` and `dossier get` (ADR 0007): a note is on a key, never a
//! lane; a retry replays and a restored or another session's note appends;
//! every refusal writes nothing; the read is capped, cut notes first, and
//! `not_found` for a key the ledger does not know.

mod common;

use chaperone_core::dossier::NoteInput;
use chaperone_core::ledger::{Applied, EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::work::{Brief, ClaimInput, CreateInput, HandoffInput};
use common::{actor, count, read_only};
use serde_json::json;

const NOTES: &str = "SELECT count(*) FROM events WHERE kind = 'dossier.noted'";
const KEY: &str = "o/r#2";

fn lane(ledger: &Ledger, key: &str) -> i64 {
    let a = actor("1");
    let input = CreateInput {
        title: "t".into(),
        source: Some("github".into()),
        source_id: Some(key.into()),
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

fn note(key: &str, text: &str) -> NoteInput {
    NoteInput {
        key: key.into(),
        note: text.into(),
        ..NoteInput::default()
    }
}

fn refused(ledger: &Ledger, who: &str, input: &NoteInput) -> String {
    match ledger.dossier_note(&actor(who), input).unwrap() {
        Err(Refusal::InvalidInput(m)) => m,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn notes_record_once_and_never_touch_a_lane() {
    let (_dir, path, ledger) = common::ledger();
    let id = lane(&ledger, KEY);
    let before = ledger.work_get(id, common::AT).unwrap().unwrap();
    let lane_events = ledger.work_events(id).unwrap().len();
    let db = read_only(&path);
    let write =
        |who: &str, input: &NoteInput| ledger.dossier_note(&actor(who), input).unwrap().unwrap();

    // Any case of the key; the secret is redacted and a retry still replays.
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123";
    let a = NoteInput {
        links: vec![
            "adr:docs/decisions/0007-issue-dossier.md".into(),
            "issue:core-7".into(),
        ],
        ..note("O/R#2", &format!("read the ADR; {secret}"))
    };
    let first = write("9", &a);
    assert!(!first.replayed);
    assert_eq!(first.payload.redactions, Some(1));
    assert_eq!(first.payload.rest["key"], json!(KEY));
    assert_eq!(
        first.payload.rest["links"],
        json!([
            {"kind": "adr", "ref": "docs/decisions/0007-issue-dossier.md"},
            {"kind": "issue", "ref": "CORE-7"}
        ])
    );
    let again = write("9", &a);
    assert!(again.replayed);
    assert_eq!(again.event_id, first.event_id);

    // A, B, A again appends; another session's identical note appends.
    let b = write("9", &note(KEY, "b"));
    assert!(!write("9", &a).replayed);
    assert!(!write("8", &note(KEY, "b")).replayed);
    assert_eq!(count(&db, NOTES), 4);

    // No lane moved.
    let after = ledger.work_get(id, common::AT).unwrap().unwrap();
    assert_eq!(after.updated_at, before.updated_at);
    assert_eq!(ledger.work_events(id).unwrap().len(), lane_events);

    // Refusals write nothing.
    for (input, says) in [
        (note("#2", "x"), "key must be"),
        (note(KEY, " \n "), "non-blank"),
        (note(KEY, &"x".repeat(601)), "at most 600"),
        (
            NoteInput {
                links: vec!["issue:CORE-1".into(); 6],
                ..note(KEY, "x")
            },
            "at most 5",
        ),
        (
            NoteInput {
                links: vec!["doc:../secrets".into()],
                ..note(KEY, "x")
            },
            "--link is",
        ),
        (
            NoteInput {
                links: vec!["blog:x".into()],
                ..note(KEY, "x")
            },
            "--link is",
        ),
        (
            NoteInput {
                supersedes: Some(b.event_id),
                ..note("o/r#3", "x")
            },
            "is not a note on o/r#3",
        ),
        (
            NoteInput {
                supersedes: Some(1),
                ..note(KEY, "x")
            },
            "is not a note on",
        ),
    ] {
        let m = refused(&ledger, "9", &input);
        assert!(m.contains(says), "{m}");
    }
    // The write itself refuses a note on a lane and one without a key.
    let event = |lane_id, rest: serde_json::Value| Mutation::Event {
        kind: EventKind::DossierNoted,
        lane_id,
        spec_id: None,
        payload: Payload {
            rest: serde_json::from_value(rest).unwrap(),
            ..Payload::default()
        },
    };
    for m in [
        event(Some(id), json!({"key": KEY, "note": "x"})),
        event(None, json!({"note": "x"})),
    ] {
        let applied = ledger.apply(&actor("9"), None, common::AT, m).unwrap();
        assert!(matches!(
            applied,
            Applied::Refused(Refusal::InvalidInput(_))
        ));
    }
    assert_eq!(count(&db, NOTES), 4);

    // A superseded note leaves the read; the log keeps it.
    let fix = write(
        "9",
        &NoteInput {
            supersedes: Some(b.event_id),
            ..note(KEY, "b, corrected")
        },
    );
    let d = ledger.dossier_get(KEY).unwrap().unwrap();
    let ids: Vec<i64> = d.notes.items.iter().map(|n| n.id).collect();
    assert!(!ids.contains(&b.event_id), "{ids:?}");
    assert_eq!(ids[0], fix.event_id);
    assert_eq!(count(&db, NOTES), 5);

    drop(db);
    drop(ledger);
    common::assert_consistent(&path);
}

#[test]
fn get_is_capped_and_cuts_notes_first() {
    let (_dir, _path, ledger) = common::ledger();
    let a = actor("1");
    assert!(matches!(
        ledger.dossier_get(KEY).unwrap(),
        Err(Refusal::NotFoundRef { .. })
    ));
    assert!(matches!(
        ledger.dossier_get("#2").unwrap(),
        Err(Refusal::InvalidInput(_))
    ));
    let body = "# P\n\n## slice 1: the slice  (O/R#2)\n\ndone when: x\n\n## decisions\n\n\
                - [slice 1] Q: a? A: b.\n- [slice 2] Q: c? A: d.\n- Q: e? A: f.\n";
    ledger
        .spec_create(&a, "o/r#1", None, body, common::AT, false)
        .unwrap()
        .unwrap();
    let slice = ledger.dossier_get("o/r#2").unwrap().unwrap().slice;
    let in_spec = slice.in_spec.unwrap();
    assert_eq!(in_spec.decisions, ["[slice 1] Q: a? A: b.", "Q: e? A: f."]);
    assert!(slice.parent_of.is_none());
    assert_eq!(
        ledger
            .dossier_get("o/r#1")
            .unwrap()
            .unwrap()
            .slice
            .parent_of
            .unwrap()
            .slices,
        1
    );

    // 11 lanes: the newest has 6 handoffs, 150 long paths and 7 decisions.
    let lanes: Vec<i64> = (0..11).map(|_| lane(&ledger, KEY)).collect();
    for (i, &id) in lanes.iter().enumerate() {
        let newest = i == lanes.len() - 1;
        for n in 0..if newest { 6 } else { 1 } {
            let input = HandoffInput {
                summary: format!("{n} {}", "s".repeat(400)),
                changed_files: if newest {
                    (0..150)
                        .map(|f| format!("{f}/{}", "p".repeat(320)))
                        .collect()
                } else {
                    vec![format!("lane{i}.rs")]
                },
                brief: newest.then(|| Brief {
                    decisions: (0..7).map(|d| format!("decision {d}")).collect(),
                    ..Brief::default()
                }),
                ..HandoffInput::default()
            };
            ledger
                .work_handoff(&a, id, &input, None, false)
                .unwrap()
                .unwrap();
        }
    }
    for n in 0..21 {
        ledger
            .dossier_note(
                &actor("9"),
                &note(KEY, &format!("{n:03} {}", "n".repeat(590))),
            )
            .unwrap()
            .unwrap();
    }

    let d = ledger.dossier_get(KEY).unwrap().unwrap();
    // The printed envelope, as the CLI writes it, with the 200 bytes the
    // ledger keeps for its meta.
    let printed = serde_json::to_string_pretty(&json!({"ok": true, "data": d})).unwrap();
    assert!(printed.len() + 200 <= 48_000, "{}", printed.len());
    assert_eq!(d.lanes.items.len(), 10);
    assert_eq!(d.lanes.omitted["lanes"], 1);
    assert_eq!(d.lanes.items[0].id, lanes[10]);
    assert_eq!(d.handoffs.items.len(), 5 + 9);
    assert_eq!(d.handoffs.omitted["handoffs"], 1);
    assert_eq!(d.handoffs.items[0].summary.chars().count(), 300);
    assert_eq!(d.handoffs.decisions.len(), 5);
    assert_eq!(d.handoffs.omitted["decisions"], 2);
    // Every path cut to 300; the cap and the budget together leave out the rest.
    let files = d.files.items.len() + d.files.omitted["files"];
    assert_eq!(files, 150 + 9);
    assert!(d.files.items.iter().all(|f| f.path.chars().count() <= 300));
    let notes = d.notes.items.len() + d.notes.omitted["notes"];
    assert_eq!(notes, 21);
    // The budget cut every note before it cut a file past the cap.
    assert!(d.notes.items.is_empty());
    assert!(d.files.omitted["files"] > 150 + 9 - 100);
}
