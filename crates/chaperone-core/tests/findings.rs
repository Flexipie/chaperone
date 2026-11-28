//! Finding moves (events.md, finding statuses): each allowed move once,
//! each refused one for its own reason with nothing written, the read of
//! what still blocks, and the tables a rebuild folds from the log.

mod common;

use chaperone_core::checks::{FindingMove, Recorded};
use chaperone_core::ledger::{Actor, EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::pipeline::Fact;
use chaperone_core::review;
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
    ("open", "accepted"),
    ("accepted", "waived"),
    ("refuted", "verified"),
    ("waived", "verified"),
];
const STATUSES: [&str; 8] = [
    "open", "fixed", "answered", "verified", "refuted", "disputed", "waived", "accepted",
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

/// A harness plan check on lane 1 with `p`'s fields, its event id.
fn plan_check(ledger: &Ledger, mut p: serde_json::Value) -> i64 {
    let harness = json!({"kind": "harness", "bin": "chaperone", "version": "0"});
    p.as_object_mut().unwrap().extend([
        ("check".into(), json!("plan_check")),
        ("head_sha".into(), json!("abc1234")),
        ("producer".into(), harness),
    ]);
    let m = Mutation::Event {
        kind: EventKind::CheckFinished,
        lane_id: Some(1),
        spec_id: None,
        payload: serde_json::from_value(p).unwrap(),
    };
    let at = "2099-01-01T00:00:00.000Z";
    common::committed(ledger.apply(&actor("9"), None, at, m).unwrap()).id
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
        waiver_kind: (to == "waived").then(|| "disproved".into()),
        waiver_ref: (to == "waived").then(|| "t.rs:1".into()),
    };
    l.finding_update(&actor(who), &input, false).unwrap()
}

/// `who` waives `key` as `kind`, with `reference` when it is not blank.
fn waive(
    l: &Ledger,
    check: i64,
    (who, key): (&str, &str),
    (kind, reference): (&str, &str),
) -> Result<Recorded, Refusal> {
    let input = FindingMove {
        check_event_id: check,
        finding_key: key.into(),
        status: "waived".into(),
        reason: Some("r".into()),
        waiver_kind: (!kind.is_empty()).then(|| kind.into()),
        waiver_ref: (!reference.is_empty()).then(|| reference.into()),
        ..FindingMove::default()
    };
    let who = Actor {
        agent: Some("a".into()),
        session: (!who.is_empty()).then(|| format!("s{who}")),
    };
    l.finding_update(&who, &input, false).unwrap()
}

/// Walks `key` from open along `path`, each move by the session it allows:
/// the lane's (s1) disputes and accepts, one that never held it (s2) does
/// the rest.
fn walk(l: &Ledger, check: i64, key: &str, path: &[&str]) {
    for (i, to) in path.iter().enumerate().filter(|s| *s.1 != "open") {
        // Work that moved after the plan is waived as scope, by a fresh session.
        if i > 0 && path[i - 1] == "accepted" {
            let moved = waive(l, check, ("3", key), ("scope", "#243"));
            assert!(moved.is_ok(), "{key} to {to}: {moved:?}");
            continue;
        }
        let who = if ["disputed", "accepted"].contains(to) {
            "1"
        } else {
            "2"
        };
        let moved = mv(l, check, (who, key, to), "r", "abc1234");
        assert!(moved.is_ok(), "{key} to {to}: {moved:?}");
    }
}

