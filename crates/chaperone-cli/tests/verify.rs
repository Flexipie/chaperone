//! `verify` through the binary on a v20 lane, with `chaperone-fake` as
//! `claude` and `codex`. The fake is the workspace's binary beside this
//! one, so these tests need the workspace built (`cargo nextest run
//! --workspace` builds it).

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chaperone_core::ledger::{Evidence, lock_lane};
use chaperone_core::review;
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

/// Lane 1 on v20 in a ledger, its worktree, and the fake's directory.
struct Lane {
    dir: tempfile::TempDir,
    db: PathBuf,
    wt: PathBuf,
}

impl Lane {
    /// `chaperone <args>` as session `who` with the fake first on `PATH`:
    /// the exit code, and `data` on success or `error` on a refusal.
    fn run(&self, who: &str, args: &[&str]) -> (i32, Value) {
        let path = std::env::var("PATH").unwrap();
        let fake = self.dir.path().join("bin");
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .args(["--agent", who, "--session", who])
            .env("PATH", format!("{}:{path}", fake.display()))
            // A caller's diff options, which no harness diff may take.
            .env("GIT_DIFF_OPTS", "--unified=0")
            .output()
            .unwrap();
        let ok = out.status.success();
        let v: Value = serde_json::from_slice(if ok { &out.stdout } else { &out.stderr }).unwrap();
        (
            out.status.code().unwrap(),
            v[if ok { "data" } else { "error" }].clone(),
        )
    }

    fn verify(&self) -> Value {
        let (code, out) = self.run("lane", &["verify", "--work", "1"]);
        assert_eq!(code, 0, "{out}");
        out
    }

    /// `finding list --work 1`'s items.
    fn findings(&self) -> Vec<Value> {
        let mut list = Command::new(BIN);
        let list = list.arg("--db").arg(&self.db);
        let out = list.args(["finding", "list", "--work", "1"]).output();
        let out: Value = serde_json::from_slice(&out.unwrap().stdout).unwrap();
        out["data"]["items"].as_array().unwrap().clone()
    }

    /// Session `who` makes `verb` (`accept`, `dispute`, `waive`) on the
    /// plan check's one finding, reason `r`.
    fn answer(&self, who: &str, verb: &str) {
        let plan = &self.findings()[0];
        let (event, key) = (&plan["check_event_id"], plan["finding_key"].as_str());
        let mut args = format!("finding {verb} {event} {} --reason r", key.unwrap());
        if verb == "waive" {
            args += " --kind disproved --ref t.rs:1";
        }
        let args: Vec<_> = args.split(' ').collect();
        assert_eq!(self.run(who, &args).0, 0);
    }

    /// `check run plan` on `text`: the result.
    fn plan(&self, text: &str) -> Value {
        let plan = self.dir.path().join("plan.md");
        std::fs::write(&plan, text).unwrap();
        let args = ["check", "run", "plan", "--work", "1", "--file"];
        let (code, out) = self.run("lane", &[&args[..], &[plan.to_str().unwrap()]].concat());
        assert_eq!(code, 0, "{out}");
        out
    }

    /// The lane's one code commit.
    fn commit(&self) {
        git(
            &self.wt,
            &["commit", "-qam", "feat"],
            &[("lib.rs", b"a\nb\n")],
        );
    }

    fn head(&self) -> String {
        git(&self.wt, &["rev-parse", "HEAD"], &[])
    }

    /// The lane's smoke claim on its commit, its command run.
    fn smoke(&self) {
        std::fs::write(self.wt.join("smoke.sh"), "echo it ran\n").unwrap();
        let claim = format!(
            "check record --work 1 --check smoke --head {} --outcome ran --run smoke.sh",
            self.head()
        );
        let claim: Vec<_> = claim.split(' ').collect();
        assert_eq!(self.run("lane", &claim).0, 0);
    }

    /// A ready handoff of the lane's commit.
    fn handoff(&self) -> (i32, Value) {
        let file = self.dir.path().join("handoff.json");
        let json =
            json!({"summary": "s", "status": "review", "ready": true, "commit": self.head()});
        std::fs::write(&file, json.to_string()).unwrap();
        self.run("lane", &["work", "handoff", "1", file.to_str().unwrap()])
    }

    /// Sessions the fake has started, both CLIs.
    fn sessions(&self) -> usize {
        let log =
            |cli: &str| std::fs::read_to_string(self.dir.path().join(format!("bin/{cli}.log")));
        ["claude", "codex"]
            .map(|cli| log(cli).unwrap_or_default().lines().count())
            .iter()
            .sum()
    }

    /// The ledger's events that match `filter`.
    fn events(&self, filter: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        let sql = format!("SELECT count(*) FROM events WHERE {filter}");
        conn.query_row(&sql, [], |r| r.get(0)).unwrap()
    }

    /// The newest review's payload.
    fn review(&self) -> Value {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        let sql = "SELECT payload_json FROM events WHERE payload_json->>'check' = 'review'
                     ORDER BY id DESC LIMIT 1";
        let p: String = conn.query_row(sql, [], |r| r.get(0)).unwrap();
        serde_json::from_str(&p).unwrap()
    }

    /// The newest review's codex row and whether the review is complete.
    fn codex(&self) -> (Value, Value) {
        let p = self.review();
        let rows = p["lenses"].as_array().unwrap();
        let codex = rows.iter().find(|r| r["lens"] == "codex").unwrap();
        (codex.clone(), p["complete"].clone())
    }
}

/// A Claude session's answer to the prompt that starts with `header`.
fn says(header: &str, answer: Value) -> Value {
    let result = json!({"type": "result", "subtype": "success", "is_error": false,
        "structured_output": answer, "total_cost_usd": 0.01, "session_id": header});
    json!({"when": header, "stdout": result.to_string()})
}

/// Claude's runs where `correctness` is the correctness lens's answer and
/// the verifier rules `rulings`; the plan check raises a risk, every other
/// lens finds nothing and the verdict says yes.
fn claude(correctness: Value, rulings: Value) -> Value {
    lenses(correctness, json!({"findings": []}), rulings)
}

