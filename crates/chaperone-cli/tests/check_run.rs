//! `check run` through the binary: `diff` measures the lane's commit in its
//! worktree, `tests` runs the base profile's commands in a checkout of it;
//! each writes one harness check, or nothing.

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

/// The branch tip is measured exactly against the remote base, past a
/// local branch named like it, while a later layer is checked out; the same
/// commit replays, a reworded one and a return to the first append; docs,
/// lock files and lines git does not count are no code lines.
#[test]
fn a_run_measures_the_lane_commit() {
    let dir = tempfile::tempdir().unwrap();
    let (base, db, wt) = (
        setup(dir.path()),
        dir.path().join("ledger.db"),
        dir.path().join("wt"),
    );
    git(&wt, &["update-ref", "refs/remotes/origin/main", &base], &[]);
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
    // A local branch that `origin/main` alone would name first.
    git(&wt, &["branch", "origin/main", &tip], &[]);
    git(&wt, &["checkout", "-qb", "layer-2"], &[]);
    git(&wt, &["commit", "-q", "--allow-empty", "-m", "two"], &[]);
    let claim = |branch| ["--worktree", wt.to_str().unwrap(), "--branch", branch];
    let id = lane(&db, &claim("feat"));
    let diff = ["check", "run", "diff", "--work", &id];
    let (code, first) = run(&db, &diff);
    assert_eq!(code, 0, "{first}");
    let mut p = first["payload"].clone();
    let producer = p.as_object_mut().unwrap().remove("producer").unwrap();
    let evidence = p.as_object_mut().unwrap().remove("evidence_ref").unwrap();
    let want = json!({"check": "diff", "outcome": "ran", "head_sha": tip, "tree": tree,
        "base": "origin/main", "files": 3, "added": 3, "removed": 1, "code_lines": 3,
        "changes_code": true, "uncounted_files": 1, "files_truncated": false});
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
    let rows: Vec<Value> = list
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let rows: Vec<_> = rows
        .iter()
        .map(|f| json!([f["path"], f["added"], f["removed"]]))
        .collect();
    let want = [
        json!(["blob.bin", null, null]),
        json!(["docs/n.md", 1, 0]),
        json!(["lib.rs", 2, 1]),
    ];
    assert_eq!(rows, want);

    let event = |want_replayed: bool| {
        let got = run(&db, &diff).1;
        assert_eq!(got["replayed"], want_replayed, "{got}");
        (got["event_id"].clone(), got["payload"]["head_sha"].clone())
    };
    assert_eq!(event(true).0, first["event_id"]);
    git(&wt, &["checkout", "-q", "feat"], &[]);
    git(&wt, &["commit", "-q", "--amend", "-m", "reworded"], &[]);
    let reworded = git(&wt, &["rev-parse", "HEAD"], &[]);
    let (second, head) = event(false);
    assert_eq!(head, reworded);
    git(&wt, &["reset", "-q", "--hard", &tip], &[]);
    let (third, head) = event(false);
    assert_eq!(head, tip);
    assert!(third != first["event_id"] && third != second);

    let attributes: &[(&str, &[u8])] = &[(".git/info/attributes", b"README.md -diff\n")];
    git(&wt, &["checkout", "-qb", "docs", &base], attributes);
    let docs: &[(&str, &[u8])] = &[
        ("docs/a.md", b"x\n"),
        ("README.md", b"r\n"),
        ("Cargo.lock", b"l\n"),
    ];
    git(&wt, &["add", "."], docs);
    git(&wt, &["commit", "-qm", "docs only"], &[]);
    let id = lane(&db, &claim("docs"));
    let p = &run(&db, &["check", "run", "diff", "--work", &id]).1["payload"];
    let got = ["files", "uncounted_files", "code_lines", "changes_code"].map(|k| &p[k]);
    assert_eq!(got, [&json!(3), &json!(1), &json!(0), &json!(false)]);
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
    let slash = format!("{wt}/");
    assert_eq!(run(&db, &["work", "claim", &id, "--worktree", &slash]).0, 0);
    let err = run(&db, &["work", "claim", &id, "--branch", "other"]).1;
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .starts_with("branch is frozen"),
        "{err}"
    );
    // Lane 4 (the folder that is no repository) never ran a harness check.
    assert_eq!(run(&db, &["work", "claim", "4", "--worktree", &wt]).0, 0);
}

