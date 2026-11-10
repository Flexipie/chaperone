//! `runner::Agent::claude` against the scripted agent.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::runner::{Agent, Ask, RunError};
use serde_json::{Value, json};

/// A result as Claude Code 2.1 prints it, less the fields not read.
fn result(structured: Value) -> String {
    json!({
        "type": "result", "subtype": "success", "is_error": false, "num_turns": 2,
        "result": structured.to_string(), "structured_output": structured,
        "session_id": "95f586ad", "total_cost_usd": 0.0015,
        "modelUsage": {"claude-haiku-5-5": {"inputTokens": 2, "outputTokens": 74, "costUSD": 0.0015}},
    })
    .to_string()
}

/// The fake linked as `claude` in a new directory, with `runs` as its
/// script, started from an environment that has a lane's variables.
fn agent(runs: Value) -> (tempfile::TempDir, Agent) {
    let dir = tempfile::tempdir().unwrap();
    let script = json!({"version": "9.9.9 (fake)\nmore", "runs": runs});
    let agent = probe(dir.path(), Some(script)).unwrap();
    (dir, agent)
}

fn probe(dir: &Path, script: Option<Value>) -> Result<Agent, RunError> {
    let bin = dir.join("claude");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_chaperone-fake"), &bin).unwrap();
    if let Some(script) = script {
        std::fs::write(dir.join("claude.json"), script.to_string()).unwrap();
    }
    let lane = [
        "CHAPERONE_WORK",
        "CLAUDECODE",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "HERDR_PANE_ID",
    ];
    let lane = lane.map(OsString::from).into_iter();
    // A name that is not UTF-8 is still the lane's.
    let odd = OsString::from_vec(b"DAISY_\xff".to_vec());
    let env = lane.chain([odd, "PATH".into(), "CLAUDE_CONFIG_DIR".into()]);
    Agent::probe(bin, env.map(|k| (k, "x".into())), soon())
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(20)
}

fn ask<'a>(prompt: &'a str, schema: &'a Value, dir: &'a Path) -> Ask<'a> {
    Ask {
        prompt,
        schema,
        model: "opus",
        tools: &["Read", "Grep"],
        dir,
        budget_cents: 150,
    }
}