/// [`claude`] with `intent` the intent lens's answer.
fn lenses(correctness: Value, intent: Value, rulings: Value) -> Value {
    let none = json!({"findings": []});
    let yes = json!({"decision": "yes", "why": "w", "proven": ["p"], "not_proven": []});
    let risk = json!({"findings": [{"severity": "risk", "title": "r", "body": "why",
        "path": "lib.rs", "line": null}]});
    json!([
        says("# Plan check", risk),
        says(
            "# Verify the review's findings",
            json!({"rulings": rulings})
        ),
        says("# Merge verdict", yes),
        says("# Review: correctness", correctness),
        says("# Review: design and tests", none.clone()),
        says("# Review: the change against its goal", intent),
    ])
}

/// Lane 1, claimed on v20 by session `lane` on branch `feat`, its plan
/// checked before its one code commit.
fn lane(claude: Value) -> Lane {
    lane_on(claude, "")
}

/// [`lane`] with `rules` added to the base profile.
fn lane_on(claude: Value, rules: &str) -> Lane {
    let lane = claimed_on(claude, rules);
    lane.plan("1. do it\n");
    lane.commit();
    lane
}

/// Lane 1, claimed on v20 by session `lane` on branch `feat`, with no
/// commit of its own; the base profile's tests pass and it declares no
/// smoke. The fake plays `claude` for Claude and finds nothing as Codex.
fn claimed(claude: Value) -> Lane {
    claimed_on(claude, "")
}

/// [`claimed`] with `rules` added to the base profile.
fn claimed_on(claude: Value, rules: &str) -> Lane {
    claimed_in("v20", claude, rules)
}