/// A repo whose lane `feat` changes `lib.rs` and carries its own profile
/// that would pass, a ledger and the lane on it; the db, the worktree and
/// the lane's id.
fn tests_lane(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let (db, wt) = (dir.join("ledger.db"), dir.join("wt"));
    setup(dir);
    let own = b"[tests]\ncommands = [[\"true\"]]\n";
    git(
        &wt,
        &["checkout", "-qb", "feat"],
        &[(".chaperone/profile.toml", own)],
    );
    git(&wt, &["add", "."], &[("lib.rs", b"a\nc\n")]);
    git(&wt, &["commit", "-qm", "feat"], &[]);
    let id = lane(
        &db,
        &["--worktree", wt.to_str().unwrap(), "--branch", "feat"],
    );
    (db, wt, id)
}

/// Points `origin/main` at a new commit on `main` whose profile has these
/// `[tests]` lines, or none, and checks the lane's branch out again.
fn base_tests(wt: &Path, tests: Option<&str>) {
    git(wt, &["checkout", "-q", "main"], &[]);
    match tests {
        Some(t) => git(
            wt,
            &["add", "."],
            &[(
                ".chaperone/profile.toml",
                format!("[tests]\n{t}\n").as_bytes(),
            )],
        ),
        None => git(
            wt,
            &["rm", "-q", "--ignore-unmatch", ".chaperone/profile.toml"],
            &[],
        ),
    };
    git(wt, &["commit", "-q", "--allow-empty", "-m", "profile"], &[]);
    git(wt, &["update-ref", "refs/remotes/origin/main", "HEAD"], &[]);
    git(wt, &["checkout", "-q", "feat"], &[]);
}

/// A pass on the base's commands, in a checkout the harness removes, with
/// the lane's worktree untouched; a failure stops at its command and runs
/// again; a pass replays without running anything.
#[test]
fn tests_run_in_a_checkout_and_a_pass_replays() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    let (mark, flag) = (dir.path().join("mark"), dir.path().join("flag"));
    let (mark, flag) = (mark.to_str().unwrap(), flag.to_str().unwrap());
    base_tests(
        &wt,
        Some(&format!(
            "commands = [[\"sh\", \"-c\", \"echo run >> {mark}; test ! -e {flag} && test -e lib.rs\"], \
         [\"sh\", \"-c\", \"echo \\\"second $T\\\" >> {mark}\"]]\ntarget_dir = \"T\""
        )),
    );
    let tests = ["check", "run", "tests", "--work", &id];
    let marks = || std::fs::read_to_string(dir.path().join("mark")).unwrap_or_default();
    let lane_state = || {
        (
            git(&wt, &["status", "--porcelain"], &[]),
            git(&wt, &["rev-parse", "HEAD"], &[]),
        )
    };
    let before = lane_state();

    std::fs::write(flag, "").unwrap();
    let (code, failed) = run(&db, &tests);
    assert_eq!(code, 0, "{failed}");
    let p = &failed["payload"];
    assert_eq!(
        (&p["result"], &p["dirty"], &p["commands"][0]["exit"]),
        (&json!("fail"), &Value::Null, &json!(1))
    );
    assert_eq!(p["commands"].as_array().unwrap().len(), 1);
    assert_eq!(marks(), "run\n");
    let again = run(&db, &tests).1;
    assert_eq!(
        (&again["replayed"], marks().as_str()),
        (&json!(false), "run\nrun\n")
    );

    std::fs::remove_file(flag).unwrap();
    let passed = run(&db, &tests).1;
    let p = &passed["payload"];
    let target = std::path::absolute(dir.path())
        .unwrap()
        .join("targets")
        .join(format!("lane-{id}"));
    assert_eq!(
        marks(),
        format!("run\nrun\nrun\nsecond {}\n", target.display())
    );
    assert_eq!(
        (&p["check"], &p["outcome"], &p["result"]),
        (&json!("tests"), &json!("ran"), &json!("pass"))
    );
    assert_eq!(
        (&p["timed_out"], &p["dirty"], &p["producer"]["kind"]),
        (&json!(false), &json!(false), &json!("harness"))
    );
    assert_eq!(p["head_sha"], git(&wt, &["rev-parse", "feat"], &[]));
    assert_eq!(p["tree"], git(&wt, &["rev-parse", "feat^{tree}"], &[]));
    assert_eq!(p["profile"].as_str().unwrap().len(), 64);
    let exits: Vec<_> = p["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["exit"].clone())
        .collect();
    assert_eq!(exits, [json!(0), json!(0)]);
    assert_eq!(p["commands"][1]["argv"][0], "sh");
    let output = Evidence::beside(&db)
        .get(p["evidence_ref"].as_str().unwrap())
        .unwrap();
    assert!(output.contains("$ git status --porcelain"), "{output}");

    let replayed = run(&db, &tests).1;
    assert_eq!(
        (&replayed["replayed"], &replayed["event_id"]),
        (&json!(true), &passed["event_id"])
    );
    assert_eq!(marks().lines().count(), 4);
    assert_eq!(lane_state(), before);
    assert!(
        !dir.path()
            .join("checkouts")
            .join(format!("lane-{id}"))
            .exists()
    );
    assert_eq!(git(&wt, &["worktree", "list"], &[]).lines().count(), 1);
}

