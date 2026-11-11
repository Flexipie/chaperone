//! `merge` through the binary, with a fake `gh` first on `PATH` that logs
//! its argv, answers `pr view` from view.json and turns it MERGED on `pr
//! merge`.

use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// Without a YES verdict nothing calls gh. With a YES on the handoff head
/// a session that held the lane is refused, the owner merges once with the
/// exact argv and one `pr.merged`, and a second run merges nothing.
#[test]
fn merge_merges_only_a_yes_on_the_pr_head() {
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
    let run = |who: &str, args: &[&str]| -> (bool, Value) {
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args(args)
            .args(["--agent", who, "--session", who])
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
    for (who, args) in [
        ("lane", &["work", "create", "--title", "t"][..]),
        ("lane", &["work", "claim", "1"]),
        ("lane", &["work", "handoff", "1", handoff.to_str().unwrap()]),
        ("owner", &["work", "ready", "1", "--yes"]),
    ] {
        let (ok, out) = run(who, args);
        assert!(ok, "{args:?}: {out}");
    }
    let refused = |who: &str, says: &str| {
        let (ok, err) = run(who, &["merge", "--work", "1"]);
        assert!(!ok && err["code"] == "invalid_input", "{err}");
        assert!(err["message"].as_str().unwrap().contains(says), "{err}");
        assert!(!log.exists());
    };
    refused("owner", "the verdict is not YES");

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
    assert!(run("owner", &verdict).0);
    let state = |file: &std::path::Path, state: &str| {
        let pr = json!({"state": state, "headRefOid": head, "mergeCommit": {"oid": "e".repeat(40)},
            "mergedAt": "2026-11-09T10:00:00Z"});
        std::fs::write(file, pr.to_string()).unwrap();
    };
    state(&view, "OPEN");
    state(&merged, "MERGED");
    refused("lane", "session lane has held work item 1");

    let merge = format!("pr merge 1 -R o/r --squash --delete-branch --match-head-commit {head}");
    for _ in 0..2 {
        let (ok, out) = run("owner", &["merge", "--work", "1"]);
        assert!(ok, "{out}");
        let done = json!({"work_id": 1, "pr": "o/r#1", "merge_sha": "e".repeat(40),
            "done": false, "left": "the lane's agent hands off done"});
        assert_eq!(out, done);
        let log = std::fs::read_to_string(&log).unwrap();
        let merges: Vec<_> = log.lines().filter(|l| l.starts_with("pr merge")).collect();
        assert_eq!(merges, [merge.as_str()]);
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    let sql = "SELECT count(*) FROM events WHERE kind = 'pr.merged'";
    let n: i64 = conn.query_row(sql, [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}
