//! `merge` through the binary, with a fake `gh` first on `PATH` that logs
//! its argv, answers `pr view` from view.json and turns it MERGED on `pr
//! merge`. The gates, GitHub's head and the replay are writer.rs's tests.

use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// The lane's session, and its agent under a new session, are refused
/// before any gh call; the owner's run without --head merges the handoff
/// head with the exact argv.
#[test]
fn merge_is_the_owners_and_defaults_to_the_handoff_head() {
    let dir = tempfile::tempdir().unwrap();
    let (db, bin) = (dir.path().join("ledger.db"), dir.path().join("bin"));
    std::fs::create_dir(&bin).unwrap();
    let (log, view, merged) = (
        dir.path().join("gh.log"),
        dir.path().join("view.json"),
        dir.path().join("merged.json"),
    );
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$*\" >> {log:?}\n\
             case $2 in view) cat {view:?};; merge) cp {merged:?} {view:?};; esac\n"
        ),
    )
    .unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&gh, mode).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = |agent: &str, session: &str, args: &[&str]| -> (bool, Value) {
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args(args)
            .args(["--agent", agent, "--session", session])
            .env("PATH", &path)
            .output()
            .unwrap();
        let ok = out.status.success();
        let v: Value = serde_json::from_slice(if ok { &out.stdout } else { &out.stderr }).unwrap();
        (ok, v[if ok { "data" } else { "error" }].clone())
    };
    let migrate = Command::new(BIN)
        .arg("--db")
        .arg(&db)
        .arg("migrate")
        .status();
    assert!(migrate.unwrap().success());
    let head = format!("abc1234{}", "d".repeat(33));
    let handoff = dir.path().join("handoff.json");
    let body = json!({"summary": "s", "commit": head, "pull_request": "o/r#1"});
    std::fs::write(&handoff, body.to_string()).unwrap();
    let verdict = [
        "check",
        "record",
        "--work",
        "1",
        "--check",
        "merge_verdict",
        "--head",
        &head,
        "--outcome",
        "ran",
        "--decision",
        "yes",
    ];
    for (who, args) in [
        ("lane", &["work", "create", "--title", "t"][..]),
        ("lane", &["work", "claim", "1"]),
        ("lane", &["work", "handoff", "1", handoff.to_str().unwrap()]),
        ("owner", &["work", "ready", "1", "--yes"]),
        ("owner", &verdict),
    ] {
        let (ok, out) = run(who, who, args);
        assert!(ok, "{args:?}: {out}");
    }
    let pr = |file: &std::path::Path, state: &str| {
        let pr = json!({"state": state, "headRefOid": head, "mergeCommit": {"oid": "e".repeat(40)},
            "mergedAt": "2026-11-09T10:00:00Z"});
        std::fs::write(file, pr.to_string()).unwrap();
    };
    pr(&view, "OPEN");
    pr(&merged, "MERGED");
    for session in ["lane", "fresh"] {
        let (ok, err) = run("lane", session, &["merge", "--work", "1"]);
        assert!(!ok && err["code"] == "invalid_input", "{err}");
        assert_eq!(
            err["message"],
            "only the owner merges this lane; ask the owner"
        );
        assert!(!log.exists());
    }

    let (ok, out) = run("owner", "owner", &["merge", "--work", "1"]);
    assert!(ok, "{out}");
    let done = json!({"work_id": 1, "pr": "o/r#1", "merge_sha": "e".repeat(40), "done_by": "lane"});
    assert_eq!(out, done);
    let log = std::fs::read_to_string(&log).unwrap();
    let merges: Vec<_> = log.lines().filter(|l| l.starts_with("pr merge")).collect();
    let merge = format!("pr merge 1 -R o/r --squash --delete-branch --match-head-commit {head}");
    assert_eq!(merges, [merge]);
}