/// The base's profile is the one run, not the lane's; a tree the commands
/// dirty, and a run past its time, fail; no tests on the base, and a run
/// already on the lane, write nothing.
#[test]
fn tests_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    let tests = ["check", "run", "tests", "--work", &id];
    let payload = || {
        let (code, out) = run(&db, &tests);
        assert_eq!(code, 0, "{out}");
        out["payload"].clone()
    };
    base_tests(&wt, Some("commands = [[\"false\"]]"));
    assert_eq!(payload()["result"], "fail");

    base_tests(
        &wt,
        Some("commands = [[\"sh\", \"-c\", \"echo x > new.txt\"]]"),
    );
    let p = payload();
    assert_eq!((&p["result"], &p["dirty"]), (&json!("fail"), &json!(true)));

    base_tests(
        &wt,
        Some("commands = [[\"sleep\", \"5\"], [\"true\"]]\ntimeout_secs = 1"),
    );
    let p = payload();
    assert_eq!(
        (&p["result"], &p["timed_out"]),
        (&json!("fail"), &json!(true))
    );
    assert_eq!(
        (
            &p["commands"][0]["exit"],
            p["commands"].as_array().unwrap().len()
        ),
        (&Value::Null, 1)
    );

    for (base, why) in [
        (None, "no .chaperone/profile.toml on the base"),
        (Some("commands = []"), "no [tests] commands"),
    ] {
        base_tests(&wt, base);
        let before = events(&db);
        let (code, err) = run(&db, &tests);
        assert_eq!(
            (code, &err["code"], events(&db)),
            (1, &json!("invalid_input"), before)
        );
        assert!(err["message"].as_str().unwrap().contains(why), "{err}");
    }

    base_tests(&wt, Some("commands = [[\"true\"]]"));
    let held = lock_lane(&db, id.parse().unwrap()).unwrap().unwrap();
    let before = events(&db);
    let (code, err) = run(&db, &tests);
    assert_eq!(
        (code, &err["code"], events(&db)),
        (4, &json!("conflict"), before)
    );
    drop(held);
    assert_eq!(payload()["result"], "pass");
}
