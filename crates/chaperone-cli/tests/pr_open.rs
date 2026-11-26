//! `pr open` through the binary on a v20 lane whose `origin` is a local
//! bare repo, with a fake `gh` first on `PATH` that logs its argv and keeps
//! the body it was given. Its `pr create` opens #7 and then fails, as a run
//! that opened a PR and never recorded it; `pr list` lists #7 once opened.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use chaperone_core::ledger::{Actor, EventKind, Ledger, Mutation};
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
/// Its plan check is waived by the owner; `create` adds to `work create`,
/// `pr` to the base profile. The fake's `gh api user` is `me`, and its
/// `gh pr view` title the file `title`.
fn lane(create: &[&str], pr: &str) -> Lane {
    lane_on("v20", create, pr)
}

/// [`lane`] on `pipeline`.
fn lane_on(pipeline: &str, create: &[&str], pr: &str) -> Lane {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, bin) = (
        dir.path().join("ledger.db"),
        dir.path().join("wt"),
        dir.path().join("bin"),
    );
    std::fs::create_dir_all(wt.join(".chaperone")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let (log, body) = (dir.path().join("gh.log"), dir.path().join("body.md"));
    let (opened, title) = (dir.path().join("opened"), dir.path().join("title"));
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$*\" >> {log:?}\ncase $2 in\n\
             list) cat {opened:?} 2>/dev/null;;\n\
             create) cat > {body:?}; echo https://github.com/o/r/pull/7 > {opened:?}; exit 1;;\n\
             edit) cat > {body:?};;\nuser) echo me;;\nview) cat {title:?};;\nesac\nexit 0\n"
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
    let profile = format!(
        "[tests]\ncommands = [[\"test\", \"!\", \"-e\", \"FAIL\"]]\n[assets]\nglobs = [\"*.png\"]\n{pr}"
    );
    let profile = (".chaperone/profile.toml", profile.as_bytes());
    let files: &[(&str, &[u8])] = &[("lib.rs", b"a\n"), profile];
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
    assert_eq!(lane.run("lane", &claim).0, 0);
    common::restamp(&lane.db, 1, pipeline);
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
    let lane = lane(&[], "");
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
        "t\n\n- Size: 1 code lines added, 0 removed, and 0 test lines in 1 files\n- Tests: passed on {head}: `test ! -e FAIL`\n\
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
/// or gh call; the owner's size waiver lifts only the line limit.
#[test]
fn pr_open_refuses_what_cannot_pass() {
    let lane = lane(&[], "");
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
    let waive = [
        "check", "waive", "size", "--work", "1", "--lines", "500", "--reason", "r",
    ];
    let (code, err) = lane.run("lane", &waive);
    assert_eq!(code, 1, "{err}");
    assert!(
        err["message"].as_str().unwrap().contains("has held lane 1"),
        "{err}"
    );
    let no_lines = ["check", "waive", "size", "--work", "1", "--reason", "r"];
    let plan_lines = [
        "check", "waive", "plan", "--work", "1", "--lines", "5", "--reason", "r",
    ];
    let two_lines = [&waive[..8], &["a\nb"]].concat();
    for (args, says) in [
        (&no_lines[..], "needs --lines"),
        (&plan_lines[..], "--lines goes with size only"),
        (&two_lines[..], "one non-blank line"),
    ] {
        let message = lane.run("owner", args).1["message"].to_string();
        assert!(message.contains(says), "{message}");
    }
    assert_eq!(lane.run("owner", &waive).0, 0);
    refused("the tests failed");
    git(&lane.wt, &["rm", "-q", "big.rs"], &[]);
    git(&lane.wt, &["add", "blob"], &[("blob", b"\0\x01")]);
    git(&lane.wt, &["commit", "-qm", "blob"], &[]);
    refused("1 uncounted (binary) files");
}

/// On v21 the size counts added code lines and is the review's question,
/// never a refusal: a cleanup removing 400 lines opens its PR, and so does a
/// diff past the cap with an asset, which its Size line counts; a binary no
/// `[assets]` glob names still refuses.
#[test]
fn pr_open_on_v21_counts_added_lines_and_refuses_no_size() {
    let lane = lane_on("v21", &[], "");
    let old = "x\n".repeat(400);
    git(
        &lane.wt,
        &["checkout", "-q", "main"],
        &[("old.rs", old.as_bytes())],
    );
    git(&lane.wt, &["add", "old.rs"], &[]);
    git(&lane.wt, &["commit", "-qm", "old"], &[]);
    git(&lane.wt, &["push", "-q", "origin", "main"], &[]);
    git(&lane.wt, &["checkout", "-q", "feat"], &[]);
    git(&lane.wt, &["merge", "-q", "--no-edit", "main"], &[]);
    git(&lane.wt, &["rm", "-q", "old.rs"], &[]);
    let new = "y\n".repeat(50);
    git(&lane.wt, &["add", "new.rs"], &[("new.rs", new.as_bytes())]);
    git(&lane.wt, &["commit", "-qm", "cleanup"], &[]);
    let open = ["pr", "open", "--work", "1"];
    let (code, err) = lane.run("lane", &open);
    // The fake's first `pr create` opens #7 and fails.
    assert_eq!((code, &err["code"]), (1, &json!("internal")), "{err}");
    let size = "- Size: 51 code lines added, 400 removed, and 0 test lines in 3 files\n\
                - Tests: left to CI\n";
    assert!(
        lane.file("body.md").contains(size),
        "{}",
        lane.file("body.md")
    );

    let big = "z\n".repeat(401);
    let files: &[(&str, &[u8])] = &[("big.rs", big.as_bytes()), ("logo.png", b"\0\x01")];
    git(&lane.wt, &["add", "big.rs", "logo.png"], files);
    git(&lane.wt, &["commit", "-qm", "big"], &[]);
    let (code, out) = lane.run("lane", &open);
    assert_eq!(code, 0, "{out}");
    let steps = out["steps"].as_array().unwrap();
    assert!(steps.iter().all(|s| s["check"] == "diff"), "{out}");
    let size = "- Size: 452 code lines added, 400 removed, and 0 test lines in 5 files, \
                1 of them assets\n";
    assert!(
        lane.file("body.md").contains(size),
        "{}",
        lane.file("body.md")
    );
    git(&lane.wt, &["add", "blob"], &[("blob", b"\0\x01")]);
    git(&lane.wt, &["commit", "-qm", "blob"], &[]);
    let (code, err) = lane.run("lane", &open);
    assert_eq!(code, 1, "{err}");
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("1 uncounted (binary) files"),
        "{err}"
    );
}

