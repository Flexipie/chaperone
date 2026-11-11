//! `pr open` through the binary on a v20 lane whose `origin` is a local
//! bare repo, with a fake `gh` first on `PATH` that logs its argv and keeps
//! the body it was given. Its `pr create` opens #7 and then fails, as a run
//! that opened a PR and never recorded it; `pr list` lists #7 once opened.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// A git call in `wt`, after writing `files` there.
fn git(wt: &Path, args: &[&str], files: &[(&str, &[u8])]) -> String {
    for (path, bytes) in files {
        std::fs::write(wt.join(path), bytes).unwrap();
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(wt);
    let out = cmd.args(["-c", "user.name=t", "-c", "user.email=t@t"]);
    let out = out.args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

struct Lane {
    dir: tempfile::TempDir,
    db: PathBuf,
    wt: PathBuf,
}

impl Lane {
    /// `chaperone <args>` as session `who`: the exit code, and `data` on
    /// success or `error` on a refusal.
    fn run(&self, who: &str, args: &[&str]) -> (i32, Value) {
        let path = std::env::var("PATH").unwrap();
        let bin = self.dir.path().join("bin");
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .args(["--agent", who, "--session", who])
            .env("PATH", format!("{}:{path}", bin.display()))
            .output()
            .unwrap();
        let ok = out.status.success();
        let v: Value = serde_json::from_slice(if ok { &out.stdout } else { &out.stderr }).unwrap();
        (
            out.status.code().unwrap(),
            v[if ok { "data" } else { "error" }].clone(),
        )
    }

    fn file(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }

    /// Where `feat` is in the bare origin, empty when it has none.
    fn pushed(&self) -> String {
        let args = ["for-each-ref", "--format=%(objectname)", "refs/heads/feat"];
        git(&self.dir.path().join("origin.git"), &args, &[])
    }

    /// The lane's `pr.opened` events, as their head_sha.
    fn opened(&self) -> Vec<String> {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        let sql = "SELECT payload_json->>'head_sha' FROM events WHERE kind = 'pr.opened'";
        let mut stmt = conn.prepare(sql).unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.map(Result::unwrap).collect()
    }
}

/// Lane 1 on v20 on branch `feat`, one commit (one code line) past `main`
/// on `origin`; the base profile's test fails when a file `FAIL` exists.
/// Its plan check is waived by the owner; `create` adds to `work create`.
fn lane(create: &[&str]) -> Lane {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, bin) = (
        dir.path().join("ledger.db"),
        dir.path().join("wt"),
        dir.path().join("bin"),
    );
    std::fs::create_dir_all(wt.join(".chaperone")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let (log, body) = (dir.path().join("gh.log"), dir.path().join("body.md"));
    let opened = dir.path().join("opened");
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$*\" >> {log:?}\ncase $2 in\n\
             list) cat {opened:?} 2>/dev/null;;\n\
             create) cat > {body:?}; echo https://github.com/o/r/pull/7 > {opened:?}; exit 1;;\n\
             edit) cat > {body:?};;\nesac\nexit 0\n"
        ),
    )
    .unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&gh, mode).unwrap();
    let origin = dir.path().join("origin.git");
    let origin = origin.to_str().unwrap();
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", origin],
        &[],
    );
    let profile: &[u8] = b"[tests]\ncommands = [[\"test\", \"!\", \"-e\", \"FAIL\"]]\n";
    let files: &[(&str, &[u8])] = &[("lib.rs", b"a\n"), (".chaperone/profile.toml", profile)];
    git(&wt, &["init", "-q", "-b", "main"], files);
    git(&wt, &["add", "."], &[]);
    git(&wt, &["commit", "-qm", "base"], &[]);
    git(&wt, &["remote", "add", "origin", origin], &[]);
    git(&wt, &["push", "-q", "origin", "main"], &[]);
    git(&wt, &["checkout", "-qb", "feat"], &[]);
    git(&wt, &["commit", "-qam", "feat"], &[("lib.rs", b"a\nb\n")]);
    let migrate = Command::new(BIN)
        .arg("--db")
        .arg(&db)
        .arg("migrate")
        .status();
    assert!(migrate.unwrap().success());
    let lane = Lane { dir, db, wt };
    let wt = lane.wt.to_str().unwrap();
    let claim = ["work", "claim", "1", "--worktree", wt, "--branch", "feat"];
    let create = [&["work", "create", "--title", "t"], create].concat();
    assert_eq!(lane.run("lane", &create).0, 0);
    assert_eq!(
        lane.run("lane", &[&claim[..], &["--pipeline", "v20"]].concat())
            .0,
        0
    );
    let waive = ["check", "waive", "plan", "--work", "1", "--reason", "r"];
    assert_eq!(lane.run("owner", &waive).0, 0);
    lane
}

