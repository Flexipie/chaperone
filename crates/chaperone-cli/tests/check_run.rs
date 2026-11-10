//! `check run diff` through the binary: it measures the lane's commit in
//! its worktree and writes one harness check, or nothing.

use std::path::Path;
use std::process::Command;

use chaperone_core::ledger::{Evidence, lock_lane};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// A git call in `wt`, after writing `files` there.
fn git(wt: &Path, args: &[&str], files: &[(&str, &[u8])]) -> String {
    for (path, bytes) in files {
        std::fs::create_dir_all(wt.join(path).parent().unwrap()).unwrap();
        std::fs::write(wt.join(path), bytes).unwrap();
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(wt);
    let out = cmd.args(["-c", "user.name=t", "-c", "user.email=t@t"]);
    let out = out.args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// The exit code, and `data` on success or `error` on a refusal.
fn run(db: &Path, args: &[&str]) -> (i32, Value) {
    let mut cmd = Command::new(BIN);
    cmd.arg("--db").arg(db).args(args);
    if args != ["migrate"] {
        cmd.args(["--agent", "a", "--session", "s"]);
    }
    let out = cmd.output().unwrap();
    let ok = out.status.success();
    let v: Value = serde_json::from_slice(if ok { &out.stdout } else { &out.stderr }).unwrap();
    (
        out.status.code().unwrap(),
        v[if ok { "data" } else { "error" }].clone(),
    )
}

fn events(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap()
}

/// A ledger and a repo `wt` in `dir` with one commit, whose sha it returns.
fn setup(dir: &Path) -> String {
    let wt = dir.join("wt");
    std::fs::create_dir(&wt).unwrap();
    git(&wt, &["init", "-q", "-b", "main"], &[("lib.rs", b"a\nb\n")]);
    git(&wt, &["add", "."], &[]);
    git(&wt, &["commit", "-qm", "base"], &[]);
    assert_eq!(run(&dir.join("ledger.db"), &["migrate"]).0, 0);
    git(&wt, &["rev-parse", "HEAD"], &[])
}

/// A new lane claimed with these flags; its id.
fn lane(db: &Path, claim: &[&str]) -> String {
    let id = run(db, &["work", "create", "--title", "t"]).1["id"].to_string();
    assert_eq!(run(db, &[&["work", "claim", &id][..], claim].concat()).0, 0);
    id
}

/// The branch tip is measured exactly while a later layer is checked out;
/// the same tree replays, a new commit appends, docs alone are no code.
#[test]
fn a_run_measures_the_lane_commit() {
    let dir = tempfile::tempdir().unwrap();
    let (base, db, wt) = (
        setup(dir.path()),
        dir.path().join("ledger.db"),
        dir.path().join("wt"),
    );
    let files: &[(&str, &[u8])] = &[
        ("lib.rs", b"a\nc\nd\n"),
        ("docs/n.md", b"x\n"),
        ("blob.bin", &[0, 1]),
    ];
    git(&wt, &["checkout", "-qb", "feat"], files);
    git(&wt, &["add", "."], &[]);
    git(&wt, &["commit", "-qm", "feat"], &[]);
    let (tip, tree) = (
        git(&wt, &["rev-parse", "HEAD"], &[]),
        git(&wt, &["rev-parse", "HEAD^{tree}"], &[]),
    );
    git(&wt, &["checkout", "-qb", "layer-2"], &[]);
    git(&wt, &["commit", "-q", "--allow-empty", "-m", "two"], &[]);
    let claim = |branch| {
        [
            "--worktree",
            wt.to_str().unwrap(),
            "--branch",
            branch,
            "--base-sha",
            &base,
        ]
    };
    let id = lane(&db, &claim("feat"));
    let diff = ["check", "run", "diff", "--work", &id];
    let (code, first) = run(&db, &diff);
    assert_eq!(code, 0, "{first}");
    let mut p = first["payload"].clone();
    let producer = p.as_object_mut().unwrap().remove("producer").unwrap();
    let evidence = p.as_object_mut().unwrap().remove("evidence_ref").unwrap();
    let want = json!({"check": "diff", "outcome": "ran", "head_sha": tip, "tree": tree, "base": base,
        "files": 3, "added": 3, "removed": 1, "code_lines": 3, "changes_code": true});
    assert_eq!(p, want);
    assert_eq!(producer["kind"], "harness");
    assert!(producer["bin"].as_str().unwrap().ends_with("/chaperone"));
    assert!(
        producer["version"]
            .as_str()
            .unwrap()
            .starts_with(env!("CARGO_PKG_VERSION"))
    );
    let list = Evidence::beside(&db)
        .get(evidence.as_str().unwrap())
        .unwrap();
    let list: Vec<Value> = serde_json::from_str(&list).unwrap();
    let rows: Vec<_> = list
        .iter()
        .map(|f| json!([f["path"], f["added"]]))
        .collect();
    assert_eq!(
        rows,
        [
            json!(["blob.bin", null]),
            json!(["docs/n.md", 1]),
            json!(["lib.rs", 2])
        ]
    );

    let again = run(&db, &diff).1;
    assert_eq!(
        (&again["replayed"], &again["event_id"]),
        (&json!(true), &first["event_id"])
    );
    git(&wt, &["checkout", "-q", "feat"], &[]);
    git(&wt, &["commit", "-qam", "docs"], &[("docs/n.md", b"y\n")]);
    let next = run(&db, &diff).1;
    assert_eq!(next["replayed"], false);
    assert_ne!(next["event_id"], first["event_id"]);

    git(&wt, &["checkout", "-qb", "docs", &base], &[]);
    git(
        &wt,
        &["add", "."],
        &[("docs/a.md", b"x\n"), ("README.md", b"r\n")],
    );
    git(&wt, &["commit", "-qm", "docs only"], &[]);
    let id = lane(&db, &claim("docs"));
    let p = &run(&db, &["check", "run", "diff", "--work", &id]).1["payload"];
    assert_eq!(
        [&p["files"], &p["code_lines"], &p["changes_code"]],
        [&json!(2), &json!(0), &json!(false)]
    );
}

/// Nothing to measure, git that fails and a run already on the lane are
/// refused and write nothing; once a lane has a harness check its worktree
/// may not move, while a lane without one still may.
#[test]
fn refusals_write_nothing_and_the_worktree_freezes() {
    let dir = tempfile::tempdir().unwrap();
    let (base, db) = (setup(dir.path()), dir.path().join("ledger.db"));
    let wt = dir.path().join("wt").to_str().unwrap().to_string();
    let (empty, gone) = (dir.path().to_str().unwrap(), format!("{wt}/gone"));
    for (claim, why) in [
        (&[][..], "has no worktree to measure"),
        (&["--worktree", "wt"], "has no worktree to measure"),
        (&["--worktree", &gone], "has no worktree to measure"),
        (&["--worktree", empty], "could not read the commit in"),
        (&["--worktree", &wt], "could not diff"),
    ] {
        let (id, before) = (lane(&db, claim), events(&db));
        let (code, err) = run(&db, &["check", "run", "diff", "--work", &id]);
        assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
        assert!(err["message"].as_str().unwrap().contains(why), "{err}");
        assert_eq!(events(&db), before, "{claim:?}");
    }
    assert_eq!(
        run(&db, &["check", "run", "diff", "--work", "99"]).1["code"],
        "not_found"
    );

    let id = lane(&db, &["--worktree", &wt, "--base-sha", &base]);
    let diff = ["check", "run", "diff", "--work", &id];
    let (held, before) = (
        lock_lane(&db, id.parse().unwrap()).unwrap().unwrap(),
        events(&db),
    );
    let (code, err) = run(&db, &diff);
    assert_eq!(
        (code, &err["code"], events(&db)),
        (4, &json!("conflict"), before)
    );
    drop(held);
    assert_eq!(run(&db, &diff).0, 0);

    let (code, err) = run(&db, &["work", "claim", &id, "--worktree", empty]);
    assert!(
        code == 1
            && err["message"]
                .as_str()
                .unwrap()
                .starts_with("worktree_path is frozen")
    );
    assert_eq!(run(&db, &["work", "claim", &id, "--worktree", &wt]).0, 0);
    // Lane 4 (the folder that is no repository) never ran a harness check.
    assert_eq!(run(&db, &["work", "claim", "4", "--worktree", &wt]).0, 0);
}
