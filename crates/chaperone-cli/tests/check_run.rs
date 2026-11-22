//! `check run` through the binary: `diff` measures the lane's commit in its
//! worktree, `tests` runs the base profile's commands in a checkout of it;
//! each writes one harness check, or nothing.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use chaperone_core::ledger::{Evidence, lock_lane, lock_slot};
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
/// commit replays, a reworded one and a return to the first append; docs
/// and lines git does not count are no code lines, a lock file's are.
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
        "test_lines": 0, "changes_code": true, "uncounted_files": 1, "asset_files": 0, "files_truncated": false});
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
    assert_eq!(got, [&json!(3), &json!(1), &json!(1), &json!(true)]);

    git(&wt, &["checkout", "-qb", "prompt", &base], &[]);
    git(&wt, &["add", "."], &[("pipelines/p.md", b"x\n")]);
    git(&wt, &["commit", "-qm", "prompt"], &[]);
    let id = lane(&db, &claim("prompt"));
    let measured = || {
        let p = &run(&db, &["check", "run", "diff", "--work", &id]).1["payload"];
        [
            p["code_lines"].clone(),
            p["changes_code"].clone(),
            p["profile_base"].clone(),
        ]
    };
    assert_eq!(measured(), [json!(0), json!(false), Value::Null]);
    git(&wt, &["checkout", "-q", "main"], &[]);
    let profile: &[(&str, &[u8])] = &[(
        ".chaperone/profile.toml",
        b"[diff]\ncode = [\"pipelines/**\"]\n",
    )];
    git(&wt, &["add", "."], profile);
    git(&wt, &["commit", "-qm", "profile"], &[]);
    git(
        &wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    let at = json!({"ref": "origin/main", "commit": git(&wt, &["rev-parse", "HEAD"], &[])});
    git(&wt, &["checkout", "-q", "prompt"], &[]);
    assert_eq!(measured(), [json!(1), json!(true), at]);
}

/// Code files a base profile's `[size] tests` glob names count as test
/// lines, not code lines; they are still code. A `[size]` that does not
/// load is refused and writes nothing, as is a size waiver for a lane
/// with no worktree to read its commit in.
#[test]
fn test_files_are_test_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
    setup(dir.path());
    let profile: &[(&str, &[u8])] = &[(
        ".chaperone/profile.toml",
        b"[size]\ntests = [\"tests/**\"]\n",
    )];
    git(&wt, &["add", "."], profile);
    git(&wt, &["commit", "-qm", "profile"], &[]);
    git(
        &wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    let tests = "t\n".repeat(401);
    let files: &[(&str, &[u8])] = &[("lib.rs", b"a\nb\nc\n"), ("tests/t.rs", tests.as_bytes())];
    git(&wt, &["checkout", "-qb", "feat"], files);
    git(&wt, &["add", "."], &[]);
    git(&wt, &["commit", "-qm", "feat"], &[]);
    let id = lane(
        &db,
        &["--worktree", wt.to_str().unwrap(), "--branch", "feat"],
    );
    let p = &run(&db, &["check", "run", "diff", "--work", &id]).1["payload"];
    let got = ["code_lines", "test_lines", "changes_code"].map(|k| &p[k]);
    assert_eq!(got, [&json!(1), &json!(401), &json!(true)]);

    git(&wt, &["checkout", "-q", "main"], &[]);
    let bad: &[(&str, &[u8])] = &[(".chaperone/profile.toml", b"[size]\ntest = [\"t/**\"]\n")];
    git(&wt, &["add", "."], bad);
    git(&wt, &["commit", "-qm", "bad"], &[]);
    git(
        &wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    git(&wt, &["checkout", "-q", "feat"], &[]);
    let before = events(&db);
    let (code, err) = run(&db, &["check", "run", "diff", "--work", &id]);
    assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("unknown field `test`"),
        "{err}"
    );
    assert_eq!(events(&db), before);

    let bare = lane(&db, &[]);
    let waive = [
        "check", "waive", "size", "--work", &bare, "--lines", "5", "--reason", "r",
    ];
    let (code, err) = run(&db, &waive);
    assert_eq!(code, 1, "{err}");
    assert!(
        err["message"].as_str().unwrap().contains("has no worktree"),
        "{err}"
    );
    assert_eq!(events(&db), before + 2, "the lane's create and claim only");
}

