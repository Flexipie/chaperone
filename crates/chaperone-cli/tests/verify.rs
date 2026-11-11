//! `verify` through the binary on a v20 lane, with `chaperone-fake` as
//! `claude` and `codex`. The fake is the workspace's binary beside this
//! one, so these tests need the workspace built (`cargo nextest run
//! --workspace` builds it).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chaperone_core::ledger::lock_lane;
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
        let args = format!("finding {verb} {event} {} --reason r", key.unwrap());
        let args: Vec<_> = args.split(' ').collect();
        assert_eq!(self.run(who, &args).0, 0);
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

    /// The newest review's codex row and whether the review is complete.
    fn codex(&self) -> (Value, Value) {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        let sql = "SELECT payload_json FROM events WHERE payload_json->>'check' = 'review'
                     ORDER BY id DESC LIMIT 1";
        let p: String = conn.query_row(sql, [], |r| r.get(0)).unwrap();
        let p: Value = serde_json::from_str(&p).unwrap();
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
/// checked before its one code commit; the base profile's tests
/// pass and it declares no smoke. The fake plays `claude` for Claude and
/// finds nothing as Codex.
fn lane(claude: Value) -> Lane {
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
    let profile: &[u8] = b"[tests]\ncommands = [[\"true\"]]\n";
    let files: &[(&str, &[u8])] = &[("lib.rs", b"a\n"), (".chaperone/profile.toml", profile)];
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
        "work claim 1 --worktree {} --branch feat --pipeline v20",
        lane.wt.display()
    );
    let claim: Vec<_> = claim.split(' ').collect();
    assert_eq!(lane.run("lane", &["work", "create", "--title", "t"]).0, 0);
    assert_eq!(lane.run("lane", &claim).0, 0);
    let plan = lane.dir.path().join("plan.md");
    std::fs::write(&plan, "1. do it\n").unwrap();
    let plan = [
        "check",
        "run",
        "plan",
        "--work",
        "1",
        "--file",
        plan.to_str().unwrap(),
    ];
    assert_eq!(lane.run("lane", &plan).0, 0);
    git(
        &lane.wt,
        &["commit", "-qam", "feat"],
        &[("lib.rs", b"a\nb\n")],
    );
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
    // The plan, three lenses and the verdict; nothing found, so no verifier.
    // Codex is skipped: one code line, under v20's 200.
    assert_eq!(lane.sessions(), 5);
    let skipped = json!({"lens": "codex", "ran": false,
        "skipped": "1 code lines, under codex_min_code_lines 200"});
    assert_eq!(lane.codex(), (skipped, json!(true)));
    assert_eq!(
        lane.events("kind = 'usage.reported' AND payload_json LIKE '%codex%'"),
        0
    );
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

/// A diff of 200 code lines (the lane's one and 199 more), v20's minimum,
/// runs the Codex lens too.
#[test]
fn a_diff_at_the_minimum_runs_the_codex_lens() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    let lines = "x\n".repeat(199);
    let files: &[(&str, &[u8])] = &[("big.rs", lines.as_bytes())];
    git(&lane.wt, &["add", "big.rs"], files);
    git(&lane.wt, &["commit", "-qm", "big"], &[]);
    let out = lane.verify();
    assert_eq!(out["stopped"], Value::Null, "{out}");
    let (codex, complete) = lane.codex();
    assert_eq!(
        (&codex["ran"], &codex["skipped"], complete),
        (&json!(true), &Value::Null, json!(true))
    );
    // The plan, four lenses and the verdict.
    assert_eq!(lane.sessions(), 6);
}

/// A small diff with a file git counts no lines of (binary) is no size to
/// skip on: the Codex lens runs.
#[test]
fn a_diff_with_an_uncounted_file_runs_the_codex_lens() {
    let lane = lane(claude(json!({"findings": []}), json!([])));
    lane.answer("lane", "accept");
    git(&lane.wt, &["add", "a.bin"], &[("a.bin", b"\0\x01")]);
    git(&lane.wt, &["commit", "-qm", "bin"], &[]);
    lane.verify();
    assert_eq!(lane.codex().0["ran"], true);
}

/// The plugin's stop hook on a ready lane: nothing while the branch is at
/// the handoff, a block once it moves on, never with `stop_hook_active`.
#[test]
fn stop_blocks_once_when_the_head_moved_past_a_ready_handoff() {
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