/// Two complete harness reviews of lane 1: the first on an older commit
/// raises a blocking risk and a nit, the second on `head` an improvement
/// and the question an unproven risk is written as. The body lists the
/// second's as open; the first round's nit never settles, so it is not.
#[test]
fn pr_open_lists_the_findings_that_do_not_block() {
    let lane = lane_on("v21", &[], "");
    let head = git(&lane.wt, &["rev-parse", "HEAD"], &[]);
    let ledger = Ledger::open(&lane.db).unwrap();
    let harness = json!({"kind": "harness", "bin": "chaperone", "version": "0"});
    let rounds = [
        (
            "abc1234",
            json!([["risk", "r", "lib.rs", 1], ["nit", "n", null, null]]),
        ),
        (
            &*head,
            json!([
                ["improvement", "i", "lib.rs", 2],
                ["question", "q", null, null]
            ]),
        ),
    ];
    for (sha, found) in rounds {
        let found = found.as_array().unwrap().iter().map(|f| {
            json!({"finding_key": f[1], "severity": f[0], "title": f[1], "body": "b",
                   "path": f[2], "line": f[3]})
        });
        let p = json!({"check": "review", "outcome": "ran", "head_sha": sha, "complete": true,
                       "producer": harness, "findings": found.collect::<Vec<_>>()});
        let m = Mutation::Event {
            kind: EventKind::CheckFinished,
            lane_id: Some(1),
            spec_id: None,
            payload: serde_json::from_value(p).unwrap(),
        };
        let actor = Actor {
            agent: Some("h".into()),
            session: Some("h".into()),
        };
        ledger
            .apply(&actor, None, "2026-11-27T00:00:00.000Z", m)
            .unwrap();
    }
    assert_eq!(lane.run("lane", &["pr", "open", "--work", "1"]).0, 1);
    let open = "- Smoke: none\n\nOpen findings, not blocking:\n\
                - improvement: i `lib.rs:2`\n- question: q\n\nMade by";
    let body = lane.file("body.md");
    assert!(body.contains(open), "{body}");
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
        let lane = lane(&["--source", "github", "--source-id", issue], "");
        let closes = |line: &str| format!("Tracker: {issue}\n\n{line}\n\n- Size");
        assert_eq!(lane.run("lane", &open).0, 1);
        assert!(lane.file("body.md").contains(&closes(created)));
        assert_eq!(lane.run("lane", &open).0, 0);
        assert!(lane.file("body.md").contains(&closes(edited)));
    }
}

const RULES: &str = "[pr]\nflow = \"pr\"\ntitle = \"{type}({key}): {text}\"\n\
    types = [\"feat\", \"fix\"]\nhead = [\"linear: [{key}](https://l/{key})\"]\n\
    assignee = \"@me\"\nreviewers = [\"ana\", \"Me\"]\n";

