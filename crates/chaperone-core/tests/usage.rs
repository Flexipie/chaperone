//! `usage report` and `usage get` against transcripts built in a temp dir:
//! one event per final message, a rerun writes nothing, sums keep R11's
//! null rule, and a subagent's description is redacted at write.

mod common;

use std::fs;
use std::path::Path;

use chaperone_core::ledger::{Applied, EventKind, Ledger, Mutation, Payload, Refusal};
use chaperone_core::runner::Run;
use chaperone_core::usage::Roots;
use chaperone_core::work::{ClaimInput, CreateInput};
use common::{actor, count, read_only};
use serde_json::{Map, Value, json};

const WT: &str = "/work/lane";

fn assistant(id: &str, usage: Value, stop: Option<&str>) -> Value {
    json!({"type": "assistant", "message": {"id": id, "model": "claude-x", "usage": usage,
           "stop_reason": stop, "content": [{"type": "text", "text": "not stored"}]}})
}

fn u(input: u64, read: u64, write: u64, output: u64) -> Value {
    json!({"input_tokens": input, "cache_read_input_tokens": read,
           "cache_creation_input_tokens": write, "output_tokens": output})
}

fn write_lines(path: &Path, lines: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(path, text).unwrap();
}

/// A Claude home with one lane session (four messages, the last still
/// streaming), a subagent, and a session from another worktree; a Codex
/// home with one lane rollout and one without usage records.
fn homes(root: &Path) -> Roots {
    let claude = root.join("claude");
    let project = claude.join("projects").join("-work-lane");
    let cwd = json!({"type": "user", "cwd": WT, "message": {"content": "not stored"}});
    let mut main = vec![
        cwd.clone(),
        // Streamed: the first block's usage is partial, the stop line's is final.
        assistant("msg_a", u(2, 100, 10, 1), None),
        assistant("msg_a", u(2, 100, 10, 50), Some("tool_use")),
        json!({"type": "user", "message": {"content": [{"type": "tool_result"}]}}),
        // No stop, but a later message began: final.
        assistant("msg_b", u(3, 200, 20, 30), None),
        // No cache write key: input_total is null, never a fake zero.
        assistant(
            "msg_c",
            json!({"input_tokens": 4, "cache_read_input_tokens": 300, "output_tokens": 5}),
            Some("end_turn"),
        ),
        // Still streaming: left for the next report.
        assistant("msg_d", u(1, 1, 1, 1), None),
    ];
    write_lines(&project.join("s1.jsonl"), &main);
    fs::OpenOptions::new()
        .append(true)
        .open(project.join("s1.jsonl"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"{\"type\":\"assistant\", broken\n"))
        .unwrap();
    let sub = project.join("s1").join("subagents");
    write_lines(
        &sub.join("agent-x1.jsonl"),
        &[assistant("msg_s1", u(1, 50, 5, 7), Some("end_turn"))],
    );
    let token = format!("ghp_{}", "a".repeat(36));
    fs::write(
        sub.join("agent-x1.meta.json"),
        json!({"agentType": "reviewer", "description": format!("Round 1 review {token}")})
            .to_string(),
    )
    .unwrap();
    // Another worktree that shares the prefix: not this lane's.
    main[0] = json!({"type": "user", "cwd": "/work/lane-2"});
    write_lines(
        &claude
            .join("projects")
            .join("-work-lane-2")
            .join("s2.jsonl"),
        &main,
    );

    let codex = root.join("codex");
    let day = codex.join("sessions").join("2026").join("11").join("05");
    write_lines(
        &day.join("rollout-1.jsonl"),
        &[
            json!({"type": "session_meta", "payload": {"id": "c1", "cwd": format!("{WT}/sub")}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-x"}}),
            json!({"type": "token_usage_record", "payload": {"response_id": "resp_1", "usage": {
                "input_tokens": 100, "cached_input_tokens": 60, "cache_write_input_tokens": 0,
                "output_tokens": 10, "reasoning_output_tokens": 4, "total_tokens": 110}}}),
        ],
    );
    // A Codex subagent: its own rollout, naming the parent thread.
    write_lines(
        &day.join("rollout-3.jsonl"),
        &[
            json!({"type": "session_meta", "payload": {"id": "c3", "cwd": WT,
                "parent_thread_id": "c1", "agent_nickname": "Hume",
                "source": {"subagent": {"thread_spawn": {"agent_role": "explorer"}}}}}),
            json!({"type": "token_usage_record", "payload": {"response_id": "resp_3", "usage": {
                "input_tokens": 20, "cached_input_tokens": 0, "output_tokens": 2}}}),
        ],
    );
    // A stray file next to the project folders is not a project.
    fs::write(claude.join("projects").join(".DS_Store"), b"").unwrap();
    write_lines(
        &day.join("rollout-2.jsonl"),
        &[json!({"type": "session_meta", "payload": {"id": "c2", "cwd": WT}})],
    );
    Roots {
        claude: Some(claude),
        codex: Some(codex),
    }
}

fn lane(ledger: &Ledger, worktree: Option<&str>) -> i64 {
    let a = actor("1");
    let input = CreateInput {
        title: "t".into(),
        source: Some("manual".into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let id = ledger
        .work_create(&a, &input, None, false)
        .unwrap()
        .unwrap()
        .id;
    let claim = ClaimInput {
        worktree_path: worktree.map(String::from),
        ..ClaimInput::default()
    };
    ledger
        .work_claim(&a, id, &claim, None, false)
        .unwrap()
        .unwrap();
    id
}

const USAGE: &str = "SELECT count(*) FROM events WHERE kind = 'usage.reported'";

#[test]
fn report_writes_final_messages_once() {
    let (dir, path, ledger) = common::ledger();
    let roots = homes(dir.path());
    let id = lane(&ledger, Some(&format!("{WT}/")));
    let lane_events = ledger.work_events(id).unwrap().len();
    let first = ledger
        .usage_report(&actor("2"), id, &roots)
        .unwrap()
        .unwrap();
    // msg_a, msg_b, msg_c, the subagent's msg_s1, Codex's resp_1 and its
    // subagent's resp_3.
    assert_eq!((first.written, first.existing, first.pending), (6, 0, 1));
    let unknown: Vec<_> = first.sessions.iter().filter(|s| s.unknown).collect();
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].session_id, "c2");

    let again = ledger
        .usage_report(&actor("2"), id, &roots)
        .unwrap()
        .unwrap();
    assert_eq!((again.written, again.existing), (0, 6));
    let db = read_only(&path);
    assert_eq!(count(&db, USAGE), 6);
    // No message text reaches the log.
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM events WHERE payload_json LIKE '%not stored%'"
        ),
        0
    );

    // msg_d finishes: only it is added.
    let s1 = dir.path().join("claude/projects/-work-lane/s1.jsonl");
    let mut text = fs::read_to_string(&s1).unwrap();
    text.push_str(&format!(
        "{}\n",
        assistant("msg_d", u(1, 1, 1, 9), Some("end_turn"))
    ));
    fs::write(&s1, text).unwrap();
    let later = ledger
        .usage_report(&actor("2"), id, &roots)
        .unwrap()
        .unwrap();
    assert_eq!((later.written, later.existing, later.pending), (1, 6, 0));
    assert_eq!(count(&db, USAGE), 7);
    // A report is not lane work: work events shows none of it.
    assert_eq!(ledger.work_events(id).unwrap().len(), lane_events);
    common::assert_consistent(&path);
}

