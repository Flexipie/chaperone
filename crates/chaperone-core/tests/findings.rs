//! Finding moves (events.md, finding statuses): each allowed move once,
//! each refused one for its own reason with nothing written, the read of
//! what still blocks, and the tables a rebuild folds from the log.

mod common;

use chaperone_core::checks::{FindingMove, Recorded};
use chaperone_core::ledger::{EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::pipeline::Fact;
use chaperone_core::work::{ClaimInput, CreateInput};
use common::{actor, count, read_only};
use serde_json::json;

const ALLOWED: &[(&str, &str)] = &[
    ("open", "fixed"),
    ("open", "answered"),
    ("verified", "fixed"),
    ("open", "verified"),
    ("open", "refuted"),
    ("disputed", "verified"),
    ("disputed", "refuted"),
    ("open", "waived"),
    ("verified", "waived"),
    ("disputed", "waived"),
    ("open", "disputed"),
    ("verified", "disputed"),
];
const STATUSES: [&str; 7] = [
    "open", "fixed", "answered", "verified", "refuted", "disputed", "waived",
];

/// Lane 1 claimed by session s1, with one review raising `keys`
/// (`severity:key`). Returns the lane and the review's event id.
fn review(ledger: &Ledger, keys: &[&str]) -> (i64, i64) {
    let input = CreateInput {
        title: "t".into(),
        source: Some("manual".into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let lane = ledger.work_create(&actor("1"), &input, None, false);
    let lane = lane.unwrap().unwrap().id;
    let claim = ledger.work_claim(&actor("1"), lane, &ClaimInput::default(), None, false);
    claim.unwrap().unwrap();
    let findings: Vec<_> = keys.iter().map(|k| k.split_once(':').unwrap()).collect();
    let findings = findings.iter().map(|(severity, key)| {
        json!({"finding_key": key, "severity": severity, "title": "t", "body": "b"})
    });
    let payload = json!({"check": "local_review", "outcome": "ran", "head_sha": "abc1234",
                         "findings": findings.collect::<Vec<_>>()});
    let payload: Payload = serde_json::from_value(payload).unwrap();
    let m = Mutation::Event {
        kind: EventKind::CheckFinished,
        lane_id: Some(lane),
        spec_id: None,
        payload,
    };
    let at = "2026-11-03T00:00:00.000Z";
    let applied = ledger.apply(&actor("9"), None, at, m).unwrap();
    (lane, common::committed(applied).id)
}

/// One move by session `s<who>`; an empty reason or sha is none.
fn mv(
    l: &Ledger,
    check: i64,
    (who, key, to): (&str, &str, &str),
    reason: &str,
    sha: &str,
) -> Result<Recorded, Refusal> {
    let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let input = FindingMove {
        check_event_id: check,
        finding_key: key.into(),
        status: to.into(),
        reason: some(reason),
        head_sha: some(sha),
    };
    l.finding_update(&actor(who), &input, false).unwrap()
}

/// Walks `key` from open along `path`, each move by the session it allows:
/// the lane's (s1) disputes, one that never held it (s2) does the rest.
fn walk(l: &Ledger, check: i64, key: &str, path: &[&str]) {
    for to in path.iter().filter(|s| **s != "open") {
        let who = if *to == "disputed" { "1" } else { "2" };
        let moved = mv(l, check, (who, key, to), "r", "abc1234");
        assert!(moved.is_ok(), "{key} to {to}: {moved:?}");
    }
}

#[test]
fn moves_follow_the_table() {
    let (_dir, path, ledger) = common::ledger();
    // Questions, so every move is open to them; b0 is a blocker.
    let keys: Vec<_> = (0..40).map(|i| format!("question:f{i}")).collect();
    let keys: Vec<_> = keys
        .iter()
        .map(String::as_str)
        .chain(["blocker:b0"])
        .collect();
    let (_, check) = review(&ledger, &keys);
    let db = read_only(&path);
    let events = || count(&db, "SELECT count(*) FROM events");
    let status = |key: &str| -> String {
        let sql = "SELECT status FROM findings WHERE finding_key = ?1";
        db.query_row(sql, [key], |r| r.get(0)).unwrap()
    };
    for (i, (from, to)) in ALLOWED.iter().enumerate() {
        let key = format!("f{i}");
        walk(&ledger, check, &key, &[from, to]);
        assert_eq!(status(&key), *to);
    }
    // Every pair the table lacks, with all a move could need.
    for (i, from) in STATUSES.iter().enumerate() {
        let key = format!("f{}", 20 + i);
        walk(&ledger, check, &key, &[from]);
        for to in STATUSES[1..]
            .iter()
            .filter(|to| !ALLOWED.contains(&(from, to)))
        {
            let before = events();
            let got = mv(&ledger, check, ("2", &key, to), "r", "abc1234");
            let want = match from == to {
                true => format!("finding {check}/{key} is already {to}"),
                false => format!("a finding cannot move from {from} to {to}"),
            };
            assert_eq!(got.unwrap_err(), Refusal::InvalidInput(want));
            assert_eq!((events(), status(&key)), (before, from.to_string()));
        }
    }
    // The lane never settles its own blocker; the wrong session, a bad
    // field, a repeat, an unknown status or finding. Columns: session,
    // finding, status, reason and sha (`-` none, `_` blank, `N` two lines,
    // `L` 501 characters), and the refusal.
    walk(&ledger, check, "f30", &["disputed"]);
    let cases = "\
        1 b0 fixed - abc1234 session s1 has held lane 1; a finding goes fixed
        1 b0 answered - - only a question is answered; finding 3/b0 is a blocker
        1 f31 verified - - session s1 has held lane 1; a finding goes verified
        1 f31 waived r - session s1 has held lane 1; a finding goes waived
        1 f30 refuted - - session s1 has held lane 1; a finding goes refuted
        2 f31 disputed r - only the lane disputes a finding
        1 f31 disputed - - a finding goes disputed only with a reason
        2 f31 waived _ - reason is one non-blank line of at most 500 characters
        2 f31 waived N - reason is one non-blank line
        2 f31 waived L - reason is one non-blank line
        2 f31 fixed - - a fixed finding needs the head_sha
        2 f31 fixed - ABC1234 a fixed finding needs the head_sha
        1 f30 disputed r - finding 3/f30 is already disputed
        2 f31 bogus - - finding.updated status is one of
        2 nope fixed - abc1234 NotFoundRef { entity: \"finding\", reference: \"3/nope\" }";
    let long = "x".repeat(501);
    for case in cases.lines() {
        let f: Vec<_> = case.trim().splitn(6, ' ').collect();
        let arg = |s| match s {
            "-" => "",
            "_" => " ",
            "N" => "a\nb",
            "L" => long.as_str(),
            s => s,
        };
        let before = events();
        let got = mv(&ledger, check, (f[0], f[1], f[2]), arg(f[3]), arg(f[4]));
        let got = format!("{:?}", got.unwrap_err());
        assert!(got.contains(f[5]), "{case}: {got}");
        assert_eq!(events(), before, "{case}");
    }
    assert_eq!(
        [status("f30"), status("f31"), status("b0")],
        ["disputed", "open", "open"]
    );
    // A secret in a reason is redacted before the write stores it.
    let reason = format!("see ghp_{}", "a".repeat(36));
    let p = mv(&ledger, check, ("1", "f31", "disputed"), &reason, "");
    let p = p.unwrap().payload;
    assert_eq!(p.rest["reason"], "see [redacted:github_token]");
    assert_eq!(p.redactions, Some(1));
    common::assert_consistent(&path);
}

/// Unsettled findings block unless their severity is a non-blocking one,
/// an unknown severity included; a dispute keeps the round open until it
/// is ruled on.
#[test]
fn blocking_read_and_round() {
    let (_dir, path, ledger) = common::ledger();
    let (lane, check) = review(
        &ledger,
        &[
            "blocker:a",
            "risk:b",
            "question:c",
            "blocker:d",
            "convention:e",
            "Blocker:f",
        ],
    );
    let closed = || {
        ledger
            .project(lane, None)
            .unwrap()
            .get("review.previous_round_closed")
            == &Fact::Bool(true)
    };
    walk(&ledger, check, "a", &["verified"]);
    walk(&ledger, check, "b", &["disputed"]);
    walk(&ledger, check, "d", &["waived"]);
    let blocking = ledger.blocking_findings(lane).unwrap();
    let got: Vec<_> = blocking
        .iter()
        .map(|f| (f.finding_key.as_str(), f.status.as_str()))
        .collect();
    assert_eq!(got, [("a", "verified"), ("b", "disputed"), ("f", "open")]);
    walk(&ledger, check, "e", &["waived"]);
    walk(&ledger, check, "f", &["waived"]);
    walk(&ledger, check, "a", &["fixed"]);
    walk(&ledger, check, "c", &["answered"]);
    assert!(!closed());
    walk(&ledger, check, "b", &["refuted"]);
    assert!(closed());
    assert!(ledger.blocking_findings(lane).unwrap().is_empty());
    common::assert_consistent(&path);
}

/// A move carrying the harness's producer (`check run review`'s verifier)
/// is made by the binary, so the lane's own session may write it; it
/// makes the verifier's moves only. The same move without it is refused.
#[test]
fn the_harness_rules_from_the_lanes_session() {
    let (_dir, path, ledger) = common::ledger();
    let (_, check) = review(&ledger, &["blocker:a", "blocker:b", "blocker:c"]);
    walk(&ledger, check, "c", &["disputed"]);
    let rule = |key: &str, to: &str| {
        let payload = json!({"check_event_id": check, "finding_key": key, "status": to,
            "reason": "r", "producer": {"kind": "harness", "bin": "chaperone", "version": "0"}});
        let m = Mutation::Event {
            kind: EventKind::FindingUpdated,
            lane_id: Some(1),
            spec_id: None,
            payload: serde_json::from_value(payload).unwrap(),
        };
        ledger.apply(&actor("1"), None, "2099-01-01T00:00:00.000Z", m)
    };
    assert!(mv(&ledger, check, ("1", "a", "verified"), "", "").is_err());
    common::committed(rule("a", "verified").unwrap());
    common::committed(rule("c", "refuted").unwrap());
    let waived = format!("{:?}", rule("b", "waived").unwrap());
    assert!(
        waived.contains("session s1 has held lane 1; a finding goes waived"),
        "{waived}"
    );
    let blocking = ledger.blocking_findings(1).unwrap();
    let got: Vec<_> = blocking
        .iter()
        .map(|f| (&*f.finding_key, &*f.status))
        .collect();
    assert_eq!(got, [("a", "verified"), ("b", "open")]);
    common::assert_consistent(&path);
}