/// A passing lane's commit is pushed and its PR created; a PR created and
/// not recorded is found on the next run, which replays the checks, edits
/// its body of ledger facts and records it once on the head. A run after
/// that edits the PR the lane holds and records nothing.
#[test]
fn pr_open_creates_the_pr_from_ledger_facts() {
    let lane = lane(&[]);
    let head = git(&lane.wt, &["rev-parse", "HEAD"], &[]);
    let open = ["pr", "open", "--work", "1"];
    let (code, err) = lane.run("lane", &open);
    assert_eq!((code, &err["code"]), (1, &json!("internal")), "{err}");
    assert_eq!(lane.pushed(), head);
    let list = "pr list --head feat --json url --jq .[0].url\n";
    let create = "pr create --base main --head feat --title t --body-file -\n";
    assert_eq!(lane.file("gh.log"), [list, create].concat());
    assert!(lane.opened().is_empty());

    let (code, out) = lane.run("lane", &open);
    assert_eq!(code, 0, "{out}");
    let steps = out["steps"].as_array().unwrap().iter();
    let steps: Vec<_> = steps.map(|s| (&s["check"], &s["replayed"])).collect();
    assert_eq!(
        steps,
        [
            (&json!("diff"), &json!(true)),
            (&json!("tests"), &json!(true))
        ]
    );
    assert_eq!(
        (&out["pr"], &out["head"], &out["created"]),
        (&json!("o/r#7"), &json!(head), &json!(false))
    );
    let edit = "pr edit 7 --repo o/r --body-file -\n";
    assert_eq!(lane.file("gh.log"), [list, create, list, edit].concat());
    let body = format!(
        "t\n\n- Size: 1 code lines in 1 files\n- Tests: passed on {head}: `test ! -e FAIL`\n\
         - Plan check: skipped (waived)\n- Smoke: none\n\nMade by `chaperone pr open` from the ledger.\n"
    );
    assert_eq!(lane.file("body.md"), body);
    assert_eq!(lane.opened(), std::slice::from_ref(&head));

    assert_eq!(lane.run("lane", &open).0, 0);
    assert_eq!(
        lane.file("gh.log"),
        [list, create, list, edit, edit].concat()
    );
    assert_eq!(lane.opened(), [head]);
}

/// A dirty worktree, failing tests, a diff over v20's 400 code lines and
/// one with a file git counts no lines of are each refused before any push
/// or gh call.
#[test]
fn pr_open_refuses_what_cannot_pass() {
    let lane = lane(&[]);
    let refused = |says: &str| {
        let (code, err) = lane.run("lane", &["pr", "open", "--work", "1"]);
        assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
        let message = err["message"].as_str().unwrap();
        assert!(message.contains(says), "{message}");
        assert_eq!((lane.file("gh.log") + &lane.pushed()), "");
    };
    std::fs::write(lane.wt.join("stray.rs"), "x\n").unwrap();
    refused("clean at the tip of the lane's branch");
    std::fs::remove_file(lane.wt.join("stray.rs")).unwrap();
    git(&lane.wt, &["add", "FAIL"], &[("FAIL", b"x\n")]);
    git(&lane.wt, &["commit", "-qm", "fail"], &[]);
    refused("the tests failed");
    let big = "x\n".repeat(401);
    git(&lane.wt, &["add", "big.rs"], &[("big.rs", big.as_bytes())]);
    git(&lane.wt, &["commit", "-qm", "big"], &[]);
    refused("pipeline allows 400");
    git(&lane.wt, &["rm", "-q", "big.rs"], &[]);
    git(&lane.wt, &["add", "blob"], &[("blob", b"\0\x01")]);
    git(&lane.wt, &["commit", "-qm", "blob"], &[]);
    refused("and 1 uncounted");
}

/// A GitHub lane's body closes its issue: by its full key before the PR
/// exists, as `#N` once the PR is in the issue's repo, and always by its
/// full key from a PR in another repo.
#[test]
fn pr_open_closes_a_github_lanes_issue() {
    let open = ["pr", "open", "--work", "1"];
    for (issue, created, edited) in [
        ("o/r#5", "Closes o/r#5", "Closes #5"),
        ("x/y#5", "Closes x/y#5", "Closes x/y#5"),
    ] {
        let lane = lane(&["--source", "github", "--source-id", issue]);
        let closes = |line: &str| format!("Tracker: {issue}\n\n{line}\n\n- Size");
        assert_eq!(lane.run("lane", &open).0, 1);
        assert!(lane.file("body.md").contains(&closes(created)));
        assert_eq!(lane.run("lane", &open).0, 0);
        assert!(lane.file("body.md").contains(&closes(edited)));
    }
}
