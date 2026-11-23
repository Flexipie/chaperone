//! The lane page's data (#111): `lane_page` (the newest handoff, gates
//! required vs passed), and `git::changed_files`, `head` and `commit_files`
//! on a temp repo.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::checks::CheckInput;
use chaperone_core::git::{changed_files, commit_files, git_bin, head};
use chaperone_core::ledger::Ledger;
use chaperone_core::pipeline::Phase;
use chaperone_core::work::{ClaimInput, CreateInput, HandoffInput, UpdateInput};
use common::actor;

const OLD: &str = "2026-01-01T00:00:00.000Z";
const PRE: &str = "2025-12-31T00:00:00.000Z";

/// A claimed lane on `repo` holding PR `<repo>#<id>`, created at `created`
/// and claimed at `at` (None: now).
fn lane(ledger: &Ledger, repo: &str, created: Option<&str>, at: Option<&str>) -> i64 {
    let a = actor("1");
    let input = CreateInput {
        title: "t".into(),
        repo: Some(repo.into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let id = ledger
        .work_create(&a, &input, created, false)
        .unwrap()
        .unwrap()
        .id;
    let claim = ClaimInput {
        branch: Some(format!("b{id}")),
        ..ClaimInput::default()
    };
    let item = ledger
        .work_claim(&a, id, &claim, at, false)
        .unwrap()
        .unwrap();
    let update = UpdateInput {
        pr_ref: Some(format!("{repo}#{id}")),
        ..UpdateInput::default()
    };
    ledger
        .work_update(&a, id, &update, Some(&item.updated_at), None, false)
        .unwrap()
        .unwrap();
    id
}

fn check(ledger: &Ledger, work: i64, check: &str) {
    let input = CheckInput {
        work,
        check: check.into(),
        head: "abc1234".into(),
        outcome: "ran".into(),
        findings: (check == "plan_check").then_some(0),
        ..CheckInput::default()
    };
    ledger
        .check_record(&actor("check"), &input)
        .unwrap()
        .unwrap();
}

#[test]
fn lane_page_reads_handoff_and_gates() {
    let (_dir, _path, ledger) = common::ledger();
    let id = lane(&ledger, "o/r", Some(PRE), Some(OLD));
    ledger
        .work_claim(&actor("1"), id, &ClaimInput::default(), None, false)
        .unwrap()
        .unwrap();
    check(&ledger, id, "plan_check");
    check(&ledger, id, "review_round");
    let handoff = HandoffInput {
        summary: "built".into(),
        commit: Some("abc1234".into()),
        ..HandoffInput::default()
    };
    ledger
        .work_handoff(&actor("1"), id, &handoff, None, None, false)
        .unwrap()
        .unwrap();

    let page = ledger.lane_page(id).unwrap();
    let newest = page.handoff.unwrap().handoff;
    assert_eq!(newest.summary, "built");
    assert!(page.merged.is_none());

    let gates = page.gates.unwrap();
    assert_eq!(gates.to, Phase::Ready);
    assert_eq!(gates.head.as_deref(), Some("abc1234"));
    let passed: Vec<(&str, bool)> = gates
        .gates
        .iter()
        .map(|g| (g.gate.as_str(), g.passed))
        .collect();
    assert_eq!(
        passed,
        [
            ("stamp_ok", true),
            ("plan_check_recorded", true),
            ("quality_gate_recorded", false),
            ("verdict_is_yes", false),
            ("verdict_on_head", false),
            ("smoke_ok", false),
            ("kept_tests_ok", false),
            ("rounds_at_most", true),
        ]
    );

    // An unknown lane: no handoff, no gates (never "0 of 0 passed").
    let none = ledger.lane_page(99).unwrap();
    assert!(none.handoff.is_none() && none.gates.is_none());
}

fn rev_parse(dir: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    assert!(out.status.success(), "git rev-parse {rev}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// Committed, uncommitted and untracked files against the merge base,
/// each with its +/-; a binary or untracked file has no counts.
#[test]
fn changed_files_against_the_merge_base() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path();
    git(wt, &["init", "-q", "-b", "main"]);
    std::fs::write(wt.join("a.rs"), "1\n2\n3\n").unwrap();
    std::fs::write(wt.join("gone.md"), "x\n").unwrap();
    git(wt, &["add", "."]);
    git(wt, &["commit", "-qm", "base"]);
    let base = rev_parse(wt, "HEAD");
    std::fs::write(wt.join("a.rs"), "1\nnew\n3\n4\n").unwrap();
    git(wt, &["rm", "-q", "gone.md"]);
    git(wt, &["add", "."]);
    git(wt, &["commit", "-qm", "lane"]);
    std::fs::write(wt.join("a.rs"), "1\nnew\n3\n4\n5\n").unwrap();
    std::fs::write(wt.join("new.rs"), "a\nb").unwrap();
    std::fs::write(wt.join("blob.bin"), [0u8, 1, 2]).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let (got_base, files) = changed_files(
        &git_bin(deadline),
        wt,
        Some(base.as_str()),
        deadline,
        1 << 20,
    )
    .unwrap();
    assert_eq!(got_base, base);
    let rows: Vec<String> = files
        .iter()
        .map(|f| {
            let n = |c: Option<u64>| c.map_or("-".into(), |c| c.to_string());
            let tracked = if f.untracked { "untracked" } else { "tracked" };
            format!("{} +{} -{} {tracked}", f.path, n(f.added), n(f.removed))
        })
        .collect();
    assert_eq!(
        rows,
        [
            "a.rs +3 -1 tracked",
            "gone.md +0 -1 tracked",
            "blob.bin +- -- untracked",
            "new.rs +- -- untracked",
        ]
    );
}

/// `head` is the worktree's commit; `commit_files` counts that commit
/// against the merge base, without main's later commit, the uncommitted
/// line or the untracked file.
#[test]
fn commit_files_are_the_commits_own() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path();
    git(wt, &["init", "-q", "-b", "main"]);
    std::fs::write(wt.join("a.rs"), "1\n2\n3\n").unwrap();
    git(wt, &["add", "."]);
    git(wt, &["commit", "-qm", "base"]);
    git(wt, &["switch", "-qc", "lane"]);
    std::fs::write(wt.join("a.rs"), "1\nnew\n3\n4\n").unwrap();
    git(wt, &["commit", "-qam", "lane"]);
    let lane_head = rev_parse(wt, "HEAD");
    // Main moves on after the lane branched.
    git(wt, &["switch", "-q", "main"]);
    std::fs::write(wt.join("main.rs"), "m\n").unwrap();
    git(wt, &["add", "."]);
    git(wt, &["commit", "-qm", "main moves"]);
    let main = rev_parse(wt, "HEAD");
    git(wt, &["switch", "-q", "lane"]);
    std::fs::write(wt.join("a.rs"), "uncommitted\n").unwrap();
    std::fs::write(wt.join("new.rs"), "a\n").unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let bin = git_bin(deadline);
    let at = head(&bin, wt, deadline).unwrap();
    assert_eq!(at, lane_head);
    let (base, files) = commit_files(&bin, wt, Some(&main), &at, deadline, 1 << 20).unwrap();
    assert_eq!(base, main);
    let rows: Vec<_> = files
        .iter()
        .map(|f| (f.path.as_str(), f.added, f.removed, f.untracked))
        .collect();
    assert_eq!(rows, [("a.rs", Some(2), Some(1), false)]);

    // A repository with no commit has no head.
    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-q", "-b", "main"]);
    let e = head(&bin, empty.path(), deadline).unwrap_err();
    assert!(e.starts_with("git rev-parse:"), "{e}");
}
