//! `work handoff` through the binary: the CLI reads the lane's commit in
//! its worktree and the write compares the handoff with it.

use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// A ready handoff that names another commit than the lane's branch is on
/// is refused with the real one; a done handoff is left to the gate.
#[test]
fn a_ready_handoff_is_checked_against_the_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let (wt, db) = (dir.path().join("wt"), dir.path().join("ledger.db"));
    std::fs::create_dir(&wt).unwrap();
    let (wt, db) = (wt.to_str().unwrap(), db.to_str().unwrap());
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-C", wt, "-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "feat"]);
    git(&["commit", "-q", "--allow-empty", "-m", "one"]);
    let head = git(&["rev-parse", "HEAD"]);
    // The error object on a refusal, None when the command succeeded.
    let run = |args: &[&str]| -> Option<Value> {
        let out = Command::new(BIN)
            .args(["--db", db])
            .args(args)
            .args(["--agent", "claude", "--session", "lane"])
            .output()
            .unwrap();
        (!out.status.success()).then(|| serde_json::from_slice(&out.stderr).unwrap())
    };
    assert!(
        Command::new(BIN)
            .args(["--db", db, "migrate"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(run(&["work", "create", "--title", "t"]), None);
    assert_eq!(
        run(&["work", "claim", "1", "--worktree", wt, "--branch", "feat"]),
        None
    );
    let handoff = |status: &str| {
        let file = dir.path().join("handoff.json");
        let json =
            format!(r#"{{"summary":"s","status":"{status}","ready":true,"commit":"def5678"}}"#);
        std::fs::write(&file, json).unwrap();
        run(&["work", "handoff", "1", file.to_str().unwrap()]).unwrap()["error"].clone()
    };
    let refused = handoff("review");
    assert_eq!(refused["code"], "invalid_input");
    assert!(
        refused["message"].as_str().unwrap().contains(&head),
        "{refused}"
    );
    assert_eq!(handoff("done")["code"], "gate_denied");
}
