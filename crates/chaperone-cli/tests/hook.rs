//! `hook` through the binary, on a v19 lane claimed on branch `feat` of a
//! real worktree, and the plugin's shim.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

struct Lane {
    _dir: tempfile::TempDir,
    db: PathBuf,
    wt: PathBuf,
}

impl Lane {
    fn new() -> Lane {
        let dir = tempfile::tempdir().unwrap();
        let (db, wt) = (dir.path().join("ledger.db"), dir.path().join("wt"));
        std::fs::create_dir(&wt).unwrap();
        let lane = Lane { _dir: dir, db, wt };
        lane.git(&["init", "-q", "-b", "feat"]);
        lane.git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        lane.cli("lane", &["migrate"]);
        lane.cli("lane", &["work", "create", "--title", "t"]);
        let wt = lane.wt.to_str().unwrap();
        lane.cli(
            "lane",
            &["work", "claim", "1", "--worktree", wt, "--branch", "feat"],
        );
        lane
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.wt)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// A command as `session`, which must succeed; its `data`.
    fn cli(&self, session: &str, args: &[&str]) -> Value {
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .envs([("DAISY_AGENT", session), ("DAISY_SESSION", session)])
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    }

    /// Every event's kind and payload, oldest first: `work events` leaves
    /// out attention.
    fn events(&self) -> Vec<(String, Value)> {
        let db = rusqlite::Connection::open(&self.db).unwrap();
        let mut rows = db
            .prepare("SELECT kind, payload_json FROM events ORDER BY id")
            .unwrap();
        let rows = rows.query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)));
        let rows = rows.unwrap().map(|r| r.unwrap());
        rows.map(|(k, p)| (k, serde_json::from_str(&p).unwrap()))
            .collect()
    }

    /// `hook <event>` in the lane's session, `work` its CHAPERONE_WORK;
    /// exits 0, and prints its stdout.
    fn hook(&self, work: Option<&str>, event: &str, input: Value) -> String {
        let mut cmd = Command::new(BIN);
        cmd.arg("--db").arg(&self.db).args(["hook", event]);
        cmd.envs([("DAISY_AGENT", "lane"), ("DAISY_SESSION", "lane")]);
        match work {
            Some(work) => cmd.env("CHAPERONE_WORK", work),
            None => cmd.env_remove("CHAPERONE_WORK"),
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.to_string().as_bytes()).unwrap();
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{event}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, event: &str, input: Value) -> Value {
        serde_json::from_str(&self.hook(Some("1"), event, input)).unwrap()
    }
}

/// The runner's children never see the lane: `CHAPERONE_WORK` is among the
/// variables chaperone-fake's `an_answer_is_read_from_a_fresh_session`
/// asserts a headless session does not get.
#[test]
fn a_hook_outside_a_lane_does_nothing() {
    let lane = Lane::new();
    lane.cli("lane", &["work", "create", "--title", "u"]);
    lane.cli("lane", &["work", "cancel", "2"]);
    let before = lane.events();
    let input = json!({"error": "rate_limit", "tool_input": {"command": "gh pr create"}});
    for event in ["session-start", "pre-bash", "stop", "stop-failure"] {
        for work in [None, Some("x"), Some("2"), Some("3")] {
            assert_eq!(
                lane.hook(work, event, input.clone()),
                "",
                "{event} {work:?}"
            );
        }
    }
    assert_eq!(lane.events(), before);
}

#[test]
fn session_start_names_the_lane_and_what_is_missing() {
    let lane = Lane::new();
    let out = lane.json("session-start", json!({"source": "startup"}));
    let context = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    for want in [
        "work 1",
        "pipeline v19",
        "plan_check_recorded",
        "verify --work 1",
    ] {
        assert!(context.contains(want), "{want}: {context}");
    }
}