/// With the base profile's `[pr]` rules, a missing or off-template title is
/// refused before any gh call or push; the create sets the assignee and the
/// reviewers but the gh login, and so does the first recorded edit of a PR
/// the create left unrecorded. A later edit keeps the PR's own title when it
/// fits and sets neither again; the head line leads every body.
#[test]
fn pr_open_follows_the_base_profiles_pr_rules() {
    let lane = lane(&["--source", "linear", "--source-id", "CORE-12"], RULES);
    let open = ["pr", "open", "--work", "1"];
    let run = |args: &[&str]| lane.run("lane", &[&open[..], args].concat());
    let refused = |args: &[&str], says: &str| {
        let (code, err) = run(args);
        assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
        assert!(err["message"].as_str().unwrap().contains(says), "{err}");
    };
    refused(&[], "passes a --title that fits");
    for off in [
        "chore(CORE-12): Do it",
        "feat(CORE-1): Do it",
        "feat(CORE-12): ",
        "feat(CORE-12):  Do it",
        "feat(CORE-12): Do it ",
    ] {
        refused(&["--title", off], "does not fit");
    }
    assert_eq!(lane.file("gh.log") + &lane.pushed(), "");

    let title = "feat(CORE-12): Do it";
    let (code, err) = run(&["--title", title]);
    assert_eq!((code, &err["code"]), (1, &json!("internal")), "{err}");
    let list = "pr list --head feat --json url --jq .[0].url\n";
    let me = "api user --jq .login\n";
    let create = format!(
        "pr create --base main --head feat --title {title} --assignee @me --reviewer ana \
         --body-file -\n"
    );
    assert_eq!(lane.file("gh.log"), [list, me, &create].concat());
    assert_eq!(run(&["--title", title]).0, 0);
    let recovered = format!(
        "pr edit 7 --repo o/r --title {title} --add-assignee @me --add-reviewer ana \
         --body-file -\n"
    );
    std::fs::write(lane.dir.path().join("title"), format!("{title}\n")).unwrap();
    assert_eq!(run(&[]).0, 0);
    let view = "pr view 7 --repo o/r --json title --jq .title\n";
    let edit = "pr edit 7 --repo o/r --body-file -\n";
    let log = [list, me, &create, list, me, &recovered, view, edit].concat();
    assert_eq!(lane.file("gh.log"), log);
    let led = "linear: [CORE-12](https://l/CORE-12)\n\nt\n\nTracker: CORE-12\n\n- Size";
    assert!(
        lane.file("body.md").starts_with(led),
        "{}",
        lane.file("body.md")
    );

    std::fs::write(lane.dir.path().join("title"), "Do it\n").unwrap();
    refused(&[], "does not fit");
    assert_eq!(lane.file("gh.log"), log + view);
}

/// A `[pr]` rule that cannot be kept refuses before any gh call or push: a
/// title or head on a lane with no tracker key, an unknown key, and a title
/// template with no `{text}`, a second `{type}`, another brace, or `{type}`
/// and no `types`. A template with no `{type}` needs no `types`.
#[test]
fn pr_open_refuses_a_broken_pr_rule() {
    let key = ["--source", "linear", "--source-id", "CORE-12"];
    for (create, pr, says) in [
        (&[][..], "head = [\"x\"]", "need a tracker key"),
        (&key[..], "reviewer = [\"ana\"]", "unknown field"),
        (&key[..], "title = \"{key}\"", "needs one {text}"),
        (
            &key[..],
            "title = \"{type}{type} {text}\"\ntypes = [\"a\"]",
            "needs one {text}",
        ),
        (&key[..], "title = \"{kind} {text}\"", "needs one {text}"),
        (&key[..], "title = \"{type}: {text}\"", "needs one {text}"),
    ] {
        let lane = lane(create, &format!("[pr]\n{pr}\n"));
        let (code, err) = lane.run("lane", &["pr", "open", "--work", "1", "--title", "x"]);
        assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
        assert!(
            err["message"].as_str().unwrap().contains(says),
            "{pr}: {err}"
        );
        assert_eq!(lane.file("gh.log") + &lane.pushed(), "");
    }
    let lane = lane(&key, "[pr]\ntitle = \"{key}: {text}\"\n");
    let (code, err) = lane.run(
        "lane",
        &["pr", "open", "--work", "1", "--title", "CORE-12: Do it"],
    );
    assert_eq!((code, &err["code"]), (1, &json!("internal")), "{err}");
    assert!(
        lane.file("gh.log")
            .contains("--title CORE-12: Do it --body-file -")
    );
}
