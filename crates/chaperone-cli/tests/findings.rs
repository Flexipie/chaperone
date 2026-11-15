//! `finding dispute` and `finding waive` through the binary: the lane
//! disputes, the owner waives, and each refuses the other's session.

use std::process::Command;

use chaperone_core::ledger::{Actor, EventKind, Ledger, Mutation};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

#[test]
fn the_lane_disputes_and_the_owner_waives() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let db = path.to_str().unwrap();
    // The envelope: stdout's on success, stderr's on a refusal.
    let run = |session: &str, args: &[&str]| -> Value {
        let out = Command::new(BIN)
            .args(["--db", db])
            .args(args)
            .args(["--agent", "claude", "--session", session])
            .output()
            .unwrap();
        let body = if out.status.success() {
            out.stdout
        } else {
            out.stderr
        };
        serde_json::from_slice(&body).unwrap()
    };
    let migrated = Command::new(BIN).args(["--db", db, "migrate"]).status();
    assert!(migrated.unwrap().success());
    run("lane", &["work", "create", "--title", "t"]);
    run("lane", &["work", "claim", "1"]);
    // Event 3: a review raising f1, as `reviews record` writes it.
    let review = json!({"check": "local_review", "outcome": "ran", "head_sha": "abc1234",
        "findings": [{"finding_key": "f1", "severity": "blocker", "title": "t", "body": "b"}]});
    let m = Mutation::Event {
        kind: EventKind::CheckFinished,
        lane_id: Some(1),
        spec_id: None,
        payload: serde_json::from_value(review).unwrap(),
    };
    let daisy = Actor {
        agent: Some("daisy".into()),
        session: Some("job".into()),
    };
    let at = "2099-01-01T00:00:00.000Z";
    Ledger::open(&path)
        .unwrap()
        .apply(&daisy, None, at, m)
        .unwrap();
    let dispute = ["finding", "dispute", "3", "f1", "--reason", "not a bug"];
    let bare = ["finding", "waive", "3", "f1", "--reason", "owner's call"];
    let waive = [&bare[..], &["--kind", "disproved", "--ref", "src/x.rs:10"]].concat();
    // The verifier's moves are the harness's (`check run review`): no
    // command makes them and none takes a producer, so the lane's session
    // cannot rule on its own finding.
    let smuggled = [&dispute[..], &["--producer", "harness"]].concat();
    for args in [
        &["finding", "verify", "3", "f1"][..],
        &["finding", "refute", "3", "f1"],
        &smuggled,
    ] {
        assert_eq!(run("lane", args)["error"]["code"], "usage", "{args:?}");
    }
    assert_eq!(run("owner", &dispute)["error"]["code"], "invalid_input");
    assert_eq!(run("lane", &waive)["error"]["code"], "invalid_input");
    // A waiver says its kind; disproved names a ref; scope is written by a
    // session fresh to the lane, and the lane's is not.
    assert_eq!(run("owner", &bare)["error"]["code"], "usage");
    let no_ref = [&bare[..], &["--kind", "disproved"]].concat();
    let said = run("owner", &no_ref)["error"]["message"].clone();
    assert_eq!(
        said,
        "a disproved waiver needs a ref: the commit and test, or the file:line"
    );
    let scope = [&bare[..], &["--kind", "scope", "--ref", "#243"]].concat();
    let said = run("lane", &scope)["error"]["message"].clone();
    assert!(said.as_str().unwrap().contains("has held lane 1"), "{said}");
    // A dry run writes nothing, so the real dispute is not a repeat.
    let dry = run("lane", &[&dispute[..], &["--dry-run"]].concat());
    assert_eq!(dry["meta"]["dry_run"], true);
    for (session, args, status) in [
        ("lane", &dispute[..], "disputed"),
        ("owner", &waive, "waived"),
    ] {
        let done = run(session, args)["data"].clone();
        assert_eq!(done["kind"], "finding.updated");
        let waiver =
            (status == "waived").then(|| json!({"kind": "disproved", "ref": "src/x.rs:10"}));
        assert_eq!(done["payload"].get("waiver"), waiver.as_ref());
        assert_eq!(
            (&done["work_id"], &done["payload"]["status"]),
            (&json!(1), &json!(status))
        );
    }
    let unknown = [&bare[..3], &["f2", "--reason", "r", "--kind", "scope"]].concat();
    let missing = run("owner", &unknown)["error"].clone();
    assert_eq!(
        (&missing["code"], &missing["details"]["reference"]),
        (&json!("not_found"), &json!("3/f2"))
    );
    // The same move again says so: a retry after a lost answer.
    let again = run("owner", &waive)["error"]["message"].clone();
    assert_eq!(again, "finding 3/f1 is already waived");
}