#[test]
fn pre_bash_denies_what_the_binary_owns() {
    let lane = Lane::new();
    let bash = |command: &str| json!({"tool_input": {"command": command}});
    for (command, reason) in [
        ("gh pr create --fill", "chaperone pr open --work 1"),
        ("cd wt && gh pr merge 5 --squash", "chaperone merge"),
        ("chaperone work ready 1 --yes", "the owner's override"),
    ] {
        let out = &lane.json("pre-bash", bash(command))["hookSpecificOutput"];
        assert_eq!(out["permissionDecision"], "deny", "{command}");
        let why = out["permissionDecisionReason"].as_str().unwrap();
        assert!(why.contains(reason), "{command}: {why}");
    }
    for command in ["gh pr view 5", "git status"] {
        assert_eq!(lane.hook(Some("1"), "pre-bash", bash(command)), "");
    }
}

#[test]
fn stop_blocks_once_when_the_head_moved_past_a_ready_handoff() {
    let lane = Lane::new();
    let head = lane.git(&["rev-parse", "HEAD"]);
    for args in [
        &["plan_check", "--outcome", "ran", "--findings", "0"][..],
        &["quality_gate", "--outcome", "ran", "--findings", "0"],
        &["merge_verdict", "--outcome", "ran", "--decision", "yes"],
        &["smoke", "--outcome", "ran"],
        &["kept_tests", "--outcome", "skipped", "--reason", "none"],
    ] {
        let check = ["check", "record", "--work", "1", "--head", &head, "--check"];
        lane.cli("checker", &[&check[..], args].concat());
    }
    let handoff = lane.wt.join("../handoff.json");
    let ready = json!({"summary": "s", "status": "review", "ready": true, "commit": head});
    std::fs::write(&handoff, ready.to_string()).unwrap();
    lane.cli("lane", &["work", "handoff", "1", handoff.to_str().unwrap()]);
    let stop = |active: bool| lane.hook(Some("1"), "stop", json!({"stop_hook_active": active}));
    assert_eq!(stop(false), "");
    lane.git(&["commit", "-q", "--allow-empty", "-m", "two"]);
    let out: Value = serde_json::from_str(&stop(false)).unwrap();
    assert_eq!(out["decision"], "block");
    let reason = out["reason"].as_str().unwrap();
    assert!(reason.contains("chaperone verify --work 1"), "{reason}");
    assert_eq!(stop(true), "");
}

#[test]
fn stop_failure_records_one_attention_per_kind() {
    let lane = Lane::new();
    let head = lane.git(&["rev-parse", "HEAD"]);
    let before = lane.events();
    let stopped = || lane.hook(Some("1"), "stop-failure", json!({"error": "rate_limit"}));
    assert_eq!(stopped(), "");
    // Something new on the lane, so only the kind's key makes the repeat replay.
    let smoke = ["--check", "smoke", "--head", &head, "--outcome", "ran"];
    lane.cli(
        "checker",
        &[&["check", "record", "--work", "1"][..], &smoke].concat(),
    );
    assert_eq!(stopped(), "");
    let events = lane.events();
    let [(kind, payload), _] = &events[before.len()..] else {
        panic!("{events:?}");
    };
    assert_eq!(kind, "attention.recorded");
    let got = (&payload["type"], &payload["note"]);
    assert_eq!(got, (&json!("stopped"), &json!("rate_limit")));
}

#[test]
fn the_shim_is_silent_without_the_binary() {
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/hooks/chaperone-hook.sh");
    for work in [Some("1"), None] {
        let mut cmd = Command::new(&shim);
        cmd.arg("stop").env("PATH", "/usr/bin:/bin");
        if let Some(work) = work {
            cmd.env("CHAPERONE_WORK", work);
        } else {
            cmd.env_remove("CHAPERONE_WORK");
        }
        let out = cmd.stdin(Stdio::null()).output().unwrap();
        assert!(out.status.success(), "{work:?}: {out:?}");
        assert_eq!((out.stdout.len(), out.stderr.len()), (0, 0), "{work:?}");
    }
}