/// [`claimed_on`] on `pipeline`.
fn claimed_in(pipeline: &str, claude: Value, rules: &str) -> Lane {
    let dir = tempfile::tempdir().unwrap();
    let (db, wt, bin) = (
        dir.path().join("ledger.db"),
        dir.path().join("wt"),
        dir.path().join("bin"),
    );
    std::fs::create_dir(&wt).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let fake = Path::new(BIN).with_file_name("chaperone-fake");
    // Else `claude` on PATH, a real one, would run.
    assert!(fake.is_file(), "no {}: build the workspace", fake.display());
    let codex =
        json!([{"when": "# Review: a second reader", "write": {"-o": r#"{"findings": []}"#}}]);
    for (cli, runs) in [("claude", claude), ("codex", codex)] {
        std::os::unix::fs::symlink(&fake, bin.join(cli)).unwrap();
        let script = json!({"version": "1 (fake)", "runs": runs}).to_string();
        std::fs::write(bin.join(format!("{cli}.json")), script).unwrap();
    }
    let profile =
        format!("[tests]\ncommands = [[\"true\"]]\n[size]\ntests = [\"tests/**\"]\n{rules}");
    let files: &[(&str, &[u8])] = &[
        ("lib.rs", b"a\n"),
        (".chaperone/profile.toml", profile.as_bytes()),
    ];
    git(&wt, &["init", "-q", "-b", "main"], files);
    git(&wt, &["add", "."], &[]);
    git(&wt, &["commit", "-qm", "base"], &[]);
    git(
        &wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    git(&wt, &["checkout", "-qb", "feat"], &[]);
    let migrate = Command::new(BIN)
        .arg("--db")
        .arg(&db)
        .arg("migrate")
        .status();
    assert!(migrate.unwrap().success());
    let lane = Lane { dir, db, wt };
    let claim = format!(
        "work claim 1 --worktree {} --branch feat",
        lane.wt.display()
    );
    let claim: Vec<_> = claim.split(' ').collect();
    assert_eq!(lane.run("lane", &["work", "create", "--title", "t"]).0, 0);
    assert_eq!(lane.run("lane", &claim).0, 0);
    common::restamp(&lane.db, 1, pipeline);
    lane
}

/// Each step as (check, passed, replayed).
fn steps(out: &Value) -> Vec<(String, bool, bool)> {
    let steps = out["steps"].as_array().unwrap().iter();
    steps
        .map(|s| {
            (
                s["check"].as_str().unwrap().into(),
                s["passed"] == true,
                s["replayed"] == true,
            )
        })
        .collect()
}

/// With a smoke claimed, a clean verify runs every step, past the plan
/// check's risk (no review's finding), and the ready handoff passes; the same tree again replays every
/// step and starts no session. A new commit is refused ready until verify
/// runs on it.
#[test]
fn a_lane_is_ready_only_on_a_commit_verify_passed() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    lane.smoke();
    let out = lane.verify();
    let all = |replayed| ["diff", "tests", "review", "verdict"].map(|c| (c.into(), true, replayed));
    assert_eq!(
        (steps(&out), &out["stopped"]),
        (all(false).into(), &Value::Null)
    );
    // The plan, three lenses and the verdict; nothing found, so no verifier,
    // and the one code line skips Codex.
    assert_eq!(lane.sessions(), 5);
    assert_eq!(steps(&lane.verify()), all(true));
    assert_eq!(lane.sessions(), 5);
    let (code, err) = lane.handoff();
    assert_eq!(code, 0, "{err}");

    git(
        &lane.wt,
        &["commit", "-qam", "more"],
        &[("lib.rs", b"a\nc\n")],
    );
    lane.smoke();
    let (code, err) = lane.handoff();
    assert_eq!((code, &err["code"]), (1, &json!("gate_denied")), "{err}");
    assert_eq!(steps(&lane.verify()), all(false));
    assert_eq!(lane.handoff().0, 0);
}

/// v21 has no `tests_passed` gate: verify runs no local tests, and the
/// verdict runs on a commit with a complete review and no `tests` result.
#[test]
fn a_v21_lane_gets_a_verdict_with_no_local_test_run() {
    let lane = claimed_in("v21", claude(json!({"findings": []}), json!([])), "");
    lane.plan("1. do it\n");
    lane.commit();
    lane.answer("lane", "accept");
    lane.smoke();
    let out = lane.verify();
    let passed = ["diff", "review", "verdict"].map(|c| (c.into(), true, false));
    assert_eq!(
        (steps(&out), &out["stopped"]),
        (passed.into(), &Value::Null)
    );
    assert_eq!(lane.events("payload_json->>'check' = 'tests'"), 0);
    assert_eq!(lane.handoff().0, 0);
    // A profile with no reviewers asks gh nothing.
    let log = lane.dir.path().join("gh.log");
    let gh = lane.dir.path().join("bin/gh");
    std::fs::write(&gh, format!("#!/bin/sh\necho \"$*\" >> {log:?}\n")).unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&gh, mode).unwrap();
    let file = lane.dir.path().join("handoff.json");
    let json = json!({"summary": "s", "status": "review", "ready": true,
        "commit": lane.head(), "pull_request": "o/r#7"});
    std::fs::write(&file, json.to_string()).unwrap();
    let (code, out) = lane.run("lane", &["work", "handoff", "1", file.to_str().unwrap()]);
    assert_eq!(
        (code, &out["reviewers"]),
        (0, &json!({"requested": []})),
        "{out}"
    );
    assert!(!log.exists());
    // A base profile that does not load is reported, the handoff kept.
    git(&lane.wt, &["checkout", "-q", "main"], &[]);
    let files: &[(&str, &[u8])] = &[(".chaperone/profile.toml", b"[pr]\nreviewer = 1\n")];
    git(&lane.wt, &["commit", "-qam", "bad"], files);
    git(
        &lane.wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    git(&lane.wt, &["checkout", "-q", "feat"], &[]);
    let (code, out) = lane.run("lane", &["work", "handoff", "1", file.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    let error = out["reviewers"]["error"].as_str().unwrap_or_default();
    assert!(error.contains("unknown field"), "{out}");
    assert!(!log.exists());
}

/// The ready handoff asks the base profile's reviewers on the PR it names
/// (as the v21 `gh pr create` fallback does, as a URL or `owner/repo#N`), but the author and those who
/// reviewed: a failed request still records the handoff and says why, the
/// same handoff again asks them, and once GitHub lists them nobody is asked.
/// A handoff that does not enter ready, or a dry run, asks no one; a team
/// named `o/core` is not asked while GitHub lists it by its slug, and the
/// team `o/me` is asked though the author's login is `me`.
#[test]
fn a_ready_handoff_requests_the_reviewers_once() {
    let rules = "[pr]\nreviewers = [\"ana\", \"bo\", \"Me\", \"o/core\", \"o/me\"]\n";
    let lane = claimed_in("v21", claude(json!({"findings": []}), json!([])), rules);
    let (bin, gh) = (lane.dir.path().join("bin"), lane.dir.path().join("bin/gh"));
    let view = |mut requested: Vec<Value>| {
        requested.push(json!({"__typename": "Team", "slug": "core"}));
        let v = json!({"author": {"login": "me"}, "reviewRequests": requested,
            "latestReviews": [{"author": {"login": "BO"}}]});
        std::fs::write(bin.join("view.json"), v.to_string()).unwrap();
    };
    view(vec![json!({"login": "core"})]);
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> {log:?}\ncase $2 in\nview) cat {view:?};;\n\
         edit) test ! -e {fail:?} || {{ echo down >&2; exit 1; }};;\nesac\n",
        log = bin.join("gh.log"),
        view = bin.join("view.json"),
        fail = bin.join("FAIL_GH"),
    );
    std::fs::write(&gh, script).unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&gh, mode).unwrap();
    std::fs::write(bin.join("FAIL_GH"), "").unwrap();
    lane.plan("1. do it\n");
    lane.commit();
    lane.answer("lane", "accept");
    lane.smoke();
    assert_eq!(lane.verify()["stopped"], Value::Null);
    let handoff_as = |status: &str, ready: bool, dry: &[&str], pr: &str| {
        let file = lane.dir.path().join("handoff.json");
        let json = json!({"summary": "s", "status": status, "ready": ready,
            "commit": lane.head(), "pull_request": pr});
        std::fs::write(&file, json.to_string()).unwrap();
        let args = [&["work", "handoff", "1", file.to_str().unwrap()][..], dry].concat();
        let (code, out) = lane.run("lane", &args);
        assert_eq!(code, 0, "{out}");
        out["reviewers"].clone()
    };
    let url = "https://github.com/o/r/pull/7";
    let handoff = || handoff_as("review", true, &[], "o/r#7");
    let log = || std::fs::read_to_string(bin.join("gh.log")).unwrap_or_default();
    assert_eq!(handoff_as("active", false, &[], url), Value::Null);
    assert_eq!(handoff_as("review", true, &["--dry-run"], url), Value::Null);
    assert_eq!(log(), "");
    let view_call = "pr view 7 --repo o/r --json author,reviewRequests,latestReviews\n";
    let edit = "pr edit 7 --repo o/r --add-reviewer ana,o/me\n";
    let failed = json!({"error": "gh pr edit: down"});
    assert_eq!(handoff_as("review", true, &[], url), failed);
    assert_eq!(lane.events("kind = 'phase.entered'"), 1);
    std::fs::remove_file(bin.join("FAIL_GH")).unwrap();
    assert_eq!(handoff(), json!({"requested": ["ana", "o/me"]}));
    view(vec![
        json!({"login": "Ana"}),
        json!({"__typename": "Team", "login": "ME"}),
    ]);
    assert_eq!(handoff(), json!({"requested": []}));
    assert_eq!(
        log(),
        [view_call, edit, view_call, edit, view_call].concat()
    );
}

/// The Codex lens skips on the lane's measure, added, removed and test
/// lines together: a diff recorded before `removed_code` skips on its
/// `code_lines`, which held them, and a named asset counts none; a file
/// git counts no lines of leaves no size to skip on; a changed line (two)
/// and 198 test lines make v20's minimum of 200.
#[test]
fn the_codex_lens_skips_on_the_lanes_measure() {
    let rules = "[assets]\nglobs = [\"*.png\"]\n";
    let lane = claimed_on(claude(json!({"findings": []}), json!([])), rules);
    lane.plan("1. do it\n");
    let files: &[(&str, &[u8])] = &[("lib.rs", b"c\n"), ("logo.png", b"\0\x01")];
    git(&lane.wt, &["add", "logo.png"], files);
    git(&lane.wt, &["commit", "-qam", "feat"], &[]);
    lane.answer("lane", "accept");
    lane.run("lane", &["check", "run", "diff", "--work", "1"]);
    let conn = rusqlite::Connection::open(&lane.db).unwrap();
    let old = "UPDATE events SET payload_json = json_set(json_remove(payload_json,
        '$.removed_code'), '$.code_lines', 3) WHERE payload_json->>'check' = 'diff'";
    conn.execute(old, []).unwrap();
    lane.run("lane", &["check", "run", "review", "--work", "1"]);
    let p = lane.review();
    let measured = (&p["uncounted_files"], &p["asset_files"]);
    assert_eq!(measured, (&json!(0), &json!(1)));
    let skipped = "3 code lines, under codex_min_code_lines 200";
    assert_eq!(lane.codex().0["skipped"], skipped);

    git(&lane.wt, &["add", "a.bin"], &[("a.bin", b"\0\x01")]);
    git(&lane.wt, &["commit", "-qm", "bin"], &[]);
    lane.verify();
    assert_eq!(lane.codex().0["ran"], true);

    let lines = "x\n".repeat(198);
    git(&lane.wt, &["rm", "-q", "a.bin"], &[]);
    git(
        &lane.wt,
        &["add", "tests/big.rs"],
        &[("tests/big.rs", lines.as_bytes())],
    );
    git(&lane.wt, &["commit", "-qm", "big"], &[]);
    lane.verify();
    let (codex, complete) = lane.codex();
    assert_eq!((&codex["ran"], complete), (&json!(true), json!(true)));
}