/// A binary file an `[assets]` glob names, added or deleted, is an asset,
/// not uncounted; a binary no glob names, text an attributes rule hides and
/// any file once the diff changes a `.gitattributes` stay uncounted; a text
/// file a glob names keeps its lines; no table behaves as before, and a
/// table that does not load is refused and writes nothing.
#[test]
fn assets_are_named_not_uncounted() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
    setup(dir.path());
    let base = |profile: &[u8]| {
        git(&wt, &["checkout", "-q", "main"], &[]);
        git(&wt, &["add", "."], &[(".chaperone/profile.toml", profile)]);
        git(&wt, &["commit", "-qm", "profile"], &[]);
        git(
            &wt,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            &[],
        );
        git(&wt, &["checkout", "-q", "feat"], &[]);
        git(&wt, &["rebase", "-q", "main"], &[]);
    };
    git(&wt, &["add", "."], &[("old.png", b"\0old")]);
    git(&wt, &["commit", "-qm", "old"], &[]);
    git(&wt, &["checkout", "-qb", "feat"], &[]);
    base(b"[assets]\nglobs = [\"**/*.png\", \"*.svg\", \"*.txt\"]\n");
    let attributes: &[(&str, &[u8])] = &[(".git/info/attributes", b"hidden.txt -diff\n")];
    git(&wt, &["rm", "-q", "old.png"], attributes);
    let files: &[(&str, &[u8])] = &[
        ("img/a.png", b"\x89PNG\0\x01"),
        ("blob.bin", b"\0\x01"),
        ("icon.svg", b"<svg/>\n"),
        ("hidden.txt", b"fn x() {}\n"),
    ];
    git(&wt, &["add", "."], files);
    git(&wt, &["commit", "-qm", "feat"], &[]);
    let id = lane(
        &db,
        &["--worktree", wt.to_str().unwrap(), "--branch", "feat"],
    );
    let diff = ["check", "run", "diff", "--work", &id];
    let measured = || {
        let (code, data) = run(&db, &diff);
        assert_eq!(code, 0, "{data}");
        let p = &data["payload"];
        let got = ["asset_files", "uncounted_files", "code_lines"].map(|k| p[k].clone());
        (
            got,
            p["evidence_ref"].as_str().unwrap_or_default().to_string(),
        )
    };
    let (got, evidence) = measured();
    assert_eq!(got, [json!(2), json!(2), json!(1)]);
    let list = Evidence::beside(&db).get(&evidence).unwrap();
    let marked: Vec<_> = list
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| r["asset"] == true)
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(marked, ["img/a.png", "old.png"]);

    git(
        &wt,
        &["add", "."],
        &[("img/.gitattributes", b"*.png binary\n")],
    );
    git(&wt, &["commit", "-qm", "attributes"], &[]);
    assert_eq!(measured().0, [json!(0), json!(4), json!(2)]);

    base(b"[size]\ntests = []\n");
    assert_eq!(measured().0, [json!(0), json!(4), json!(2)]);

    base(b"[assets]\nglob = [\"*.png\"]\n");
    let before = events(&db);
    let (code, err) = run(&db, &diff);
    assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("unknown field `glob`"),
        "{err}"
    );
    assert_eq!(events(&db), before);
}