#[test]
fn get_sums_per_source_and_transcript() {
    let (dir, _path, ledger) = common::ledger();
    let roots = homes(dir.path());
    let id = lane(&ledger, Some(WT));
    assert_eq!(ledger.usage_get(id).unwrap().unwrap()["known"], false);
    ledger
        .usage_report(&actor("2"), id, &roots)
        .unwrap()
        .unwrap();
    let got = ledger.usage_get(id).unwrap().unwrap();
    assert_eq!(got["known"], true);
    let parent = &got["totals"]["parent"];
    // Claude msg_a, msg_b, msg_c plus Codex resp_1.
    assert_eq!(parent["messages"], 4);
    assert_eq!(parent["output"], 50 + 30 + 5 + 10);
    // msg_c's input_total is null and skipped; Codex's is its input_tokens.
    assert_eq!(parent["input_total"], 112 + 223 + 100);
    assert_eq!(parent["peak_input_total"], 223);
    assert_eq!(parent["reasoning"], 4);
    let sub = &got["totals"]["subagent"];
    // Claude's msg_s1 and the Codex subagent's resp_3.
    assert_eq!(
        (sub["messages"].clone(), sub["input_total"].clone()),
        (json!(2), json!(56 + 20))
    );

    let sources = got["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 4);
    let spawned = sources.iter().find(|s| s["agent_id"] == "c3").unwrap();
    assert_eq!(
        (spawned["session_id"].clone(), spawned["agent_type"].clone()),
        (json!("c1"), json!("explorer"))
    );
    assert_eq!(spawned["description"], "Hume");
    let agent = sources.iter().find(|s| s["agent_id"] == "x1").unwrap();
    assert_eq!(agent["agent_type"], "reviewer");
    assert_eq!(
        agent["description"],
        "Round 1 review [redacted:github_token]"
    );
    let claude = sources
        .iter()
        .find(|s| s["session_id"] == "s1" && s["agent_id"].is_null());
    let claude = claude.unwrap();
    assert_eq!(claude["cache_write"], 30);
    assert_eq!(claude["models"], "claude-x");
    let codex = sources
        .iter()
        .find(|s| s["cli"] == "codex" && s["source"] == "parent")
        .unwrap();
    assert_eq!(
        (codex["cache_read"].clone(), codex["models"].clone()),
        (json!(60), json!("gpt-x"))
    );
}

#[test]
fn refusals_write_nothing() {
    let (dir, path, ledger) = common::ledger();
    let roots = homes(dir.path());
    let a = actor("2");
    let bare = lane(&ledger, None);
    assert!(matches!(
        ledger.usage_report(&a, bare, &roots).unwrap(),
        Err(Refusal::InvalidInput(m)) if m.contains("no worktree_path")
    ));
    // `/` (trimmed to nothing) or a relative path (which only an older
    // binary's claim could store) would match every transcript or none.
    for wt in ["/", "wt"] {
        let odd = lane(&ledger, None);
        let sql = "UPDATE lanes SET worktree_path = ?2 WHERE id = ?1";
        let conn = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(conn.execute(sql, rusqlite::params![odd, wt]).unwrap(), 1);
        assert!(matches!(
            ledger.usage_report(&a, odd, &roots).unwrap(),
            Err(Refusal::InvalidInput(m)) if m.contains("absolute folder")
        ));
    }
    let id = lane(&ledger, Some(WT));
    let typo = Roots {
        claude: Some(dir.path().join("nope")),
        codex: None,
    };
    assert!(matches!(
        ledger.usage_report(&a, id, &typo).unwrap(),
        Err(Refusal::InvalidInput(m)) if m.contains("is not a folder")
    ));
    assert!(matches!(
        ledger.usage_report(&a, id, &Roots::default()).unwrap(),
        Err(Refusal::InvalidInput(_))
    ));
    assert!(matches!(
        ledger.usage_report(&a, 99, &roots).unwrap(),
        Err(Refusal::NotFound { .. })
    ));
    // The write itself refuses a usage event it could not dedupe.
    let bare = Mutation::Event {
        kind: EventKind::UsageReported,
        lane_id: Some(id),
        spec_id: None,
        payload: Payload::default(),
    };
    assert!(matches!(
        ledger.apply(&a, None, common::AT, bare).unwrap(),
        Applied::Refused(Refusal::InvalidInput(m)) if m.contains("message_id")
    ));
    assert_eq!(count(&read_only(&path), USAGE), 0);
}

#[test]
fn an_idle_transcript_counts_its_last_message() {
    let (dir, _path, ledger) = common::ledger();
    let roots = homes(dir.path());
    let id = lane(&ledger, Some(WT));
    // msg_d never stopped (an interrupt), and nothing has written for an hour.
    let s1 = dir.path().join("claude/projects/-work-lane/s1.jsonl");
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&s1)
        .and_then(|f| f.set_modified(hour_ago))
        .unwrap();
    let got = ledger
        .usage_report(&actor("2"), id, &roots)
        .unwrap()
        .unwrap();
    assert_eq!((got.written, got.pending), (7, 0));
}