/// With `codex` down (a file first on `PATH` that cannot start), a small
/// diff is not refused: its Codex lens is skipped, so Codex is not needed.
/// At the minimum the lens falls back to Claude on Sonnet, said on its row,
/// and the review is complete.
#[test]
fn codex_down_falls_back_or_is_not_needed() {
    let mut runs = claude(json!({"findings": []}), json!([]));
    let second = says("# Review: a second reader", json!({"findings": []}));
    runs.as_array_mut().unwrap().push(second);
    let lane = lane(runs);
    let codex = lane.dir.path().join("bin/codex");
    std::fs::remove_file(&codex).unwrap();
    std::fs::write(&codex, "not a program").unwrap();
    lane.answer("lane", "accept");
    let out = lane.verify();
    assert_eq!(out["stopped"], Value::Null, "{out}");
    let (row, complete) = lane.codex();
    assert_eq!(
        (&row["ran"], complete),
        (&json!(false), json!(true)),
        "{row}"
    );

    let lines = "x\n".repeat(199);
    let files: &[(&str, &[u8])] = &[("tests/big.rs", lines.as_bytes())];
    git(&lane.wt, &["add", "tests/big.rs"], files);
    git(&lane.wt, &["commit", "-qm", "big"], &[]);
    let out = lane.verify();
    assert_eq!(out["stopped"], Value::Null, "{out}");
    let (row, complete) = lane.codex();
    let fell = json!([
        row["ran"],
        row["runner"],
        row["model"],
        row["fallback_from"],
        complete
    ]);
    let want = json!([
        true,
        "codex_fallback",
        "claude-sonnet-5-5",
        "codex_lens",
        true
    ]);
    assert_eq!(fell, want, "{row}");
    let error = row["fallback_error"].as_str().unwrap();
    assert!(error.starts_with("could not start codex"), "{error}");
}

/// A CLI that sessions need with no other engine and that cannot start
/// refuses the review, and nothing is written: fail closed.
#[test]
fn claude_down_refuses_the_review() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    let claude = lane.dir.path().join("bin/claude");
    std::fs::remove_file(&claude).unwrap();
    std::fs::write(&claude, "not a program").unwrap();
    let (code, err) = lane.run("lane", &["check", "run", "review", "--work", "1"]);
    assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
    let message = err["message"].as_str().unwrap();
    assert!(message.starts_with("could not start claude"), "{message}");
    assert_eq!(lane.events("payload_json->>'check' = 'review'"), 0);
}

/// The plugin's stop hook on a ready lane: nothing while the branch is at
/// the handoff, a block once it moves on, never with `stop_hook_active`,
/// and nothing once the lane hands off active to work again.
#[test]
fn stop_blocks_a_moved_head_until_the_lane_hands_off_active() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    lane.smoke();
    lane.verify();
    assert_eq!(lane.handoff().0, 0);
    let stop = |active: bool| {
        let mut hook = Command::new(BIN);
        let hook = hook.arg("--db").arg(&lane.db).args(["hook", "stop"]);
        let hook = hook.env("CHAPERONE_WORK", "1").stdin(Stdio::piped());
        let mut child = hook.stdout(Stdio::piped()).spawn().unwrap();
        let input = json!({"stop_hook_active": active}).to_string();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    assert_eq!(stop(false), "");
    git(
        &lane.wt,
        &["commit", "-qam", "more"],
        &[("lib.rs", b"a\nc\n")],
    );
    let out: Value = serde_json::from_str(&stop(false)).unwrap();
    assert_eq!(out["decision"], "block");
    let reason = out["reason"].as_str().unwrap();
    assert!(reason.contains("chaperone verify --work 1"), "{reason}");
    assert_eq!(stop(true), "");
    let file = lane.dir.path().join("active.json");
    let json = json!({"summary": "s", "status": "active", "next_action": "fix the red check"});
    std::fs::write(&file, json.to_string()).unwrap();
    let handoff = ["work", "handoff", "1", file.to_str().unwrap()];
    assert_eq!(lane.run("lane", &handoff).0, 0);
    assert_eq!(stop(false), "");
}

/// A blocker the verifier verifies stops verify after the review: no
/// verdict runs or is written, and the result names the stop.
#[test]
fn a_blocking_finding_stops_verify_before_the_verdict() {
    let said = json!({"findings": [{"severity": "blocker", "title": "b", "body": "why",
        "path": "lib.rs", "line": null}]});
    let key = review::findings("correctness", said.clone()).unwrap()[0]
        .finding_key
        .clone();
    let rulings = json!([{"key": key, "verdict": "verified", "why": "it is"}]);
    let lane = lane(claude(said, rulings));
    lane.answer("lane", "accept");
    let out = lane.verify();
    let stopped = json!({"check": "review", "why": "1 finding(s) still block"});
    assert_eq!(out["stopped"], stopped);
    assert_eq!(steps(&out).len(), 3);
    assert_eq!(lane.events("payload_json->>'check' = 'merge_verdict'"), 0);
}