/// A commit that changes a file a base profile's companion rule names
/// without one it needs is refused and writes nothing; the needed file, or
/// a lane commit's `No-companion` trailer naming the rule and why, passes. A rule
/// with an empty list does not load.
#[test]
fn a_companion_rule_needs_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
    setup(dir.path());
    let rule = "[[companions]]\nname = \"contract\"\nwhen = [\"*.rs\"]\n";
    let profile = format!("{rule}needs = [\"docs/contract.md\"]\n");
    git(
        &wt,
        &["add", "."],
        &[(".chaperone/profile.toml", profile.as_bytes())],
    );
    git(&wt, &["commit", "-qm", "profile"], &[]);
    git(
        &wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    git(
        &wt,
        &["checkout", "-qb", "feat"],
        &[("lib.rs", b"changed\n")],
    );
    git(&wt, &["commit", "-qam", "code"], &[]);
    let id = lane(
        &db,
        &["--worktree", wt.to_str().unwrap(), "--branch", "feat"],
    );
    let diff = ["check", "run", "diff", "--work", &id];
    let before = events(&db);
    let (code, err) = run(&db, &diff);
    assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
    let why = "companion rule contract: lib.rs changed but not docs/contract.md";
    assert!(err["message"].as_str().unwrap().contains(why), "{err}");
    assert_eq!(events(&db), before);

    let bare = "skip\n\nNo-companion: contract";
    git(&wt, &["commit", "-q", "--allow-empty", "-m", bare], &[]);
    assert_eq!(run(&db, &diff).0, 1, "a waiver needs a why");
    git(&wt, &["reset", "-q", "--hard", "HEAD~1"], &[]);
    let folded = "skip\n\nNo-companion: other because\n contract is named on a continuation";
    git(&wt, &["commit", "-q", "--allow-empty", "-m", folded], &[]);
    assert_eq!(run(&db, &diff).0, 1, "a continuation line waives nothing");
    git(&wt, &["reset", "-q", "--hard", "HEAD~1"], &[]);
    let long = dir.path().join("long");
    let text = format!("skip\n\nNo-companion: contract {}", "x".repeat(1 << 20));
    std::fs::write(&long, text).unwrap();
    let file = long.to_str().unwrap();
    git(&wt, &["commit", "-q", "--allow-empty", "-F", file], &[]);
    let (code, err) = run(&db, &diff);
    assert_eq!(code, 1, "{err}");
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("over the output limit")
    );
    git(&wt, &["reset", "-q", "--hard", "HEAD~1"], &[]);
    let waive = "skip\n\nNo-companion: contract no behaviour change";
    git(&wt, &["commit", "-q", "--allow-empty", "-m", waive], &[]);
    assert_eq!(run(&db, &diff).0, 0);
    git(&wt, &["reset", "-q", "--hard", "HEAD~1"], &[]);
    assert_eq!(run(&db, &diff).0, 1);
    git(&wt, &["add", "."], &[("docs/contract.md", b"x\n")]);
    git(&wt, &["commit", "-qm", "doc"], &[]);
    assert_eq!(run(&db, &diff).0, 0);

    git(&wt, &["checkout", "-q", "main"], &[]);
    let needs = "needs = [\"docs/contract.md\"]\n";
    let bad = [
        format!("{rule}needs = []\n"),
        rule.replace("contract", "a b") + needs,
        rule.replace("contract", "a:b") + needs,
        rule.replace("contract", "") + needs,
    ];
    for profile in bad {
        let files: &[(&str, &[u8])] = &[(".chaperone/profile.toml", profile.as_bytes())];
        git(&wt, &["add", "."], files);
        git(&wt, &["commit", "-qm", "bad"], &[]);
        git(
            &wt,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            &[],
        );
        let (code, err) = run(&db, &diff);
        assert_eq!(code, 1, "{profile}: {err}");
        let message = err["message"].as_str().unwrap();
        assert!(
            message.contains("non-empty `when` and `needs`"),
            "{profile}: {err}"
        );
    }
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
    // `rel` stands for a relative path an older binary let a claim store.
    for (claim, why) in [
        (&[][..], "has no worktree to measure"),
        (&["rel"], "has no worktree to measure"),
        (&["--worktree", &gone], "has no worktree to measure"),
        (&["--worktree", empty], "could not read the commit in"),
        (&["--worktree", &wt], "could not diff"),
    ] {
        let id = lane(&db, if claim == ["rel"] { &[] } else { claim });
        if claim == ["rel"] {
            let conn = rusqlite::Connection::open(&db).unwrap();
            let sql = "UPDATE lanes SET worktree_path = 'wt' WHERE id = ?1";
            assert_eq!(conn.execute(sql, [&id]).unwrap(), 1);
        }
        let before = events(&db);
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
    // The lane had no branch when it was checked: it may set one, once.
    assert_eq!(run(&db, &["work", "claim", &id, "--branch", "one"]).0, 0);
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

/// A repo whose lane `feat` changes `lib.rs` to `a c`, ignores `built`
/// and carries its own profile that would pass, a ledger and the lane on
/// it; the db, the worktree and the lane's id.
fn tests_lane(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let (db, wt) = (dir.join("ledger.db"), dir.join("wt"));
    setup(dir);
    let own: &[u8] = b"[tests]\ncommands = [[\"true\"]]\n";
    let files: &[(&str, &[u8])] = &[
        (".chaperone/profile.toml", own),
        (".gitignore", b"built\n*.o\n"),
    ];
    git(&wt, &["checkout", "-qb", "feat"], files);
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
        Some(t) => {
            let profile = format!("[tests]\n{t}\n");
            git(
                wt,
                &["add", "."],
                &[(".chaperone/profile.toml", profile.as_bytes())],
            )
        }
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

/// What a run may not change in the lane's repository.
fn lane_repo(wt: &Path) -> [String; 4] {
    [
        "status --porcelain",
        "rev-parse HEAD",
        "for-each-ref",
        "worktree list",
    ]
    .map(|args| git(wt, &args.split(' ').collect::<Vec<_>>(), &[]))
}

/// A pass on the base's commands, in the harness's own checkout of the
/// lane's commit, with the lane's repository untouched and an ignored file
/// the commands made recorded; a failure stops at its command and runs
/// again; a pass replays without running anything.
#[test]
fn tests_run_in_a_checkout_and_a_pass_replays() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    let (mark, flag) = (dir.path().join("mark"), dir.path().join("flag"));
    let (mark, flag) = (mark.to_str().unwrap(), flag.to_str().unwrap());
    let first = format!(r#"echo run >> {mark}; test ! -e {flag} && grep -qx c lib.rs"#);
    let second = format!(r#"echo \"second $T\" >> {mark}; touch built"#);
    let commands = format!(
        "commands = [[\"sh\", \"-c\", \"{first}\"], [\"sh\", \"-c\", \"{second}\"]]\ntarget_dir = \"T\""
    );
    base_tests(&wt, Some(&commands));
    let tests = ["check", "run", "tests", "--work", &id];
    let marks = || std::fs::read_to_string(dir.path().join("mark")).unwrap_or_default();
    let before = lane_repo(&wt);

    std::fs::write(flag, "").unwrap();
    let (code, failed) = run(&db, &tests);
    assert_eq!(code, 0, "{failed}");
    let p = &failed["payload"];
    let got = (&p["result"], &p["dirty"], &p["commands"][0]["exit"]);
    assert_eq!(got, (&json!("fail"), &Value::Null, &json!(1)));
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
    let got = (&p["check"], &p["outcome"], &p["result"], &p["timed_out"]);
    assert_eq!(
        got,
        (
            &json!("tests"),
            &json!("ran"),
            &json!("pass"),
            &json!(false)
        )
    );
    let got = (&p["dirty"], &p["ignored_files"], &p["producer"]["kind"]);
    assert_eq!(got, (&json!(false), &json!(1), &json!("harness")));
    assert_eq!(p["head_sha"], git(&wt, &["rev-parse", "feat"], &[]));
    assert_eq!(p["tree"], git(&wt, &["rev-parse", "feat^{tree}"], &[]));
    assert_eq!(p["profile"].as_str().unwrap().len(), 64);
    let base = git(&wt, &["rev-parse", "refs/remotes/origin/main"], &[]);
    assert_eq!(
        p["profile_base"],
        json!({"ref": "origin/main", "commit": base})
    );
    let exits: Vec<_> = p["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["exit"])
        .collect();
    assert_eq!(exits, [&json!(0), &json!(0)]);
    let output = Evidence::beside(&db)
        .get(p["evidence_ref"].as_str().unwrap())
        .unwrap();
    let listing = "$ git status --porcelain --untracked-files=normal --ignored\n!! built\n$ [";
    assert!(output.starts_with(listing), "{output}");
    let after = (&p["dirty"], &p["head_moved"], &p["status_cut"]);
    assert_eq!(after, (&json!(false), &json!(false), &json!(false)));
    assert!(
        !dir.path()
            .join("checkouts")
            .join(format!("lane-{id}"))
            .exists()
    );

    let replayed = run(&db, &tests).1;
    assert_eq!(
        (&replayed["replayed"], &replayed["event_id"]),
        (&json!(true), &passed["event_id"])
    );
    assert_eq!(marks().lines().count(), 4);
    assert_eq!(lane_repo(&wt), before);
}

/// The lane repository's hooks and replace refs never reach the checkout,
/// nor does config the caller's environment passes git: the commit's own
/// tree is tested, and run from a relative `--db`.
#[test]
fn tests_ignore_the_lane_repos_hooks_and_replace_refs() {
    let dir = tempfile::tempdir().unwrap();
    let (_, wt, id) = tests_lane(dir.path());
    base_tests(
        &wt,
        Some(
            r#"commands = [["sh", "-c", "test ! -e hooked && test -z \"$GIT_CONFIG_COUNT$GIT_CONFIG_PARAMETERS\" && grep -qx c lib.rs"]]"#,
        ),
    );
    let hook = wt.join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\ntouch hooked\n").unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let (feat, main) = (
        git(&wt, &["rev-parse", "feat"], &[]),
        git(&wt, &["rev-parse", "main~1"], &[]),
    );
    git(&wt, &["replace", &feat, &main], &[]);
    let before = lane_repo(&wt);
    let hooks = wt.join(".git/hooks");
    let mut cmd = Command::new(BIN);
    cmd.env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", &hooks)
        .env(
            "GIT_CONFIG_PARAMETERS",
            format!("'core.hookspath'='{}'", hooks.display()),
        );
    cmd.current_dir(dir.path())
        .args(["--db", "ledger.db", "check", "run", "tests", "--work", &id]);
    let out = cmd
        .args(["--agent", "a", "--session", "s"])
        .output()
        .unwrap();
    let data: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let p = &data["data"]["payload"];
    assert_eq!(
        (&p["result"], &p["head_sha"]),
        (&json!("pass"), &json!(feat))
    );
    let tree = git(
        &wt,
        &["--no-replace-objects", "rev-parse", "feat^{tree}"],
        &[],
    );
    assert_eq!(p["tree"], tree);
    assert_eq!(lane_repo(&wt), before);
}

/// The base's profile is the one run, not the lane's; a tree the commands
/// dirty, a run past its time and a command killed by a signal fail; a
/// profile with no tests or a bad `[tests]`, a command that cannot start,
/// and a run already on the lane, write nothing.
#[test]
fn tests_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    let tests = ["check", "run", "tests", "--work", &id];
    let payload = |profile: &str| {
        base_tests(&wt, Some(profile));
        let (code, out) = run(&db, &tests);
        assert_eq!(code, 0, "{out}");
        out["payload"].clone()
    };
    assert_eq!(payload(r#"commands = [["false"]]"#)["result"], "fail");
    let p = payload(r#"commands = [["sh", "-c", "echo 'nice: command failed' >&2; exit 127"]]"#);
    assert_eq!(
        (&p["result"], &p["commands"][0]["exit"]),
        (&json!("fail"), &json!(127))
    );
    let p = payload(r#"commands = [["sh", "-c", "echo x > new.txt"]]"#);
    assert_eq!((&p["result"], &p["dirty"]), (&json!("fail"), &json!(true)));
    for (profile, timed_out) in [
        (
            "commands = [[\"sleep\", \"10\"], [\"true\"]]\ntimeout_secs = 3",
            true,
        ),
        (
            r#"commands = [["sh", "-c", "kill -9 $$"], ["true"]]"#,
            false,
        ),
    ] {
        let p = payload(profile);
        let got = (&p["result"], &p["timed_out"], &p["commands"][0]["exit"]);
        assert_eq!(
            got,
            (&json!("fail"), &json!(timed_out), &Value::Null),
            "{profile}"
        );
        assert_eq!(p["commands"].as_array().unwrap().len(), 1);
    }

    git(
        &wt,
        &["add", "."],
        &[("noexec.sh", b"true\n"), ("bad.sh", b"#!/no/interp\n")],
    );
    use std::os::unix::fs::PermissionsExt;
    let exec = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(wt.join("bad.sh"), exec).unwrap();
    git(&wt, &["add", "bad.sh"], &[]);
    git(&wt, &["commit", "-qm", "scripts"], &[]);
    for (profile, why) in [
        (None, "no .chaperone/profile.toml on the base"),
        (Some("commands = []"), "no [tests] commands"),
        (
            Some("commands = [[\"true\"]]\nshell = true"),
            "unknown field `shell`",
        ),
        (
            Some("commands = [[\"true\"]]\ntimeout_secs = 0"),
            "timeout_secs must be 1 to 86400",
        ),
        (
            Some("commands = [[\"true\"]]\ntimeout_secs = 9223372036854775807"),
            "timeout_secs must be",
        ),
        (
            Some("commands = [[\"true\"]]\ntarget_dir = \"PATH\""),
            "target_dir must be a variable name",
        ),
        (
            Some("commands = [[\"true\"]]\ntarget_dir = \"t\""),
            "target_dir must be a variable name",
        ),
        (
            Some("commands = [[\"/no/such/command\"]]"),
            "could not start",
        ),
        (
            Some("commands = [[\"./noexec.sh\"]]"),
            "could not start [\"./noexec.sh\"]: nice: ./noexec.sh: Permission denied",
        ),
        (
            Some("commands = [[\"./bad.sh\"]]"),
            "could not start [\"./bad.sh\"]",
        ),
    ] {
        base_tests(&wt, profile);
        let before = events(&db);
        let (code, err) = run(&db, &tests);
        let got = (code, &err["code"], events(&db));
        assert_eq!(got, (1, &json!("invalid_input"), before), "{profile:?}");
        assert!(err["message"].as_str().unwrap().contains(why), "{err}");
    }

    base_tests(&wt, Some(r#"commands = [["true"]]"#));
    let held = lock_lane(&db, id.parse().unwrap()).unwrap().unwrap();
    let before = events(&db);
    let (code, err) = run(&db, &tests);
    assert_eq!(
        (code, &err["code"], events(&db)),
        (4, &json!("conflict"), before)
    );
    drop(held);
    assert_eq!(payload(r#"commands = [["true"]]"#)["result"], "pass");
}

/// Nothing a run's commands write into the harness's repository reaches a
/// later run: a first run turns on a sparse checkout that would leave out
/// the file the second commit's test needs, and the second still fails.
#[test]
fn tests_start_from_a_fresh_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    let sparse =
        "git config core.sparseCheckout true && printf 'lib.rs\\\\n' > .git/info/sparse-checkout";
    let command = format!(r#"if [ -e strict ]; then false; else {sparse}; fi"#);
    base_tests(
        &wt,
        Some(&format!(r#"commands = [["sh", "-c", "{command}"]]"#)),
    );
    let tests = ["check", "run", "tests", "--work", &id];
    let checkout = dir.path().join("checkouts").join(format!("lane-{id}"));
    assert_eq!(run(&db, &tests).1["payload"]["result"], "pass");
    assert!(!checkout.exists());
    git(&wt, &["add", "."], &[("strict", b"x\n")]);
    git(&wt, &["commit", "-qm", "strict"], &[]);
    assert_eq!(run(&db, &tests).1["payload"]["result"], "fail");
    assert!(!checkout.exists());
}

/// After commands that passed, the checkout must still be at the commit
/// and its listing whole: a test that commits fails, an ignored directory
/// is one entry however many files it holds, and a listing past its cap
/// fails.
#[test]
fn tests_check_the_checkout_after() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    let tests = ["check", "run", "tests", "--work", &id];
    let payload = |profile: &str| {
        base_tests(&wt, Some(profile));
        let (code, out) = run(&db, &tests);
        assert_eq!(code, 0, "{out}");
        out["payload"].clone()
    };
    let commit = r#"git -c user.name=t -c user.email=t@t commit -q --allow-empty -m own"#;
    let p = payload(&format!(r#"commands = [["sh", "-c", "{commit}"]]"#));
    let got = (&p["result"], &p["head_moved"], &p["dirty"]);
    assert_eq!(got, (&json!("fail"), &json!(true), &json!(false)));
    let many = r#"mkdir built && cd built && seq 1 5000 | xargs touch"#;
    let p = payload(&format!(r#"commands = [["sh", "-c", "{many}"]]"#));
    let got = (&p["result"], &p["ignored_files"], &p["status_cut"]);
    assert_eq!(got, (&json!("pass"), &json!(1), &json!(false)));
    let long = r#"seq -f %0240g.o 1 4600 | xargs touch"#;
    let p = payload(&format!(r#"commands = [["sh", "-c", "{long}"]]"#));
    assert_eq!(
        (&p["result"], &p["status_cut"]),
        (&json!("fail"), &json!(true))
    );
}

/// A lane on v17 to v19 has no review or verdict checks in its pipeline:
/// each is refused, says what such a lane records with, and writes
/// nothing.
#[test]
fn a_review_or_verdict_needs_a_pipeline_that_names_one() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
    let id = lane(&db, &["--worktree", wt.to_str().unwrap()]);
    for (kind, said) in [
        (
            "review",
            "pipeline v19 has no check review_correctness; a lane on it records reviews \
             with `check record` and `reviews record`",
        ),
        (
            "verdict",
            "pipeline v19 has no check verdict; a lane on it records verdicts with `check record`",
        ),
    ] {
        let before = events(&db);
        let (code, error) = run(&db, &["check", "run", kind, "--work", &id]);
        assert_eq!((code, &error["code"]), (1, &json!("invalid_input")));
        assert_eq!(error["message"], said);
        assert_eq!(events(&db), before);
    }
}

/// `--file` goes with plan only, and is required there. A lane whose
/// pipeline has no plan check is refused before anything is read, even
/// with code in its commit, so it is not pointed at a waiver it cannot
/// have; the waiver is refused there too. Each writes nothing. (The plan
/// file and code rules are unit tests of `checks.rs`.)
#[test]
fn a_plan_check_needs_a_plan_file_and_a_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let base = setup(dir.path());
    let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
    git(&wt, &["update-ref", "refs/remotes/origin/main", &base], &[]);
    git(&wt, &["checkout", "-qb", "plan"], &[("lib.rs", b"a\nc\n")]);
    git(&wt, &["add", "."], &[]);
    git(&wt, &["commit", "-qm", "code"], &[]);
    let id = lane(
        &db,
        &["--worktree", wt.to_str().unwrap(), "--branch", "plan"],
    );
    let good = dir.path().join("plan.md");
    std::fs::write(&good, b"1. change lib.rs\n").unwrap();
    let good = good.to_str().unwrap();
    let refused = |args: &[&str], why: &str| {
        let before = events(&db);
        let (code, err) = run(&db, args);
        assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
        assert_eq!(err["message"], why);
        assert_eq!(events(&db), before);
    };
    let file = "--file <plan> goes with plan, and is required there";
    refused(&["check", "run", "plan", "--work", &id], file);
    refused(
        &["check", "run", "diff", "--work", &id, "--file", good],
        file,
    );
    let v19 =
        "pipeline v19 has no check plan; a lane on it records plan checks with `check record`";
    refused(
        &["check", "run", "plan", "--work", &id, "--file", good],
        v19,
    );
    let waive = [
        "check",
        "waive",
        "plan",
        "--work",
        &id,
        "--reason",
        "coded first",
    ];
    let v19 = "pipeline v19 has no check plan; a lane on it records a skipped plan check with \
        `check record`";
    refused(&waive, v19);
}

/// A smoke's evidence files are the claim of the session that records it,
/// stored by hash: a text file in the worktree is kept, by its path there.
/// One outside it, reached through a link that leaves it, under `.git` or
/// `.chaperone`, a FIFO, not text or not there, files on another check,
/// and a record from a session that held the lane, are refused: no event,
/// and no file in the evidence store.
#[test]
fn smoke_evidence_stays_in_the_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let base = setup(dir.path());
    let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
    let id = lane(&db, &["--worktree", wt.to_str().unwrap()]);
    let outside = dir.path().join("outside.log");
    for (path, bytes) in [
        (wt.join("out/smoke.log"), &b"ran: ok\n"[..]),
        (outside.clone(), b"secret\n"),
        (wt.join(".chaperone/x.txt"), b"x\n"),
        (wt.join("bin.dat"), &[0, 1]),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    std::os::unix::fs::symlink(&outside, wt.join("link.log")).unwrap();
    let fifo = Command::new("mkfifo").arg(wt.join("pipe.log")).status();
    assert!(fifo.unwrap().success());
    let in_store = || {
        let mut find = Command::new("find");
        let files = find.arg(dir.path().join("evidence")).args(["-type", "f"]);
        let out = files.output().unwrap().stdout;
        String::from_utf8(out).unwrap().lines().count()
    };
    // `fresh` never held the lane, as `check record` needs; `s` did.
    let record_as = |session: &str, check: &[&str], file: &str| {
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args([
                "check",
                "record",
                "--work",
                &id,
                "--head",
                &base,
                "--outcome",
                "ran",
            ])
            .args(check)
            .args(["--evidence", file, "--agent", "a", "--session", session])
            .output()
            .unwrap();
        let ok = out.status.success();
        let v: Value = serde_json::from_slice(if ok { &out.stdout } else { &out.stderr }).unwrap();
        (ok, v[if ok { "data" } else { "error" }].clone())
    };
    let record = |check: &[&str], file: &str| record_as("fresh", check, file);
    let smoke = ["--check", "smoke"];
    let (ok, kept) = record(&smoke, "out/smoke.log");
    assert!(ok, "{kept}");
    let p = &kept["payload"];
    let file = &p["evidence_files"][0];
    let got = (&file["path"], &file["bytes"], &p["producer"]["kind"]);
    assert_eq!(got, (&json!("out/smoke.log"), &json!(8), &json!("session")));
    let stored = Evidence::beside(&db).get(file["sha256"].as_str().unwrap());
    assert_eq!(stored.unwrap(), "ran: ok\n");

    let plan = ["--check", "plan_check", "--findings", "0"];
    for (check, file, why) in [
        (
            &smoke[..],
            outside.to_str().unwrap(),
            "outside the lane's worktree",
        ),
        (&smoke, "link.log", "outside the lane's worktree"),
        (&smoke, ".git/config", "under .git or .chaperone"),
        (&smoke, ".chaperone/x.txt", "under .git or .chaperone"),
        (&smoke, "bin.dat", "not UTF-8 text"),
        (&smoke, "gone.log", "No such file"),
        (&smoke, "pipe.log", "not a regular file"),
        (&plan, "out/smoke.log", "--evidence goes with smoke only"),
    ] {
        let before = (events(&db), in_store());
        let (ok, err) = record(check, file);
        assert!(!ok && err["code"] == "invalid_input", "{file}: {err}");
        assert!(err["message"].as_str().unwrap().contains(why), "{err}");
        assert_eq!((events(&db), in_store()), before);
    }
    std::fs::write(wt.join("out/new.log"), "new\n").unwrap();
    let before = (events(&db), in_store());
    let (ok, err) = record_as("s", &smoke, "out/new.log");
    assert!(
        !ok && err["message"].as_str().unwrap().contains("has held lane"),
        "{err}"
    );
    assert_eq!((events(&db), in_store()), before);
}

/// A smoke claim on a v20 lane runs its command: `--run` keeps the script,
/// the output and the exit code, a failing run's too. The script alone as `--evidence` is refused, writing nothing.
#[test]
fn a_v20_smoke_claim_runs_its_command() {
    let dir = tempfile::tempdir().unwrap();
    let base = setup(dir.path());
    let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
    let id = lane(
        &db,
        &["--pipeline", "v20", "--worktree", wt.to_str().unwrap()],
    );
    std::fs::write(wt.join("smoke.sh"), "echo hello from smoke\n").unwrap();
    std::fs::write(wt.join("bad.sh"), "echo boom\nexit 3\n").unwrap();
    // Named like a bash option, it still runs as a script.
    std::fs::write(wt.join("-s"), "echo boom\nexit 3\n").unwrap();
    // Output past the 16 KiB a stream keeps, and a secret in the script.
    let long = "printf '%20000s' x\n# ghp_0123456789abcdefghijklmnopqrstuvwxyzAB\n";
    std::fs::write(wt.join("long.sh"), long).unwrap();
    let claim = |how: &str, file: &str| {
        let args = ["check", "record", "--work", &id, "--check", "smoke"];
        let rest = ["--head", &base, "--outcome", "ran", how, file];
        run(&db, &[&args[..], &rest].concat())
    };
    let before = events(&db);
    let (code, err) = claim("--evidence", "smoke.sh");
    assert_eq!(
        (code, &err["code"], events(&db)),
        (1, &json!("invalid_input"), before)
    );
    assert!(
        err["message"].as_str().unwrap().contains("--run <script>"),
        "{err}"
    );

    for (script, exit) in [("smoke.sh", 0), ("bad.sh", 3), ("./-s", 3)] {
        let (code, kept) = claim("--run", script);
        assert_eq!(code, 0, "{kept}");
        let p = &kept["payload"];
        let path = script.trim_start_matches("./");
        let argv = json!({"argv": ["bash", "--", path], "exit": exit});
        assert_eq!(p["run"], argv);
        let file = &p["evidence_files"][0];
        let stored = Evidence::beside(&db).get(file["sha256"].as_str().unwrap());
        let stored = stored.unwrap();
        let printed = if exit == 0 {
            "hello from smoke"
        } else {
            "boom"
        };
        assert!(stored.contains(printed), "{stored}");
        assert!(stored.ends_with(&format!("exit {exit}\n")), "{stored}");
    }
    // The same run again, with the same output, replays and writes nothing.
    let before = events(&db);
    let (code, again) = claim("--run", "./-s");
    assert_eq!(
        (code, &again["replayed"], events(&db)),
        (0, &json!(true), before)
    );
    // A skipped smoke ran nothing: no run goes with it, and none starts.
    let before = events(&db);
    let skipped = [
        "check",
        "record",
        "--work",
        &id,
        "--check",
        "smoke",
        "--head",
        &base,
        "--outcome",
        "skipped",
        "--reason",
        "nothing_runs",
        "--run",
        "smoke.sh",
    ];
    let (code, err) = run(&db, &skipped);
    assert_eq!(
        (code, &err["code"], events(&db)),
        (1, &json!("invalid_input"), before)
    );
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("--run goes with a smoke that ran"),
        "{err}"
    );
    let (code, kept) = claim("--run", "long.sh");
    assert_eq!(code, 0, "{kept}");
    let file = &kept["payload"]["evidence_files"][0];
    assert_eq!(
        (&file["truncated"], &file["redactions"]),
        (&json!(true), &json!(1))
    );
}

/// A harness `smoke` passes v19's `smoke_recorded` whoever starts it, so
/// `check run smoke` on a lane whose pipeline has no `verdict` (v17 to
/// v19) is refused, even with a `[smoke]` in the base profile, and writes
/// nothing; such a lane records smoke with `check record` from a session
/// that never held it.
#[test]
fn smoke_runs_need_a_harness_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    base_tests(
        &wt,
        Some("commands = [[\"true\"]]\n[smoke]\ncommands = [[\"true\"]]"),
    );
    let before = events(&db);
    let (code, err) = run(&db, &["check", "run", "smoke", "--work", &id]);
    assert_eq!(
        (code, &err["code"], events(&db)),
        (1, &json!("invalid_input"), before)
    );
    let v19 = "pipeline v19 has no check verdict, so a harness smoke would stand in for one a \
        session that never held the lane records; a lane on it records smoke with `check record`";
    assert_eq!(err["message"], v19);
}

/// A run first removes the build directories and leftover checkouts of
/// lanes that are done, cancelled or not in the ledger; a live lane's, the
/// running one's included, stay.
#[test]
fn a_run_sweeps_finished_lanes_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    base_tests(&wt, None);
    let create = || run(&db, &["work", "create", "--title", "t"]).1["id"].to_string();
    let (live, cancelled, done) = (create(), create(), create());
    assert_eq!(run(&db, &["work", "cancel", &cancelled]).0, 0);
    // No lane here passes the done gate; the sweep reads only the status.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let sql = "UPDATE lanes SET status = 'done' WHERE id = ?1";
    assert_eq!(conn.execute(sql, [&done]).unwrap(), 1);
    let path = |d: &str, id: &str| dir.path().join(d).join(format!("lane-{id}"));
    let dirs = [
        ("targets", id.as_str()),
        ("targets", &live),
        ("targets", &cancelled),
        ("targets", &done),
        ("checkouts", &done),
        ("targets", "99"),
    ];
    for (d, id) in dirs {
        std::fs::create_dir_all(path(d, id).join("deps")).unwrap();
    }
    assert_eq!(run(&db, &["check", "run", "diff", "--work", &id]).0, 0);
    let kept: Vec<_> = dirs.iter().map(|(d, id)| path(d, id).exists()).collect();
    assert_eq!(kept, [true, true, false, false, false, false]);
}

/// `check prune` removes the build directory of a lane in `review` whose
/// PR merged (its agent never handed off done), keeps one whose PR has not
/// merged and one that took another PR since, and skips a merged one whose
/// run lock is held.
#[test]
fn prune_removes_merged_lanes_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledger.db");
    assert_eq!(run(&db, &["migrate"]).0, 0);
    let conn = rusqlite::Connection::open(&db).unwrap();
    let lane = |n: &str, merged: bool, next: Option<&str>| {
        let (_, item) = run(&db, &["work", "create", "--title", n]);
        let (id, at) = (item["id"].to_string(), item["updated_at"].as_str().unwrap());
        let pr = format!("o/r#{n}");
        let update = [
            "work",
            "update",
            &id,
            "--pr",
            &pr,
            "--expected-updated-at",
            at,
        ];
        assert_eq!(run(&db, &update).0, 0);
        let sql = "UPDATE lanes SET status = 'review' WHERE id = ?1";
        assert_eq!(conn.execute(sql, [&id]).unwrap(), 1);
        if merged {
            let at = "2026-11-17T00:00:00.000Z";
            let args = [
                "--merge-sha",
                "abcdef1",
                "--head",
                "1234567",
                "--merged-at",
                at,
            ];
            let (code, out) = run(
                &db,
                &[&["work", "merged", &id, "--pr", &pr], &args[..]].concat(),
            );
            assert_eq!(code, 0, "{out}");
        }
        if let Some(next) = next {
            let sql = "SELECT updated_at FROM lanes WHERE id = ?1";
            let at: String = conn.query_row(sql, [&id], |r| r.get(0)).unwrap();
            let update = [
                "work",
                "update",
                &id,
                "--pr",
                next,
                "--expected-updated-at",
                &at,
            ];
            assert_eq!(run(&db, &update).0, 0);
        }
        std::fs::create_dir_all(
            dir.path()
                .join("targets")
                .join(format!("lane-{id}"))
                .join("deps"),
        )
        .unwrap();
        id
    };
    let (merged, open) = (lane("1", true, None), lane("2", false, None));
    let (locked, moved) = (lane("3", true, None), lane("4", true, Some("o/r#5")));
    let _held = lock_lane(&db, locked.parse().unwrap()).unwrap().unwrap();
    let out = Command::new(BIN)
        .arg("--db")
        .arg(&db)
        .args(["check", "prune"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let data = serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone();
    let path = |id: &str| dir.path().join("targets").join(format!("lane-{id}"));
    let why = format!("lane {locked} has a check run in progress");
    let shown = |id: &str| path(id).display().to_string();
    assert_eq!(
        data,
        json!({"removed": [shown(&merged)], "skipped": [{"path": shown(&locked), "why": why}]})
    );
    let kept = [&merged, &open, &locked, &moved].map(|id| path(id).exists());
    assert_eq!(kept, [false, true, true, true]);
}

/// `check prune` removes only directories named `lane-<digits>`, never
/// through a symlink named like one, and skips a lane whose run lock is
/// held, saying so.
#[test]
fn prune_removes_only_lane_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledger.db");
    assert_eq!(run(&db, &["migrate"]).0, 0);
    let targets = dir.path().join("targets");
    for name in ["lane-8", "lane-9", "lane-", "lane-+1", "lane-1x", "other"] {
        std::fs::create_dir_all(targets.join(name)).unwrap();
    }
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), "x").unwrap();
    std::os::unix::fs::symlink(&outside, targets.join("lane-7")).unwrap();
    let _held = lock_lane(&db, 9).unwrap().unwrap();
    let out = Command::new(BIN)
        .arg("--db")
        .arg(&db)
        .args(["check", "prune"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let data = serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone();
    let path = |name: &str| targets.join(name).display().to_string();
    let why = "lane 9 has a check run in progress";
    assert_eq!(
        data,
        json!({"removed": [path("lane-8")], "skipped": [{"path": path("lane-9"), "why": why}]})
    );
    let mut left: Vec<_> = std::fs::read_dir(&targets)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        ["lane-", "lane-+1", "lane-1x", "lane-7", "lane-9", "other"]
    );
    assert!(outside.join("keep").exists());
}
/// `check run tests` on lane `id` on a machine with one test slot.
fn tests_on_one_slot(db: &Path, id: &str) -> std::process::Child {
    tests_on_slots(db, id, "1")
}

/// `check run tests` on lane `id` with `CHAPERONE_SLOTS` set to `slots`.
fn tests_on_slots(db: &Path, id: &str, slots: &str) -> std::process::Child {
    let mut cmd = Command::new(BIN);
    cmd.arg("--db")
        .arg(db)
        .args(["check", "run", "tests", "--work", id]);
    cmd.args(["--agent", "a", "--session", "s"])
        .env("CHAPERONE_SLOTS", slots);
    let piped = std::process::Stdio::piped;
    cmd.stdout(piped()).stderr(piped()).spawn().unwrap()
}

/// The line a run writes to stderr once it waits for a slot.
fn waits(child: &mut std::process::Child) -> String {
    let mut line = String::new();
    let stderr = child.stderr.take().unwrap();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(stderr), &mut line).unwrap();
    line
}

/// The run's `data`, and its stderr when not read already.
fn finished(child: std::process::Child) -> (Value, String) {
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    (v["data"].clone(), String::from_utf8(out.stderr).unwrap())
}

/// A run waits while the slot is held, says for whom, then runs and
/// records the wait; a replay takes no slot.
#[test]
fn runs_share_a_slot_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, id) = tests_lane(dir.path());
    base_tests(&wt, Some(r#"commands = [["true"]]"#));
    for slots in ["0", "x"] {
        let before = events(&db);
        let out = tests_on_slots(&db, &id, slots).wait_with_output().unwrap();
        let err: Value = serde_json::from_slice(&out.stderr).unwrap();
        let got = (out.status.code(), &err["error"]["code"], events(&db));
        assert_eq!(got, (Some(1), &json!("invalid_input"), before), "{slots}");
        let why = format!("CHAPERONE_SLOTS must be a whole number of 1 or more, not \"{slots}\"");
        assert_eq!(err["error"]["message"], why);
    }
    let held = lock_slot(&db, 1, 99, |_| {}).unwrap();
    let mut waiting = tests_on_one_slot(&db, &id);
    let line = format!("chaperone: lane {id} waits for a test slot (1 held, by lanes 99)\n");
    assert_eq!(waits(&mut waiting), line);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        waiting.try_wait().unwrap().is_none(),
        "ran past a held slot"
    );
    drop(held);
    let (data, _) = finished(waiting);
    let slot = &data["payload"]["slot"];
    assert_eq!(data["payload"]["result"], "pass");
    assert_eq!(slot["slots"], 1);
    assert!(slot["wait_ms"].as_u64().unwrap() >= 300, "{slot}");

    let _held = lock_slot(&db, 1, 99, |_| {}).unwrap();
    let (again, err) = finished(tests_on_one_slot(&db, &id));
    assert_eq!((&again["replayed"], err.as_str()), (&json!(true), ""));
}

/// A holder killed mid-run frees its slot for the lane waiting on it.
#[test]
fn a_killed_holder_frees_the_slot() {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, first) = tests_lane(dir.path());
    let hold = r#"commands = [["sh", "-c", "if [ -e hold ]; then sleep 20; fi"]]"#;
    base_tests(&wt, Some(hold));
    let wt2 = dir.path().join("wt2");
    git(
        dir.path(),
        &["clone", "-q", wt.to_str().unwrap(), "wt2"],
        &[],
    );
    git(
        &wt2,
        &["update-ref", "refs/remotes/origin/main", "origin/main"],
        &[],
    );
    let second = lane(
        &db,
        &["--worktree", wt2.to_str().unwrap(), "--branch", "feat"],
    );
    git(&wt, &["add", "."], &[("hold", b"x\n")]);
    git(&wt, &["commit", "-qm", "hold"], &[]);

    let mut holder = tests_on_one_slot(&db, &first);
    let slot = dir.path().join("ledger.db.slot-0.lock");
    let started = std::time::Instant::now();
    while std::fs::read_to_string(&slot).unwrap_or_default() != first {
        assert!(started.elapsed() < Duration::from_secs(60), "never held");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut waiting = tests_on_one_slot(&db, &second);
    assert!(waits(&mut waiting).contains(&format!("by lanes {first})")));
    holder.kill().unwrap();
    holder.wait().unwrap();
    let (data, _) = finished(waiting);
    let waited = data["payload"]["slot"]["wait_ms"].as_u64().unwrap();
    assert_eq!(data["payload"]["result"], "pass");
    assert!(waited < 15_000, "waited {waited} ms, for the sleep");
}