/// A session the harness ran is one row, source and origin `harness`,
/// keyed by its session id: a retry writes nothing, its counts keep no
/// string, its tokens are Claude's summed over models or Codex's, and a
/// session that left no id writes nothing.
#[test]
fn a_harness_session_is_one_row() {
    let (_dir, path, ledger) = common::ledger();
    let id = lane(&ledger, None);
    let usage = json!({"claude-x": {"inputTokens": 3, "cacheReadInputTokens": 100,
        "cacheCreationInputTokens": 10, "outputTokens": 2, "costUSD": 0.5, "note": "not stored"},
        "claude-y": {"inputTokens": 1, "cacheReadInputTokens": 0,
        "cacheCreationInputTokens": 5, "outputTokens": 4}});
    let run = Run {
        raw: "not stored".into(),
        cost_usd: Some(0.5),
        model_usage: usage,
        session_id: Some("sess".into()),
    };
    let labels = Map::from_iter([
        ("phase".into(), json!("review")),
        ("cli".into(), json!("claude")),
    ]);
    let codex = Run {
        raw: String::new(),
        cost_usd: None,
        model_usage: json!({"input_tokens": 50, "cached_input_tokens": 40, "output_tokens": 6}),
        session_id: Some("thread".into()),
    };
    let no_id = Run {
        raw: String::new(),
        cost_usd: Some(1.0),
        model_usage: Value::Null,
        session_id: None,
    };
    let mut codex_labels = labels.clone();
    codex_labels.insert("cli".into(), json!("codex"));
    let runs = [
        (&run, &labels),
        (&run, &labels),
        (&codex, &codex_labels),
        (&no_id, &labels),
    ];
    for (run, labels) in runs {
        let written = ledger.usage_run(&actor("1"), id, labels.clone(), run);
        written.unwrap().unwrap();
    }
    let db = read_only(&path);
    assert_eq!(count(&db, USAGE), 2);
    let payload = |cli: &str| -> Value {
        let sql = "SELECT payload_json FROM events WHERE kind = 'usage.reported'
                   AND json_extract(payload_json, '$.cli') = ?1";
        let text: String = db.query_row(sql, [cli], |r| r.get(0)).unwrap();
        serde_json::from_str(&text).unwrap()
    };
    let mut raw = run.model_usage.clone();
    raw["claude-x"].as_object_mut().unwrap().remove("note");
    let want = json!({"source": "harness", "phase": "review", "cli": "claude",
        "message_id": "run:sess", "origin": "harness", "session_id": "sess", "cost_usd": 0.5,
        "raw": raw, "uncached_input": 4, "cache_read": 100, "cache_write": 15, "output": 6,
        "reasoning": null, "input_includes_cache": false, "output_includes_reasoning": true,
        "input_total": 119, "output_total": 6});
    assert_eq!(payload("claude"), want);
    let tokens = ["input_total", "cache_read", "cache_write", "output_total"];
    let got = tokens.map(|k| payload("codex")[k].clone());
    assert_eq!(got, [json!(50), json!(40), Value::Null, json!(6)]);
    let got = ledger.usage_get(id).unwrap().unwrap();
    let harness = &got["totals"]["harness"];
    assert_eq!(
        (
            &harness["messages"],
            &harness["cost_usd"],
            &harness["input_total"]
        ),
        (&json!(2), &json!(0.5), &json!(169))
    );
}
