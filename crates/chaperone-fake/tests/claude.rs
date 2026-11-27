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
        "CODEX_THREAD_ID",
        "HERDR_PANE_ID",
    ];
    let lane = lane.map(OsString::from).into_iter();
    // A name that is not UTF-8 is still the lane's.
    let odd = OsString::from_vec(b"DAISY_\xff".to_vec());
    let keep = ["PATH", "CLAUDE_CONFIG_DIR", "CODEX_HOME"].map(OsString::from);
    let env = lane.chain([odd]).chain(keep);
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
        max_tokens: 1000,
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
        // Set by llvm's profile runtime in the fake itself under the coverage job.
        .filter(|k| *k != "__LLVM_PROFILE_RT_INIT_ONCE")
        .collect();
    env.sort();
    assert_eq!(env, ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "PATH"]);
    let args = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
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
        r#"{"sandbox":{"enabled":true,"failIfUnavailable":true,"autoAllowBashIfSandboxed":false,"allowUnsandboxedCommands":false,"network":{"strictAllowlist":true}}}"#,
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--no-session-persistence",
    ];
    assert_eq!(call["args"], json!(args));
}

/// Every way a run gives no answer is an error that says which, never an
/// empty answer, and a failure the CLI reports keeps what it cost.
#[test]
fn a_run_with_no_answer_says_why() {
    let stopped = json!({"type": "result", "subtype": "error_max_turns", "is_error": true,
        "structured_output": null, "session_id": "s", "errors": ["Reached maximum turns (3)"]});
    let api = json!({"type": "result", "subtype": "success", "is_error": true,
        "result": "Invalid API key", "errors": null});
    let no_output = json!({"type": "result", "subtype": "success", "is_error": false});
    let (dir, agent) = agent(json!([
        {"when": "turns", "stdout": stopped.to_string(), "exit": 1},
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
    } = run("turns", soon())
    else {
        panic!("a turns stop was not the CLI's failure");
    };
    assert_eq!(subtype, "error_max_turns");
    assert_eq!(message, "Reached maximum turns (3)");
    assert_eq!(spent.raw, stopped.to_string());
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

/// A session past its tokens is stopped where it is, not at its end, and
/// what it used so far is kept, each message counted once.
#[test]
fn a_session_past_its_tokens_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let message = |id: &str, read: u64| {
        json!({"type": "assistant", "message": {"id": id, "model": "m", "usage": {
            "input_tokens": 1, "cache_read_input_tokens": read,
            "cache_creation_input_tokens": 2, "output_tokens": 3}}})
    };
    let init = json!({"type": "system", "subtype": "init", "session_id": "s1"});
    // b's start is under the cap; its delta's final output passes it.
    let usage = |output: u64| {
        json!({"input_tokens": 1, "cache_read_input_tokens": 580,
            "cache_creation_input_tokens": 2, "output_tokens": output})
    };
    let begin = json!({"type": "stream_event", "event": {"type": "message_start",
        "message": {"id": "b", "model": "m", "usage": usage(0)}}});
    let delta = json!({"type": "stream_event", "event": {"type": "message_delta",
        "usage": usage(23)}});
    let stream = [
        init,
        // A later line with less is partial: each kind keeps its largest.
        message("a", 400),
        message("a", 100),
        begin,
        message("b", 580),
        delta,
    ];
    let lines: Vec<String> = stream.iter().map(Value::to_string).collect();
    let script = format!(
        "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo '1 (sh)'; exit 0; }}\n\
         cat > /dev/null\ncat <<'EOF'\n{}\nEOF\nsleep 30\n",
        lines.join("\n")
    );
    let bin = dir.path().join("claude");
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap();
    let agent = Agent::probe(bin, [("PATH".into(), path)], soon()).unwrap();
    let start = Instant::now();
    let schema = json!({});
    let err = agent
        .claude(&ask("go", &schema, dir.path()), soon())
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "waited for its end"
    );
    let RunError::Failed {
        subtype,
        message,
        run,
    } = err
    else {
        panic!("{err}");
    };
    assert_eq!(
        (subtype.as_str(), message.as_str()),
        ("max_tokens", "used 1012 tokens, past its cap of 1000")
    );
    assert_eq!(run.session_id.as_deref(), Some("s1"));
    let usage = json!({"m": {"inputTokens": 2, "cacheReadInputTokens": 980,
        "cacheCreationInputTokens": 4, "outputTokens": 26}});
    assert_eq!(run.model_usage, usage);
}

#[test]
fn a_line_without_end_is_cut() {
    let dir = tempfile::tempdir().unwrap();
    let script = "#!/bin/sh\n[ \"$1\" = --version ] && { echo '1 (sh)'; exit 0; }\n\
                  cat > /dev/null\nhead -c 17000000 /dev/zero | tr '\\0' a\nsleep 30\n";
    let bin = dir.path().join("claude");
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap();
    let agent = Agent::probe(bin, [("PATH".into(), path)], soon()).unwrap();
    let start = Instant::now();
    let schema = json!({});
    let err = agent
        .claude(&ask("go", &schema, dir.path()), soon())
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "waited for its end"
    );
    assert!(
        matches!(&err, RunError::Output(m) if m.contains("past")),
        "{err}"
    );
}
