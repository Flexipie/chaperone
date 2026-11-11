//! `pr open` through the binary on a v20 lane whose `origin` is a local
//! bare repo, with a fake `gh` first on `PATH` that logs its argv and keeps
//! the body file it was given.

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
/// Its plan check is waived by the owner.
fn lane() -> Lane {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, bin) = (
        dir.path().join("ledger.db"),
        dir.path().join("wt"),
        dir.path().join("bin"),
    );
    std::fs::create_dir_all(wt.join(".chaperone")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let (log, body) = (dir.path().join("gh.log"), dir.path().join("body.md"));
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$*\" >> {log:?}\n\
             for a; do [ \"$prev\" = --body-file ] && cp \"$a\" {body:?}; prev=$a; done\n\
             [ \"$2\" = create ] && echo https://github.com/o/r/pull/7\nexit 0\n"
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
    assert_eq!(lane.run("lane", &["work", "create", "--title", "t"]).0, 0);
    assert_eq!(
        lane.run("lane", &[&claim[..], &["--pipeline", "v20"]].concat())
            .0,
        0
    );
    let waive = ["check", "waive", "plan", "--work", "1", "--reason", "r"];
    assert_eq!(lane.run("owner", &waive).0, 0);
    lane
}

/// A passing lane is pushed, its PR created with a body of ledger facts and
/// recorded once on the head; the same head again replays the checks and
/// edits the body of the PR the lane holds.
#[test]
fn pr_open_creates_the_pr_from_ledger_facts() {
    let lane = lane();
    let head = git(&lane.wt, &["rev-parse", "HEAD"], &[]);
    let open = ["pr", "open", "--work", "1"];
    let (code, out) = lane.run("lane", &open);
    assert_eq!(code, 0, "{out}");
    let steps = |out: &Value| {
        let steps = out["steps"].as_array().unwrap().iter();
        steps
            .map(|s| (s["check"].clone(), s["head"].clone(), s["replayed"].clone()))
            .collect::<Vec<_>>()
    };
    let ran = |replayed| ["diff", "tests"].map(|c| (json!(c), json!(head), json!(replayed)));
    assert_eq!(
        (&out["pr"], &out["head"], &out["created"]),
        (&json!("o/r#7"), &json!(head), &json!(true))
    );
    assert_eq!(steps(&out), ran(false));
    assert_eq!(lane.pushed(), head);
    let file = lane.dir.path().join("pr-body-lane-1.md");
    let file = file.to_str().unwrap();
    let create = format!("pr create --base main --head feat --title t --body-file {file}\n");
    assert_eq!(lane.file("gh.log"), create);
    let body = lane.file("body.md");
    assert!(body.contains("- Size: 1 code lines in 1 files\n"), "{body}");
    assert!(
        body.contains(&format!("- Tests: passed on {head}: `test ! -e FAIL`\n")),
        "{body}"
    );
    assert!(body.contains("- Plan check: skipped (waived)\n"), "{body}");
    assert_eq!(lane.opened(), std::slice::from_ref(&head));

    let (code, out) = lane.run("lane", &open);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        (&out["pr"], &out["created"]),
        (&json!("o/r#7"), &json!(false))
    );
    assert_eq!(steps(&out), ran(true));
    let edit = format!("pr edit 7 --body-file {file}\n");
    assert_eq!(lane.file("gh.log"), create + &edit);
    assert_eq!(lane.opened(), std::slice::from_ref(&head));
}

/// A dirty worktree, failing tests and a diff over v20's 400 code lines
/// are each refused before any push or gh call.
#[test]
fn pr_open_refuses_what_cannot_pass() {
    let lane = lane();
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
    refused("over the pipeline's limit of 400");
}