/// The calls the fake logged, oldest first.
fn calls(dir: &Path) -> Vec<Value> {
    let log = std::fs::read_to_string(dir.join("claude.log")).unwrap();
    log.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// An answer is the CLI's `structured_output`, with its cost and usage;
/// the session got the prompt on stdin, the flags of a fresh read-only
/// sandboxed run, the run's directory, and none of the lane's variables.
#[test]
fn an_answer_is_read_from_a_fresh_session() {
    let found = json!({"findings": []});
    let (dir, agent) = agent(json!([{"when": "review this", "stdout": result(found.clone())}]));
    assert_eq!(agent.version, "9.9.9 (fake)");
    let schema = json!({"type": "object"});
    let long = format!("review this\n{}", "x".repeat(2 * 1024 * 1024));
    let answer = agent
        .claude(&ask(&long, &schema, dir.path()), soon())
        .unwrap();
    assert_eq!(answer.output, found);
    assert_eq!(answer.run.cost_usd, Some(0.0015));
    assert_eq!(
        answer.run.model_usage["claude-haiku-5-5"]["outputTokens"],
        74
    );
    assert_eq!(answer.run.session_id.as_deref(), Some("95f586ad"));
    assert_eq!(answer.run.raw, result(found));

    let call = calls(dir.path()).pop().unwrap();
    assert_eq!(call["prompt"], long);
    assert_eq!(
        PathBuf::from(call["cwd"].as_str().unwrap()),
        dir.path().canonicalize().unwrap()
    );
    let mut env: Vec<&str> = call["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    env.sort();
    assert_eq!(env, ["CLAUDE_CONFIG_DIR", "PATH"]);
    let args = [
        "-p",
        "--output-format",
        "json",
        "--json-schema",
        r#"{"type":"object"}"#,
        "--model",
        "opus",
        "--tools",
        "Read,Grep",
        "--permission-mode",
        "dontAsk",
        "--setting-sources",
        "user",
        "--settings",
        r#"{"sandbox":{"enabled":true,"failIfUnavailable":true,"allowUnsandboxedCommands":false,"network":{"strictAllowlist":true}}}"#,
        "--strict-mcp-config",
        "--no-session-persistence",
        "--max-budget-usd",
        "1.50",
    ];
    assert_eq!(call["args"], json!(args));
}

/// Every way a run gives no answer is an error that says which, never an
/// empty answer, and a failure the CLI reports keeps what it cost.
#[test]
fn a_run_with_no_answer_says_why() {
    let stopped = json!({"type": "result", "subtype": "error_max_budget_usd", "is_error": true,
        "structured_output": null, "total_cost_usd": 1.52, "session_id": "s",
        "errors": ["Reached maximum budget ($1.5)"]});
    let api = json!({"type": "result", "subtype": "success", "is_error": true,
        "result": "Invalid API key", "errors": null});
    let no_output = json!({"type": "result", "subtype": "success", "is_error": false});
    let (dir, agent) = agent(json!([
        {"when": "budget", "stdout": stopped.to_string(), "exit": 1},
        {"when": "api", "stdout": api.to_string(), "exit": 1},
        {"when": "crash", "stderr": "boom\n", "exit": 3},
        {"when": "prose", "stdout": "Here is my review."},
        {"when": "no output", "stdout": no_output.to_string()},
        {"when": "slow", "stdout": result(json!({})), "sleep_ms": 5000},
    ]));
    let schema = json!({});
    let run = |prompt: &str, deadline| {
        agent
            .claude(&ask(prompt, &schema, dir.path()), deadline)
            .unwrap_err()
    };
    let RunError::Failed {
        subtype,
        message,
        run: spent,
    } = run("budget", soon())
    else {
        panic!("a budget stop was not the CLI's failure");
    };
    assert_eq!(subtype, "error_max_budget_usd");
    assert_eq!(message, "Reached maximum budget ($1.5)");
    assert_eq!(
        (spent.cost_usd, spent.raw),
        (Some(1.52), stopped.to_string())
    );
    let said = "the session failed (success): Invalid API key";
    assert_eq!(run("api", soon()).to_string(), said);
    let crash = RunError::Exit {
        code: Some(3),
        stderr: "boom\n".into(),
    };
    assert_eq!(run("crash", soon()), crash);
    assert_eq!(
        run("prose", soon()),
        RunError::Output("not the CLI's JSON result".into())
    );
    assert_eq!(
        run("no output", soon()),
        RunError::Output("no structured_output".into())
    );
    let start = Instant::now();
    assert_eq!(
        run("slow", start + Duration::from_millis(300)),
        RunError::TimedOut
    );
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "waited for the child"
    );
    // A prompt the script does not know: the fake's own exit.
    assert!(matches!(
        run("unknown", soon()),
        RunError::Exit { code: Some(64), .. }
    ));
}

/// A CLI that is missing, or cannot say its version, is no agent; one
/// that answers before reading its whole prompt gave no answer.
#[test]
fn a_broken_cli_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let gone = Agent::probe(dir.path().join("nope"), [], soon()).unwrap_err();
    assert!(matches!(gone, RunError::Spawn(_)), "{gone:?}");
    // No script: the fake exits 64.
    let broken = probe(dir.path(), None).unwrap_err();
    assert!(
        matches!(broken, RunError::Exit { code: Some(64), .. }),
        "{broken:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    let runs = json!([{"when": "review", "stdout": result(json!({}))}]);
    let script = json!({"version": "1", "stdin_bytes": 6, "runs": runs});
    let agent = probe(dir.path(), Some(script)).unwrap();
    let long = format!("review {}", "x".repeat(2 * 1024 * 1024));
    let err = agent
        .claude(&ask(&long, &json!({}), dir.path()), soon())
        .unwrap_err();
    assert!(matches!(err, RunError::Io(_)), "{err:?}");
}
