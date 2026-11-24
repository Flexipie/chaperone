//! `merge` through the binary, with a fake `gh` first on `PATH` that logs
//! its argv, answers `pr view` from view.json and turns it MERGED on `pr
//! merge`, and a fake `herdr` that logs its argv. The gates, GitHub's head
//! and the replay are writer.rs's tests.

mod common;

use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// The lane's session, and its agent under a new session, are refused
/// before any gh call; the owner's run without --head merges the handoff
/// head with the exact argv and, the lease live, tells the lane once. A
/// herdr that fails is reported, and the merge still succeeds.
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
    let herdr_log = dir.path().join("herdr.log");
    let herdr = bin.join("herdr");
    let fake_herdr = |exit: u8| {
        let body = format!("#!/bin/sh\necho \"$*\" >> {herdr_log:?}\necho down >&2\nexit {exit}\n");
        std::fs::write(&herdr, body).unwrap();
    };
    fake_herdr(0);
    for f in [&gh, &herdr] {
        let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        std::fs::set_permissions(f, mode).unwrap();
    }
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
    for args in [
        &["work", "create", "--title", "t"][..],
        &["work", "claim", "1"],
    ] {
        assert!(run("lane", "lane", args).0, "{args:?}");
    }
    // A v19 lane records its verdict from a session.
    common::restamp(&db, 1, "v19");
    for (who, args) in [
        (
            "lane",
            &["work", "handoff", "1", handoff.to_str().unwrap()][..],
        ),
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
    let sha = "e".repeat(40);
    let mut done = json!({"work_id": 1, "pr": "o/r#1", "merge_sha": sha, "done_by": "lane",
        "told": true, "tell_error": null});
    assert_eq!(out, done);
    let told = format!(
        "agent prompt lane Owner (chaperone merge): merged o/r#1 as {sha} (work merged \
         recorded). Hand off work 1 done the /ship way.\n"
    );
    assert_eq!(std::fs::read_to_string(&herdr_log).unwrap(), told);
    fake_herdr(1);
    let (ok, out) = run("owner", "owner", &["merge", "--work", "1"]);
    assert!(ok, "{out}");
    done["told"] = json!(false);
    done["tell_error"] = json!("herdr agent prompt: down");
    assert_eq!(out, done);
    let log = std::fs::read_to_string(&log).unwrap();
    let merges: Vec<_> = log.lines().filter(|l| l.starts_with("pr merge")).collect();
    let merge = format!("pr merge 1 -R o/r --squash --delete-branch --match-head-commit {head}");
    assert_eq!(merges, [merge]);
}

/// On v21 `work merged` asks GitHub: a PR in another repo, from a fork,
/// from another branch or not merged is refused, each saying which; else the merge is GitHub's (not the
/// flags'), and a lane with no PR takes it as its `pr_ref`; a lane with one
/// refuses any other. Bad input makes no gh call, and a gh that fails is
/// `internal`. Off v21 the merge's flags are required.
#[test]
fn work_merged_on_v21_records_what_github_shows() {
    let dir = tempfile::tempdir().unwrap();
    let (db, bin, view) = (
        dir.path().join("ledger.db"),
        dir.path().join("bin"),
        dir.path().join("view.json"),
    );
    std::fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    let log = dir.path().join("gh.log");
    std::fs::write(
        &gh,
        format!("#!/bin/sh\necho \"$*\" >> {log:?}\ncat {view:?}\n"),
    )
    .unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&gh, mode).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = |args: &[&str]| -> (bool, Value) {
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args(args)
            .envs([("DAISY_AGENT", "lane"), ("DAISY_SESSION", "lane")])
            .env("PATH", &path)
            .output()
            .unwrap();
        let ok = out.status.success();
        let v: Value = serde_json::from_slice(if ok { &out.stdout } else { &out.stderr }).unwrap();
        (ok, v[if ok { "data" } else { "error" }].clone())
    };
    for args in [
        &["migrate"][..],
        &["work", "create", "--title", "t", "--repo", "o/r"],
        &["work", "claim", "1", "--branch", "lane-b"],
        &["work", "create", "--title", "t"],
        &["work", "claim", "2"],
    ] {
        assert!(run(args).0, "{args:?}");
    }
    common::restamp(&db, 2, "v20");
    let (ok, err) = run(&["work", "merged", "2", "--pr", "o/r#9"]);
    assert!(
        !ok && err["message"] == "--merge-sha is required off v21",
        "{err}"
    );
    // A branch "fork" is the lane's branch name from another repository.
    let merged = |pr: &str, branch: &str, state: &str| {
        let fork = branch == "fork";
        let v = json!({"headRefName": if fork { "lane-b" } else { branch },
            "isCrossRepository": fork, "headRefOid": "1234567abc", "state": state,
            "mergeCommit": {"oid": "abcdef1"}, "mergedAt": "2026-11-05T10:00:00Z"});
        std::fs::write(&view, v.to_string()).unwrap();
        // The flags are not GitHub's, so they are not what is recorded.
        let flags = ["--merge-sha", "fffffff", "--head", "eeeeeee"];
        run(&[&["work", "merged", "1", "--pr", pr][..], &flags].concat())
    };
    for (pr, branch, state, why) in [
        (
            "x/y#5",
            "lane-b",
            "MERGED",
            "x/y#5 is not in the lane's repo o/r",
        ),
        (
            "o/r#5",
            "fork",
            "MERGED",
            "o/r#5 has its head in another repository on GitHub",
        ),
        (
            "o/r#5",
            "other",
            "MERGED",
            "o/r#5 has head branch other on GitHub, not the lane's branch lane-b",
        ),
        ("o/r#5", "lane-b", "OPEN", "o/r#5 is not merged on GitHub"),
    ] {
        let (ok, err) = merged(pr, branch, state);
        assert!(!ok && err["message"] == why, "{why}: {err}");
    }
    std::fs::remove_file(&log).unwrap();
    for bad in ["not a pr", "o/r#abc"] {
        let (ok, err) = merged(bad, "lane-b", "MERGED");
        assert!(
            !ok && err["code"] == "invalid_input" && !log.exists(),
            "{err}"
        );
    }
    std::fs::remove_file(&view).unwrap();
    let (ok, err) = run(&["work", "merged", "1", "--pr", "o/r#5"]);
    assert!(!ok && err["code"] == "internal", "{err}");
    let (ok, out) = merged("o/r#5", "lane-b", "MERGED");
    assert!(ok, "{out}");
    let p = &out["payload"];
    assert_eq!(
        (&p["merge_sha"], &p["head_sha"], &p["merged_at"]),
        (
            &json!("abcdef1"),
            &json!("1234567abc"),
            &json!("2026-11-05T10:00:00.000Z")
        )
    );
    assert_eq!(run(&["work", "get", "1"]).1["pr_ref"], "o/r#5");
    // The lane moves on to another PR; the old merge is not its PR now.
    let at = run(&["work", "get", "1"]).1["updated_at"].clone();
    let update = ["work", "update", "1", "--pr", "o/r#6"];
    let cas = ["--expected-updated-at", at.as_str().unwrap()];
    assert!(run(&[&update[..], &cas].concat()).0);
    let (ok, err) = merged("o/r#5", "lane-b", "MERGED");
    assert!(
        !ok && err["message"] == "o/r#5 is not the lane's PR o/r#6",
        "{err}"
    );
    assert_eq!(run(&["work", "get", "1"]).1["pr_ref"], "o/r#6");
}