#[test]
fn moves_follow_the_table() {
    let (_dir, path, ledger) = common::ledger();
    // Questions, so every move is open to them; b0 is a blocker. The table
    // walks a plan check's, so `accepted` is a move they can make.
    let (_, check) = review(&ledger, &["question:f30", "question:f31", "blocker:b0"]);
    let found: Vec<_> = (0..30)
        .map(|i| json!({"finding_key": format!("f{i}"), "severity": "question", "title": "t", "body": "b"}))
        .collect();
    let plan = plan_check(&ledger, json!({"outcome": "ran", "findings": found}));
    let db = read_only(&path);
    let events = || count(&db, "SELECT count(*) FROM events");
    let status = |key: &str| -> String {
        let sql = "SELECT status FROM findings WHERE finding_key = ?1";
        db.query_row(sql, [key], |r| r.get(0)).unwrap()
    };
    for (i, (from, to)) in ALLOWED.iter().enumerate() {
        let key = format!("f{i}");
        walk(&ledger, plan, &key, &[from, to]);
        assert_eq!(status(&key), *to);
    }
    // Every pair the table lacks, with all a move could need.
    for (i, from) in STATUSES.iter().enumerate() {
        let key = format!("f{}", 20 + i);
        walk(&ledger, plan, &key, &[from]);
        for to in STATUSES[1..]
            .iter()
            .filter(|to| !ALLOWED.contains(&(from, to)))
        {
            let before = events();
            let got = mv(&ledger, plan, ("2", &key, to), "r", "abc1234");
            let want = match from == to {
                true => format!("finding {plan}/{key} is already {to}"),
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
        2 f31 disputed r - only the lane disputes or accepts a finding
        1 f31 accepted - - a finding goes accepted only with a reason
        1 f31 accepted r - only a plan check's finding is accepted
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
/// makes the verifier's moves only, on a harness review's findings only.
/// The same move without it, or on another check's finding, is refused,
/// as is one out of `refuted` (an overturn is the owner's) and a ruling on
/// an owner-held dispute: titled `owner:`, disputed twice, or verified by
/// the owner.
#[test]
fn the_harness_rules_from_the_lanes_session() {
    let (_dir, path, ledger) = common::ledger();
    let (_, local) = review(&ledger, &["blocker:a"]);
    let at = "2099-01-01T00:00:00.000Z";
    let harness = json!({"kind": "harness", "bin": "chaperone", "version": "0"});
    let found = ["b", "c", "d", "e", "g", "h"].map(|k| {
        let title = if k == "e" {
            "owner: drops a table"
        } else {
            "t"
        };
        json!({"finding_key": k, "severity": "blocker", "title": title, "body": "b"})
    });
    let payload = json!({"check": "review", "outcome": "ran", "head_sha": "abc1234",
        "producer": harness, "findings": found});
    let event = |kind, payload: serde_json::Value| Mutation::Event {
        kind,
        lane_id: Some(1),
        spec_id: None,
        payload: serde_json::from_value(payload).unwrap(),
    };
    let applied = ledger.apply(
        &actor("9"),
        None,
        at,
        event(EventKind::CheckFinished, payload),
    );
    let check = common::committed(applied.unwrap()).id;
    walk(&ledger, check, "d", &["disputed"]);
    let rule = |check: i64, key: &str, to: &str| {
        let payload = json!({"check_event_id": check, "finding_key": key, "status": to,
            "reason": "r", "producer": harness});
        ledger.apply(
            &actor("1"),
            None,
            at,
            event(EventKind::FindingUpdated, payload),
        )
    };
    assert!(mv(&ledger, check, ("1", "b", "verified"), "", "").is_err());
    common::committed(rule(check, "b", "verified").unwrap());
    common::committed(rule(check, "d", "refuted").unwrap());
    let refused = format!("{:?}", rule(check, "d", "verified").unwrap());
    assert!(refused.contains("session s1 has held lane 1"), "{refused}");
    walk(&ledger, check, "e", &["disputed"]);
    walk(&ledger, check, "g", &["verified", "disputed"]);
    walk(&ledger, check, "h", &["disputed"]);
    common::committed(rule(check, "h", "verified").unwrap());
    walk(&ledger, check, "h", &["disputed"]);
    for key in ["e", "g", "h"] {
        let refused = format!("{:?}", rule(check, key, "refuted").unwrap());
        assert!(refused.contains("is the owner's"), "{key}: {refused}");
    }
    for (check, key, to) in [(check, "c", "waived"), (local, "a", "verified")] {
        let refused = format!("{:?}", rule(check, key, to).unwrap());
        let said = format!("session s1 has held lane 1; a finding goes {to}");
        assert!(refused.contains(&said), "{refused}");
    }
    let blocking = ledger.blocking_findings(1).unwrap();
    let got: Vec<_> = blocking
        .iter()
        .map(|f| (&*f.finding_key, &*f.status))
        .collect();
    let want = [
        ("a", "open"),
        ("b", "verified"),
        ("c", "open"),
        ("e", "disputed"),
        ("g", "disputed"),
        ("h", "disputed"),
    ];
    assert_eq!(got, want);
    common::assert_consistent(&path);
}

/// A plan check's findings are the lane's to answer: it accepts one with a
/// reason; none blocks; the newest plan check, whatever its outcome,
/// decides which need answers.
#[test]
fn the_newest_plan_checks_findings_are_answered_and_never_block() {
    let (_dir, path, ledger) = common::ledger();
    review(&ledger, &[]);
    let plan = |p| plan_check(&ledger, p);
    let found = ["p1", "p2"]
        .map(|k| json!({"finding_key": k, "severity": "blocker", "title": "t", "body": "b"}));
    let check = plan(json!({"outcome": "ran", "findings": found}));
    assert!(ledger.blocking_findings(1).unwrap().is_empty());
    mv(&ledger, check, ("1", "p1", "accepted"), "r", "").unwrap();
    let answers = || {
        let found = ledger.plan_findings(1).unwrap().into_iter();
        found
            .map(|f| format!("{} {} {:?}", f.finding_key, f.status, f.reason))
            .collect::<Vec<_>>()
    };
    assert_eq!(answers(), ["p1 accepted Some(\"r\")", "p2 open None"]);
    plan(json!({"outcome": "skipped", "reason": "no_answer"}));
    assert!(answers().is_empty());
    common::assert_consistent(&path);
}

/// A waiver says its kind. fixed and disproved name a ref; scope, deferred
/// and accepted are countersigned by who writes them: a session with
/// no event on the lane but its own such waivers. Only scope releases an
/// accepted plan finding. Each refusal writes nothing.
#[test]
fn a_waiver_has_a_kind_a_ref_or_a_fresh_countersign() {
    let (_dir, path, ledger) = common::ledger();
    let (_, check) = review(&ledger, &["blocker:a", "blocker:b", "blocker:c"]);
    let found = [json!({"finding_key": "p", "severity": "blocker", "title": "t", "body": "b"})];
    let plan = plan_check(&ledger, json!({"outcome": "ran", "findings": found}));
    walk(&ledger, plan, "p", &["accepted"]);
    // Session 2 has written on the lane: it ruled c verified.
    walk(&ledger, check, "c", &["verified"]);
    let db = read_only(&path);
    let events = || count(&db, "SELECT count(*) FROM events");
    let (a, p) = ((check, "5", "a"), (plan, "5", "p"));
    let cases = [
        (a, ("", ""), "only with a waiver kind"),
        (a, ("waved", "x"), "only with a waiver kind"),
        (a, ("disproved", ""), "a disproved waiver needs a ref"),
        (a, ("fixed", ""), "a fixed waiver needs a ref"),
        ((check, "2", "a"), ("scope", "#243"), "countersigned"),
        // No session: every write needs one (`Actor::check`).
        (
            (check, "", "a"),
            ("deferred", "#9"),
            "needs an agent and a session",
        ),
        (p, ("deferred", "#243"), "waived only as scope"),
        (a, ("disproved", " "), "ref is one non-blank line"),
        (a, ("disproved", "a\nb"), "ref is one non-blank line"),
        (p, ("scope", ""), "waived only as scope, with the issue"),
    ];
    for ((check, who, key), kind, said) in cases {
        let before = events();
        let got = format!("{:?}", waive(&ledger, check, (who, key), kind).unwrap_err());
        assert!(got.contains(said), "{who} {kind:?}: {got}");
        assert_eq!(events(), before);
    }
    // A waiver, even a ref alone, rides only on a waive.
    let carried = FindingMove {
        check_event_id: check,
        finding_key: "a".into(),
        status: "disputed".into(),
        reason: Some("r".into()),
        waiver_ref: Some("#243".into()),
        ..FindingMove::default()
    };
    let got = ledger.finding_update(&actor("1"), &carried, false).unwrap();
    assert_eq!(
        got.unwrap_err(),
        Refusal::InvalidInput("only a waived finding carries a waiver".into())
    );
    // 2 has now written a disproved waiver: a fixed-or-disproved waiver does
    // not make a session fresh. 5 countersigns two, its own waivers only.
    let got = waive(&ledger, check, ("2", "a"), ("disproved", "src/x.rs:10"));
    assert_eq!(
        got.unwrap().payload.rest["waiver"],
        json!({"kind": "disproved", "ref": "src/x.rs:10"})
    );
    assert!(waive(&ledger, check, ("2", "b"), ("accepted", "")).is_err());
    waive(&ledger, check, ("5", "b"), ("accepted", "")).unwrap();
    // The lenses' plan text, and so a review's plan_answers, is as before
    // until a plan finding is waived with a kind.
    let shown = || review::plan_findings(&ledger, 1).unwrap();
    assert!(shown().contains("`waived` by the owner") && !shown().contains("waiver"));
    let moved = waive(&ledger, plan, ("5", "p"), ("scope", "#243")).unwrap();
    assert_eq!(moved.payload.rest["waiver"]["kind"], "scope");
    assert!(shown().contains(r#""waiver":"scope""#), "{}", shown());
    common::assert_consistent(&path);
}

/// The lane defers an open or verified harness review finding it cannot
/// fix in the lane, with a ref (#340): it stops blocking. Never one titled
/// `owner:`, a disputed one before its ruling, another check's, or one the
/// owner overturned, and nothing is written for a refusal.
#[test]
fn the_lane_defers_a_review_finding_it_cannot_fix() {
    let (_dir, path, ledger) = common::ledger();
    let (_, local) = review(&ledger, &["blocker:l"]);
    let harness = json!({"kind": "harness", "bin": "chaperone", "version": "0"});
    let found = ["a", "v", "o", "d", "x"].map(|k| {
        let title = if k == "o" {
            "owner: drops a table"
        } else {
            "t"
        };
        json!({"finding_key": k, "severity": "blocker", "title": title, "body": "b"})
    });
    let payload = json!({"check": "review", "outcome": "ran", "head_sha": "abc1234",
        "producer": harness, "complete": true, "findings": found});
    let m = Mutation::Event {
        kind: EventKind::CheckFinished,
        lane_id: Some(1),
        spec_id: None,
        payload: serde_json::from_value(payload).unwrap(),
    };
    let at = "2026-11-28T00:00:00.000Z";
    let check = common::committed(ledger.apply(&actor("9"), None, at, m).unwrap()).id;
    let ruling = json!({"check_event_id": check, "finding_key": "v", "status": "verified",
        "reason": "r", "producer": harness});
    let m = Mutation::Event {
        kind: EventKind::FindingUpdated,
        lane_id: Some(1),
        spec_id: None,
        payload: serde_json::from_value(ruling).unwrap(),
    };
    ledger.apply(&actor("1"), None, at, m).unwrap();
    walk(&ledger, check, "d", &["disputed"]);
    let db = read_only(&path);
    let events = || count(&db, "SELECT count(*) FROM events");
    let not_one = "the lane defers an open or verified review finding that is not the owner's";
    for ((check, key), kind, said) in [
        (
            (check, "a"),
            ("deferred", ""),
            "a lane's deferral needs a ref",
        ),
        ((check, "a"), ("disproved", "t.rs:1"), "has held lane 1"),
        ((check, "o"), ("deferred", "#9"), not_one),
        ((check, "d"), ("deferred", "#9"), not_one),
        ((local, "l"), ("deferred", "#9"), not_one),
    ] {
        let before = events();
        let got = format!("{:?}", waive(&ledger, check, ("1", key), kind).unwrap_err());
        assert!(got.contains(said), "{key} {kind:?}: {got}");
        assert_eq!(events(), before);
    }
    for key in ["a", "v", "x"] {
        let moved = waive(
            &ledger,
            check,
            ("1", key),
            ("deferred", "CONTRIBUTING.md rule"),
        );
        assert_eq!(moved.unwrap().payload.rest["waiver"]["kind"], "deferred");
    }
    let blocking = || {
        let found = ledger.blocking_findings(1).unwrap();
        found.into_iter().map(|f| f.finding_key).collect::<Vec<_>>()
    };
    assert_eq!(blocking(), ["l", "o", "d"]);
    // The owner overturns x: it blocks again, and is the owner's from then on.
    mv(&ledger, check, ("2", "x", "verified"), "r", "").unwrap();
    assert_eq!(blocking(), ["l", "o", "d", "x"]);
    let again = waive(&ledger, check, ("1", "x"), ("deferred", "#9")).unwrap_err();
    assert!(format!("{again:?}").contains(not_one), "{again:?}");
    common::assert_consistent(&path);
}

/// The lane labels an open harness review finding that does not block
/// with its outcome (#326): the status stays, so it still lists as open,
/// and nothing is written for a refusal: a blocking, settled, disputed or
/// other check's finding, an unknown outcome, no reason, or a session that
/// never held the lane.
#[test]
fn the_lane_labels_a_finding_it_leaves_open() {
    let (_dir, path, ledger) = common::ledger();
    let (_, local) = review(&ledger, &["nit:l"]);
    let harness = json!({"kind": "harness", "bin": "chaperone", "version": "0"});
    let found = [("n", "nit"), ("b", "blocker"), ("s", "nit"), ("d", "nit")]
        .map(|(k, s)| json!({"finding_key": k, "severity": s, "title": "t", "body": "b"}));
    let payload = json!({"check": "review", "outcome": "ran", "head_sha": "abc1234",
        "producer": harness, "complete": true, "findings": found});
    let m = Mutation::Event {
        kind: EventKind::CheckFinished,
        lane_id: Some(1),
        spec_id: None,
        payload: serde_json::from_value(payload).unwrap(),
    };
    let at = "2026-11-28T00:00:00.000Z";
    let check = common::committed(ledger.apply(&actor("9"), None, at, m).unwrap()).id;
    walk(&ledger, check, "s", &["waived"]);
    walk(&ledger, check, "d", &["disputed"]);
    let label = |who: &str, (check, key): (i64, &str), outcome: &str, reason: &str| {
        let input = FindingMove {
            check_event_id: check,
            finding_key: key.into(),
            reason: (!reason.is_empty()).then(|| reason.into()),
            ..FindingMove::default()
        };
        ledger
            .finding_label(&actor(who), &input, outcome, false)
            .unwrap()
    };
    let db = read_only(&path);
    let events = || count(&db, "SELECT count(*) FROM events");
    let not_one = "the lane labels an open harness review finding that does not block";
    for (who, at, outcome, reason, said) in [
        ("1", (local, "l"), "wrong", "r", not_one),
        ("1", (check, "b"), "wrong", "r", not_one),
        ("1", (check, "s"), "wrong", "r", not_one),
        ("1", (check, "d"), "wrong", "r", not_one),
        ("1", (check, "n"), "nope", "r", "outcome is one of"),
        (
            "1",
            (check, "n"),
            "wrong",
            "",
            "a label's reason is one non-blank line",
        ),
        (
            "2",
            (check, "n"),
            "wrong",
            "r",
            "only the lane labels a finding",
        ),
    ] {
        let before = events();
        let got = format!("{:?}", label(who, at, outcome, reason).unwrap_err());
        assert!(got.contains(said), "{at:?} {outcome}: {got}");
        assert_eq!(events(), before);
    }
    let moved = label("1", (check, "n"), "did_not_matter", "a wording choice").unwrap();
    assert_eq!(moved.payload.rest["label"], "did_not_matter");
    let n = ledger.lane_findings(1).unwrap();
    let n = n.into_iter().find(|f| f.finding_key == "n").unwrap();
    let got = (n.status.as_str(), n.outcome.as_deref(), n.reason.as_deref());
    assert_eq!(
        got,
        ("open", Some("did_not_matter"), Some("a wording choice"))
    );
    common::assert_consistent(&path);
}