/// On v21 a diff past the cap reaches review, which records it over the
/// cap, and the design and tests lens's `cut:` finding the verifier
/// verifies stops verify there, as any blocking finding does.
#[test]
fn an_over_cap_diff_on_v21_reaches_review_and_a_cut_blocks() {
    let cut = json!({"findings": [{"severity": "risk", "title": "cut: the helper",
        "body": "the goal does not need it", "path": "lib.rs", "line": 2}]});
    let key = review::findings("design_tests", cut.clone()).unwrap()[0]
        .finding_key
        .clone();
    let rulings = json!([{"key": key, "verdict": "verified", "why": "it is not needed"}]);
    // First match wins: the verifier is told about the cap too.
    let mut runs = vec![
        says(
            "# Verify the review's findings",
            json!({"rulings": rulings}),
        ),
        says("## Over the size cap", cut),
    ];
    runs.extend(
        claude(json!({"findings": []}), json!([]))
            .as_array()
            .unwrap()
            .clone(),
    );
    let lane = claimed_in("v21", json!(runs), "");
    lane.plan("1. do it\n");
    let big = format!("a\n{}", "b\n".repeat(401));
    git(
        &lane.wt,
        &["commit", "-qam", "feat"],
        &[("lib.rs", big.as_bytes())],
    );
    lane.answer("lane", "accept");
    let out = lane.verify();
    let stopped = json!({"check": "review", "why": "1 finding(s) still block"});
    assert_eq!(out["stopped"], stopped);
    let p = lane.review();
    assert_eq!(p["over_cap"], json!({"code_lines": 401, "cap": 400}));
    // Only the design and tests lens is asked what to cut, and the verifier
    // is told.
    let store = Evidence::beside(&lane.db);
    let lenses = p["lenses"].as_array().unwrap();
    for (lens, asked) in [
        ("correctness", false),
        ("intent", false),
        ("design_tests", true),
        ("verifier", true),
    ] {
        let row = match lens {
            "verifier" => &p["verifier"],
            _ => lenses.iter().find(|r| r["lens"] == lens).unwrap(),
        };
        let prompt = store.get(row["prompt_ref"].as_str().unwrap()).unwrap();
        assert_eq!(prompt.contains("`cut: <what>`"), asked, "{lens}");
    }
}

/// On v21 the design and tests lens is told a comment finding is a `nit`;
/// one that says so does not block, and verify passes to the verdict.
#[test]
fn a_comment_finding_on_v21_is_a_nit_that_never_blocks() {
    let nit = json!({"findings": [{"severity": "nit", "title": "comment restates the code",
        "body": "lib.rs:2 `// b`: delete", "path": "lib.rs", "line": 2}]});
    let none = json!({"findings": []});
    let yes = json!({"decision": "yes", "why": "w", "proven": ["p"], "not_proven": []});
    let risk = json!({"findings": [{"severity": "risk", "title": "r", "body": "why",
        "path": "lib.rs", "line": null}]});
    let runs = json!([
        says("# Plan check", risk),
        says("# Verify the review's findings", json!({"rulings": []})),
        says("# Merge verdict", yes),
        says("# Review: correctness", none.clone()),
        says("# Review: the change against its goal", none),
        says(
            "A comment (stale, wrong, restating the code, too long)",
            nit
        ),
    ]);
    let lane = claimed_in("v21", runs, "");
    lane.plan("1. do it\n");
    lane.commit();
    lane.answer("lane", "accept");
    lane.smoke();
    let out = lane.verify();
    let passed = ["diff", "review", "verdict"].map(|c| (c.into(), true, false));
    assert_eq!(
        (steps(&out), &out["stopped"]),
        (passed.into(), &Value::Null)
    );
    let findings = lane.findings();
    let comment = findings.iter().find(|f| f["severity"] == "nit").unwrap();
    assert_eq!(comment["blocks"], false);
    assert_eq!(lane.handoff().0, 0);
}

/// While another run holds the lane's lock, verify is a conflict and
/// writes nothing; a lane that does not exist is not_found, as for
/// `check run`.
#[test]
fn a_second_verify_on_the_lane_is_a_conflict() {
    let lane = lane(json!([]));
    let held = lock_lane(&lane.db, 1).unwrap().unwrap();
    let before = lane.events("true");
    let (code, err) = lane.run("lane", &["verify", "--work", "1"]);
    let after = lane.events("true");
    assert_eq!((code, &err["code"], after), (4, &json!("conflict"), before));
    drop(held);
    let (code, err) = lane.run("lane", &["verify", "--work", "9"]);
    assert_eq!((code, &err["code"]), (1, &json!("not_found")));
}

