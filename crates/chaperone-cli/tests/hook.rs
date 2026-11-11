//! `hook` through the binary, on lane 1 claimed on v20, and the plugin's
//! shim. The stop hook's test is verify.rs's, on a lane verify made ready.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

struct Lane {
    dir: tempfile::TempDir,
    db: PathBuf,
}

impl Lane {
    fn new() -> Lane {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("ledger.db");
        let lane = Lane { dir, db };
        lane.cli(&["migrate"]);
        lane.cli(&["work", "create", "--title", "t"]);
        lane.cli(&["work", "claim", "1", "--pipeline", "v20"]);
        lane
    }

    /// A command as the lane's session, which must succeed.
    fn cli(&self, args: &[&str]) {
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .envs([("DAISY_AGENT", "lane"), ("DAISY_SESSION", "lane")])
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
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

/// Unset, a cancelled lane and a v19 lane. The runner's children never see
/// the lane: `CHAPERONE_WORK` is among the variables chaperone-fake's
/// `an_answer_is_read_from_a_fresh_session` asserts a headless session
/// does not get.
#[test]
fn a_hook_outside_a_lane_does_nothing() {
    let lane = Lane::new();
    lane.cli(&["work", "create", "--title", "u"]);
    lane.cli(&["work", "cancel", "2"]);
    lane.cli(&["work", "create", "--title", "v"]);
    lane.cli(&["work", "claim", "3"]);
    let before = lane.events();
    let input = json!({"error": "rate_limit", "tool_input": {"command": "gh pr create"}});
    for event in ["session-start", "pre-bash", "stop", "stop-failure"] {
        for work in [None, Some("2"), Some("3")] {
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
    for want in ["work 1", "pipeline v20", "tests_passed", "verify --work 1"] {
        assert!(context.contains(want), "{want}: {context}");
    }
}

#[test]
fn pre_bash_denies_what_the_binary_owns() {
    let lane = Lane::new();
    let bash = |command: &str| json!({"tool_input": {"command": command}});
    let (open, merge, ready) = ("chaperone pr open --work 1", "chaperone merge", "override");
    // An empty reason: not denied.
    for (command, reason) in [
        ("gh pr create --fill", open),
        ("url=$(gh pr create --fill)", open),
        ("(gh pr create)", open),
        ("cd wt && GH_DEBUG=1 gh pr merge 5 --squash", merge),
        ("chaperone work ready 1 --yes;", ready),
        ("gh pr view \"$PR\" | grep merge", ""),
        ("echo gh pr create", ""),
        ("git commit -m \"no gh pr create here\"", ""),
        ("gh pr view 5", ""),
        ("git status", ""),
    ] {
        let out = lane.hook(Some("1"), "pre-bash", bash(command));
        if reason.is_empty() {
            assert_eq!(out, "", "{command}");
            continue;
        }
        let out = &serde_json::from_str::<Value>(&out).unwrap()["hookSpecificOutput"];
        assert_eq!(out["permissionDecision"], "deny", "{command}");
        let why = out["permissionDecisionReason"].as_str().unwrap();
        assert!(why.contains(reason), "{command}: {why}");
    }
}

#[test]
fn stop_failure_records_again_only_after_the_lane_moves() {
    let lane = Lane::new();
    let before = lane.events().len();
    let stopped = || lane.hook(Some("1"), "stop-failure", json!({"error": "rate_limit"}));
    assert_eq!(stopped(), "");
    assert_eq!(stopped(), "");
    lane.cli(&["work", "release", "1"]);
    assert_eq!(stopped(), "");
    let events = lane.events();
    let kinds: Vec<_> = events[before..].iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        ["attention.recorded", "lane.released", "attention.recorded"]
    );
    let payload = &events[before].1;
    let got = (&payload["type"], &payload["note"]);
    assert_eq!(got, (&json!("stopped"), &json!("rate_limit")));
}

/// Silent unless the lane is set and `chaperone` answers with exit 0; its
/// stdin passes through and its stderr never shows.
#[test]
fn the_shim_is_silent_unless_chaperone_answers() {
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/hooks/chaperone-hook.sh");
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("chaperone");
    std::fs::write(&fake, "#!/bin/sh\ncat\necho err >&2\nexit $FAKE_EXIT\n").unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&fake, mode).unwrap();
    let with = format!("{}:/usr/bin:/bin", dir.path().display());
    for (work, path, exit, want) in [
        (None, with.as_str(), "0", ""),
        (Some("1"), "/usr/bin:/bin", "0", ""),
        (Some("1"), &with, "1", ""),
        (Some("1"), &with, "0", "in\n"),
    ] {
        let mut cmd = Command::new(&shim);
        cmd.arg("stop").env("PATH", path).env("FAKE_EXIT", exit);
        match work {
            Some(work) => cmd.env("CHAPERONE_WORK", work),
            None => cmd.env_remove("CHAPERONE_WORK"),
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"in").unwrap();
        let out = child.wait_with_output().unwrap();
        let got = (out.status.code(), &*String::from_utf8_lossy(&out.stdout));
        assert_eq!(
            (got, out.stderr.len()),
            ((Some(0), want), 0),
            "{work:?} {path} {exit}"
        );
    }
}

/// `stop` records the lane session's tokens from its transcript, once.
#[test]
fn stop_reports_the_lanes_usage() {
    let lane = Lane::new();
    let wt = lane.dir.path().join("wt");
    let project = lane.dir.path().join("projects/p");
    std::fs::create_dir_all(&project).unwrap();
    let cwd = wt.to_str().unwrap();
    lane.cli(&["work", "create", "--title", "u"]);
    lane.cli(&["work", "claim", "2", "--pipeline", "v20", "--worktree", cwd]);
    let usage = json!({"input_tokens": 5, "cache_read_input_tokens": 7,
        "cache_creation_input_tokens": 1, "output_tokens": 2});
    let lines = [
        json!({"type": "user", "cwd": cwd}),
        json!({"type": "assistant", "message": {"id": "m1", "model": "claude-x",
            "usage": usage, "stop_reason": "end_turn"}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(project.join("s1.jsonl"), text).unwrap();
    let input = json!({"transcript_path": project.join("s1.jsonl")});
    for _ in 0..2 {
        assert_eq!(lane.hook(Some("2"), "stop", input.clone()), "");
    }
    let usage: Vec<Value> = lane
        .events()
        .into_iter()
        .filter(|(k, _)| k == "usage.reported")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(usage.len(), 1);
    assert_eq!(
        (&usage[0]["source"], &usage[0]["input_total"]),
        (&json!("parent"), &json!(13))
    );
}