/// The plan check's risk refuses diff (so verify) and review until the lane
/// answers it. Accepted, every lens is shown it with the answer, and an
/// intent lens that finds the diff does not address it blocks verify.
#[test]
fn an_accepted_plan_finding_the_diff_ignores_blocks() {
    let said = json!({"findings": [{"severity": "blocker", "title": "accepted r not done",
        "body": "why", "path": "lib.rs", "line": null}]});
    let key = review::findings("intent", said.clone()).unwrap()[0]
        .finding_key
        .clone();
    let rulings = json!([{"key": key, "verdict": "verified", "why": "it is"}]);
    let lane = lane(lenses(json!({"findings": []}), said, rulings));
    let out = lane.verify();
    let why = out["stopped"]["why"].as_str().unwrap();
    assert_eq!(out["stopped"]["check"], "diff");
    assert!(
        why.starts_with("1 plan finding(s) have no answer: 4/plan-"),
        "{why}"
    );
    let (code, err) = lane.run("lane", &["check", "run", "review", "--work", "1"]);
    assert_eq!((code, err["message"].as_str()), (1, Some(why)));
    let plan = &lane.findings()[0];
    assert_eq!(
        (&plan["check"], &plan["status"], &plan["blocks"]),
        (&json!("plan_check"), &json!("open"), &json!(false))
    );

    lane.answer("lane", "accept");
    let plan = &lane.findings()[0];
    assert_eq!(
        (&plan["status"], &plan["reason"]),
        (&json!("accepted"), &json!("r"))
    );
    let out = lane.verify();
    let stopped = json!({"check": "review", "why": "1 finding(s) still block"});
    assert_eq!(out["stopped"], stopped);
    let log = std::fs::read_to_string(lane.dir.path().join("bin/claude.log")).unwrap();
    let shown = |header: &str| {
        let line = log
            .lines()
            .find(|l| l.contains(&format!(r#""prompt":"{header}"#)));
        line.unwrap()
            .contains(r#"\"status\":\"accepted\",\"title\""#)
    };
    assert!(shown("# Review: the change against its goal"));
    assert!(shown("# Review: correctness"));
    assert!(!shown("# Verify the review's findings"));
    assert!(!log.contains("The lane's earlier review findings"));
    // A new commit's lenses are shown the first review's finding.
    git(&lane.wt, &["commit", "-qam", "more"], &[("lib.rs", b"b\n")]);
    lane.run("lane", &["check", "run", "review", "--work", "1"]);
    let log = std::fs::read_to_string(lane.dir.path().join("bin/claude.log")).unwrap();
    let lens = log
        .lines()
        .rfind(|l| l.contains(r##""prompt":"# Review: correctness"##));
    assert!(
        lens.unwrap()
            .contains("## The lane's earlier review findings")
    );
}

/// A changed plan answer on an unchanged commit reviews again: the stored
/// review replays only while the answers its lenses were shown stand. The
/// lane disputes the plan finding, then the owner waives it.
#[test]
fn a_changed_plan_answer_reviews_the_same_commit_again() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "dispute");
    let review = || {
        let (code, out) = lane.run("lane", &["check", "run", "review", "--work", "1"]);
        assert_eq!(code, 0, "{out}");
        (out["event_id"].clone(), out["replayed"] == true)
    };
    let (first, _) = review();
    assert_eq!(review(), (first.clone(), true));
    lane.answer("owner", "waive");
    let sessions = lane.sessions();
    let (second, replayed) = review();
    assert!(second != first && !replayed, "{second} {first}");
    assert_eq!(lane.sessions(), sessions + 4);
}

/// Plan A (a risk), plan B (none), then A again: the replay starts no
/// session and makes A the current plan check again, so its risk, never
/// answered, refuses verify until the lane answers it.
#[test]
fn a_replayed_older_plan_is_the_current_one() {
    let mut runs = claude(json!({"findings": []}), json!([]));
    let none = says("PLAN-B", json!({"findings": []}));
    runs.as_array_mut().unwrap().insert(0, none);
    let lane = claimed(runs);
    let a = lane.plan("PLAN-A\n");
    assert!(lane.plan("PLAN-B\n")["event_id"] != a["event_id"]);
    let sessions = lane.sessions();
    let again = lane.plan("PLAN-A\n");
    assert_eq!(
        (&again["payload"]["carried_from"], lane.sessions()),
        (&a["event_id"], sessions)
    );
    lane.commit();
    let out = lane.verify();
    let why = out["stopped"]["why"].as_str().unwrap();
    let named = format!("1 plan finding(s) have no answer: {}/plan-", a["event_id"]);
    assert!(why.starts_with(&named), "{why}");
    let plan = &lane.findings()[0];
    assert_eq!(
        (&plan["check_event_id"], &plan["status"]),
        (&a["event_id"], &json!("open"))
    );
    lane.answer("lane", "accept");
    lane.smoke();
    assert_eq!(lane.verify()["stopped"], Value::Null);
}

/// A plan answer changed after verify passed leaves the commit with no
/// review under the answers that stand: the ready handoff and the verdict
/// are refused until a review reads them.
#[test]
fn a_plan_answer_changed_after_verify_needs_a_new_review() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "dispute");
    lane.smoke();
    assert_eq!(lane.verify()["stopped"], Value::Null);
    lane.answer("owner", "waive");
    let (code, err) = lane.handoff();
    assert_eq!((code, &err["code"]), (1, &json!("gate_denied")), "{err}");
    assert!(err.to_string().contains("review_complete"), "{err}");
    let (code, err) = lane.run("lane", &["check", "run", "verdict", "--work", "1"]);
    let why = err["message"].as_str().unwrap();
    assert!(
        code == 1 && why.contains("no harness complete review"),
        "{err}"
    );
    assert_eq!(lane.verify()["stopped"], Value::Null);
    assert_eq!(lane.handoff().0, 0);
}

/// A clean merge from main keeps the lane's diff, so its patch-id: the
/// review is carried to the merge, `carried_by` `patch_id`, and no session
/// starts. Moving the lane's line, or a whitespace-only edit to it, changes
/// the diff and runs a fresh review, whatever the caller's diff options.
#[test]
fn a_clean_merge_from_main_carries_the_review() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    let review = || {
        let (code, out) = lane.run("lane", &["check", "run", "review", "--work", "1"]);
        assert_eq!(code, 0, "{out}");
        out
    };
    let first = review();
    let mut sessions = lane.sessions();
    assert!(first["payload"]["patch_id"].is_string(), "{first}");

    git(&lane.wt, &["checkout", "-q", "main"], &[]);
    git(&lane.wt, &["add", "other.rs"], &[("other.rs", b"x\n")]);
    git(&lane.wt, &["commit", "-qm", "main moves"], &[]);
    git(
        &lane.wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    git(&lane.wt, &["checkout", "-q", "feat"], &[]);
    git(
        &lane.wt,
        &["merge", "-q", "--no-edit", "refs/remotes/origin/main"],
        &[],
    );
    let carried = review();
    let p = &carried["payload"];
    assert_eq!(
        (&p["head_sha"], &p["carried_from"], &p["carried_by"]),
        (&json!(lane.head()), &first["event_id"], &json!("patch_id"))
    );
    assert_eq!(lane.sessions(), sessions);
    let again = review();
    assert_eq!(
        (&again["event_id"], &again["replayed"]),
        (&carried["event_id"], &json!(true))
    );

    for (step, text) in [("moved", &b"b\na\n"[..]), ("space", b"b \na\n")] {
        git(&lane.wt, &["commit", "-qam", step], &[("lib.rs", text)]);
        let fresh = review();
        assert!(fresh["payload"].get("carried_from").is_none(), "{fresh}");
        assert!(lane.sessions() > sessions);
        sessions = lane.sessions();
    }
}

/// On v21 a rebase onto a newer main that keeps the lane's patch-id
/// carries the review and the verdict, `carried_by` `patch_id` on each
/// step, and starts no session, nor does an amend after it (no `carried_by`
/// then); `merge --head` takes the rebased head. A
/// one-line change to the patch runs a fresh review and verdict.
#[test]
fn a_rebase_that_keeps_the_patch_carries_the_review_and_verdict() {
    let lane = claimed_in("v21", claude(json!({"findings": []}), json!([])), "");
    lane.plan("1. do it\n");
    lane.commit();
    lane.answer("lane", "accept");
    lane.smoke();
    let first = lane.verify();
    assert_eq!(first["stopped"], Value::Null, "{first}");
    let sessions = lane.sessions();

    git(&lane.wt, &["checkout", "-q", "main"], &[]);
    git(&lane.wt, &["add", "other.rs"], &[("other.rs", b"x\n")]);
    git(&lane.wt, &["commit", "-qm", "main moves"], &[]);
    let main = git(&lane.wt, &["rev-parse", "HEAD"], &[]);
    git(
        &lane.wt,
        &["update-ref", "refs/remotes/origin/main", &main],
        &[],
    );
    git(&lane.wt, &["checkout", "-q", "feat"], &[]);
    git(&lane.wt, &["rebase", "-q", &main], &[]);
    lane.smoke();
    let second = lane.verify();
    assert_eq!(second["stopped"], Value::Null, "{second}");
    assert_eq!(lane.sessions(), sessions);
    let steps = second["steps"].as_array().unwrap();
    for (step, check) in steps[1..].iter().zip(["review", "verdict"]) {
        let first = first["steps"].as_array().unwrap().iter();
        let mut from = first
            .filter(|s| s["check"] == check)
            .map(|s| &s["event_id"]);
        assert_eq!(
            (
                &step["check"],
                &step["carried_by"],
                Some(&step["carried_from"])
            ),
            (&json!(check), &json!("patch_id"), from.next_back()),
        );
    }

    git(
        &lane.wt,
        &["commit", "-q", "--amend", "-m", "same tree"],
        &[],
    );
    lane.smoke();
    let amend = lane.verify();
    assert_eq!(lane.sessions(), sessions);
    let steps = amend["steps"].as_array().unwrap();
    for step in &steps[1..] {
        let carried = (step.get("carried_from").is_some(), step.get("carried_by"));
        assert_eq!(carried, (true, None), "{amend}");
    }

    let rebased = lane.head();
    let bin = lane.dir.path().join("bin");
    let view = bin.join("view.json");
    let pr = json!({"state": "OPEN", "headRefOid": rebased,
        "mergeCommit": {"oid": "e".repeat(40)}, "mergedAt": "2026-11-09T10:00:00Z"});
    std::fs::write(&view, pr.to_string()).unwrap();
    let merged = pr.to_string().replace("OPEN", "MERGED");
    std::fs::write(bin.join("merged.json"), merged).unwrap();
    let gh = format!(
        "#!/bin/sh\necho \"$*\" >> {log:?}\ncase $2 in view) cat {view:?};; \
         merge) cp {merged:?} {view:?};; esac\n",
        log = bin.join("gh.log"),
        merged = bin.join("merged.json"),
    );
    for (cli, body) in [("gh", gh.as_str()), ("herdr", "#!/bin/sh\n")] {
        std::fs::write(bin.join(cli), body).unwrap();
        let chmod = Command::new("chmod").arg("+x").arg(bin.join(cli)).status();
        assert!(chmod.unwrap().success());
    }
    let file = lane.dir.path().join("handoff.json");
    let ready = json!({"summary": "s", "status": "review", "ready": true, "commit": rebased,
        "pull_request": "o/r#1"});
    std::fs::write(&file, ready.to_string()).unwrap();
    let (code, out) = lane.run("lane", &["work", "handoff", "1", file.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = lane.run("owner", &["merge", "--work", "1", "--head", &rebased]);
    assert_eq!(code, 0, "{out}");
    let log = std::fs::read_to_string(bin.join("gh.log")).unwrap();
    assert!(
        log.contains(&format!("--match-head-commit {rebased}")),
        "{log}"
    );

    git(
        &lane.wt,
        &["commit", "-qam", "one line"],
        &[("lib.rs", b"a\nb\nc\n")],
    );
    lane.smoke();
    let fresh = lane.verify();
    let steps = fresh["steps"].as_array().unwrap();
    assert!(
        steps.iter().all(|s| s.get("carried_by").is_none()),
        "{fresh}"
    );
    assert_eq!(fresh["stopped"], Value::Null, "{fresh}");
    assert_eq!(lane.sessions(), sessions + 4, "{fresh}");
}

/// A lane diff git cannot give is refused before any review, and writes
/// nothing.
#[test]
fn a_review_whose_patch_id_diff_fails_is_refused() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    // The fake's directory is first on PATH; the shim drops it to find git.
    let shim = "#!/bin/sh\ncase \" $* \" in *' --binary '*) exit 1;; esac\n\
                PATH=${PATH#*:} exec git \"$@\"\n";
    let bin = lane.dir.path().join("bin/git");
    std::fs::write(&bin, shim).unwrap();
    let chmod = Command::new("chmod").arg("+x").arg(&bin).status();
    assert!(chmod.unwrap().success());
    let before = lane.events("1 = 1");
    let (code, err) = lane.run("lane", &["check", "run", "review", "--work", "1"]);
    let why = err["message"].as_str().unwrap();
    assert!(code != 0 && why.starts_with("could not diff"), "{err}");
    assert_eq!((lane.events("1 = 1"), lane.sessions()), (before, 1));
}

/// A base profile's required lens, `when` to follow.
const LOSS: &str = "[review]\ndaisy = false\n[[review.required]]\nname = \"loss\"\nprompt = \"Can a line be lost?\"\nwhen = ";

/// A change to a file a `[[review.required]]` lens names gets that lens:
/// while it has not run (the fake has no answer for it) the review is not
/// complete, verify stops there and the lane cannot hand off ready; once
/// it answers, verify passes and the handoff is ready.
#[test]
fn a_required_lens_gates_ready_on_the_paths_it_names() {
    let none = || claude(json!({"findings": []}), json!([]));
    let lane = lane_on(none(), &format!("{LOSS}[\"*.rs\"]\n"));
    lane.answer("lane", "accept");
    lane.smoke();
    let out = lane.verify();
    let stopped = json!({"check": "review", "why": "review not complete"});
    assert_eq!(out["stopped"], stopped, "{out}");
    let review = lane.review();
    let rows = review["lenses"].as_array().unwrap();
    let loss = rows.iter().find(|r| r["lens"] == "loss").unwrap();
    assert_eq!(
        (&loss["ran"], &review["complete"]),
        (&json!(false), &json!(false))
    );
    let (code, err) = lane.handoff();
    assert_eq!((code, &err["code"]), (1, &json!("gate_denied")), "{err}");

    let mut runs = none();
    let loss = says("# Review: loss", json!({"findings": []}));
    runs.as_array_mut().unwrap().push(loss);
    let script = json!({"version": "1 (fake)", "runs": runs}).to_string();
    std::fs::write(lane.dir.path().join("bin/claude.json"), script).unwrap();
    let out = lane.verify();
    assert_eq!(out["stopped"], Value::Null, "{out}");
    assert_eq!(lane.review()["complete"], true);
    assert_eq!(lane.handoff().0, 0);
}

/// A lane that changes no file a required lens names reviews as before:
/// the pipeline's lenses only, keyed by the pipeline's hash.
#[test]
fn a_required_lens_leaves_other_paths_alone() {
    let lane = lane_on(
        claude(json!({"findings": []}), json!([])),
        &format!("{LOSS}[\"store.rs\"]\n"),
    );
    lane.answer("lane", "accept");
    assert_eq!(lane.verify()["stopped"], Value::Null);
    assert_eq!(lane.sessions(), 5);
    let review = lane.review();
    let rows = review["lenses"].as_array().unwrap().iter();
    let names: Vec<_> = rows.map(|r| r["lens"].as_str().unwrap()).collect();
    assert_eq!(names, review::LENSES);
    let conn = rusqlite::Connection::open(&lane.db).unwrap();
    let sql = "SELECT pipeline_hash FROM lanes WHERE id = 1";
    let hash: String = conn.query_row(sql, [], |r| r.get(0)).unwrap();
    assert_eq!(review["profile"], hash);
}

/// A required lens that does not load (no `when`, a name with a space or
/// `:`, a blank prompt, a field it does not know) or whose name is a
/// pipeline lens's, the verifier's or given twice: the review is refused,
/// naming it, and nothing is written.
#[test]
fn a_bad_required_lens_is_refused() {
    let rule = |name: &str, prompt: &str, when: &str| {
        format!("[[review.required]]\nname = \"{name}\"\nprompt = \"{prompt}\"\nwhen = {when}\n")
    };
    let any = "[\"*.rs\"]";
    let bad = [
        rule("loss", "p", "[]"),
        rule("a b", "p", any),
        rule("a:b", "p", any),
        rule("loss", " ", any),
        rule("loss", "p", any) + "runner = \"x\"\n",
        rule("correctness", "p", any),
        rule("verifier", "p", any),
        rule("loss", "p", any) + &rule("loss", "q", any),
    ];
    for rules in bad {
        // The plan check reads the profile too, so the lane commits unplanned.
        let lane = claimed_on(json!([]), &rules);
        lane.commit();
        let before = lane.events("1 = 1");
        let (code, err) = lane.run("lane", &["check", "run", "review", "--work", "1"]);
        assert_eq!(
            (code, &err["code"]),
            (1, &json!("invalid_input")),
            "{rules}: {err}"
        );
        let why = err["message"].as_str().unwrap();
        assert!(
            why.contains("[[review.required]]") || why.contains("unknown field `runner`"),
            "{err}"
        );
        assert_eq!(lane.events("1 = 1"), before);
    }
}

/// A rule that lands on the base after the lane's commit had a complete
/// review: the next review is keyed anew, so the old one neither replays
/// nor carries, and while the required lens has not run that newer,
/// incomplete review uncovers the old one: verify stops at the review and
/// the verdict refuses to run.
#[test]
fn a_rule_landing_after_a_complete_review_reviews_again() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    assert_eq!(lane.verify()["stopped"], Value::Null);
    let old = lane.review();
    git(&lane.wt, &["checkout", "-q", "main"], &[]);
    let profile = std::fs::read_to_string(lane.wt.join(".chaperone/profile.toml")).unwrap();
    let profile = profile + LOSS + "[\"*.rs\"]\n";
    let files: &[(&str, &[u8])] = &[(".chaperone/profile.toml", profile.as_bytes())];
    git(&lane.wt, &["commit", "-qam", "rule"], files);
    git(
        &lane.wt,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
        &[],
    );
    git(&lane.wt, &["checkout", "-q", "feat"], &[]);

    let out = lane.verify();
    let stopped = json!({"check": "review", "why": "review not complete"});
    assert_eq!(out["stopped"], stopped, "{out}");
    let review = lane.review();
    assert_ne!(review["profile"], old["profile"]);
    assert_eq!(review["carried_from"], Value::Null);
    let (code, err) = lane.run("lane", &["check", "run", "verdict", "--work", "1"]);
    assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
    assert!(
        err["message"].as_str().unwrap().contains("complete review"),
        "{err}"
    );
}

/// A lane that forges its clone's base ref back to a profile without the
/// rule, reviews there (no required lens) and restores the ref gets no
/// verdict: the review's key is not the one the base profile requires now.
#[test]
fn a_review_under_a_forged_base_ref_gets_no_verdict() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    let old = git(&lane.wt, &["rev-parse", "main"], &[]);
    git(&lane.wt, &["checkout", "-q", "main"], &[]);
    let profile = std::fs::read_to_string(lane.wt.join(".chaperone/profile.toml")).unwrap();
    let profile = profile + LOSS + "[\"*.rs\"]\n";
    let files: &[(&str, &[u8])] = &[(".chaperone/profile.toml", profile.as_bytes())];
    git(&lane.wt, &["commit", "-qam", "rule"], files);
    let rule = git(&lane.wt, &["rev-parse", "HEAD"], &[]);
    git(&lane.wt, &["checkout", "-q", "feat"], &[]);

    git(
        &lane.wt,
        &["update-ref", "refs/remotes/origin/main", &old],
        &[],
    );
    for check in ["diff", "tests", "review"] {
        let (code, out) = lane.run("lane", &["check", "run", check, "--work", "1"]);
        assert_eq!(code, 0, "{out}");
    }
    assert_eq!(lane.review()["complete"], true);
    git(
        &lane.wt,
        &["update-ref", "refs/remotes/origin/main", &rule],
        &[],
    );
    let before = lane.events("1 = 1");
    let (code, err) = lane.run("lane", &["check", "run", "verdict", "--work", "1"]);
    assert_eq!((code, &err["code"]), (1, &json!("invalid_input")), "{err}");
    let why = "no harness complete review by the lenses the base profile requires now";
    assert!(err["message"].as_str().unwrap().contains(why), "{err}");
    assert_eq!(lane.events("1 = 1"), before);
}
