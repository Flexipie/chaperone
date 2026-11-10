//! `runner::Agent::codex` against the scripted agent.

use std::path::Path;
use std::time::{Duration, Instant};

use chaperone_core::runner::{Agent, Ask, RunError};
use serde_json::{Value, json};

/// The fake linked as `codex` in a new directory, with `runs` as its script.
fn agent(runs: Value) -> (tempfile::TempDir, Agent) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("codex");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_chaperone-fake"), &bin).unwrap();
    let script = json!({"version": "codex-cli 0.165.0", "runs": runs});
    std::fs::write(dir.path().join("codex.json"), script.to_string()).unwrap();
    let agent = Agent::probe(bin, [("PATH".into(), "x".into())], soon()).unwrap();
    (dir, agent)
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(20)
}

fn ask<'a>(prompt: &'a str, schema: &'a Value, dir: &'a Path) -> Ask<'a> {
    Ask {
        prompt,
        schema,
        model: "gpt-5.5",
        tools: &[],
        dir,
        budget_cents: 0,
    }
}

/// The calls the fake logged, oldest first.
fn calls(dir: &Path) -> Vec<Value> {
    let log = std::fs::read_to_string(dir.join("codex.log")).unwrap();
    log.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Events as `codex exec --json` prints them, less the items.
const EVENTS: &str = concat!(
    r#"{"type":"thread.started","thread_id":"0199a213"}"#,
    "\n",
    r#"{"type":"turn.started"}"#,
    "\n",
    r#"{"type":"turn.completed","usage":{"input_tokens":24763,"cached_input_tokens":24448,"output_tokens":122}}"#,
    "\n",
);

/// An answer is the last message Codex wrote to its `-o` file, read under
/// the schema it was given as a file, from a read-only session that got
/// the prompt on stdin; the files are gone afterwards.
#[test]
fn an_answer_is_read_from_the_last_message() {
    let found = json!({"findings": []});
    let write = json!({"-o": found.to_string()});
    let (dir, agent) = agent(json!([{"when": "review this", "stdout": EVENTS, "write": write}]));
    let schema = json!({"type": "object"});
    let answer = agent
        .codex(&ask("review this", &schema, dir.path()), soon())
        .unwrap();
    assert_eq!(answer.output, found);
    assert_eq!(answer.run.raw, EVENTS);
    assert_eq!(answer.run.session_id.as_deref(), Some("0199a213"));
    assert_eq!(answer.run.model_usage["output_tokens"], 122);
    assert_eq!(answer.run.cost_usd, None);

    let call = calls(dir.path()).pop().unwrap();
    assert_eq!(call["prompt"], "review this");
    let mut args: Vec<&str> = call["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let (schema_at, last_at) = (Path::new(args[10]), Path::new(args[12]));
    assert_eq!(call["files"][args[10]], r#"{"type":"object"}"#);
    let scratch = schema_at.parent().unwrap();
    assert_eq!(last_at.parent(), Some(scratch));
    (args[10], args[12]) = ("<schema>", "<last>");
    let here = dir.path().to_str().unwrap();
    let flags = [
        "exec",
        "--json",
        "--ephemeral",
        "-s",
        "read-only",
        "-m",
        "gpt-5.5",
        "-C",
        here,
        "--output-schema",
        "<schema>",
        "-o",
        "<last>",
        "-",
    ];
    assert_eq!(args, flags);
    assert!(scratch.starts_with(std::env::temp_dir()));
    assert!(!scratch.exists(), "the run's files were left behind");
}

/// A run that exits non-zero, fails its turn, times out, or leaves no
/// JSON in its file gave no answer, and says which; a written file never
/// makes a failed run an answer.
#[test]
fn a_run_with_no_answer_says_why() {
    let answer = json!({"-o": "{}"});
    let failed = r#"{"type":"turn.failed","error":{"message":"stream disconnected"}}"#;
    let (dir, agent) = agent(json!([
        {"when": "crash", "stderr": "boom", "exit": 1, "write": answer},
        {"when": "turn", "stdout": failed, "exit": 1, "write": answer},
        {"when": "missing", "stdout": EVENTS},
        {"when": "empty", "write": {"-o": ""}},
        {"when": "prose", "write": {"-o": "Here is my review."}},
        {"when": "slow", "sleep_ms": 5000, "write": answer},
    ]));
    let schema = json!({});
    let run = |prompt: &str, deadline| {
        agent
            .codex(&ask(prompt, &schema, dir.path()), deadline)
            .unwrap_err()
    };
    let crash = RunError::Exit {
        code: Some(1),
        stderr: "boom".into(),
    };
    assert_eq!(run("crash", soon()), crash);
    let RunError::Failed {
        subtype,
        message,
        run: spent,
    } = run("turn", soon())
    else {
        panic!("a failed turn was not the CLI's failure");
    };
    assert_eq!(
        (&*subtype, &*message),
        ("turn.failed", "stream disconnected")
    );
    assert_eq!(spent.raw, failed);
    for prompt in ["missing", "empty", "prose"] {
        let err = run(prompt, soon());
        assert!(matches!(err, RunError::Output(_)), "{prompt}: {err:?}");
    }
    let start = Instant::now();
    assert_eq!(
        run("slow", start + Duration::from_millis(300)),
        RunError::TimedOut
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    for call in calls(dir.path()) {
        let schema = Path::new(call["args"][10].as_str().unwrap());
        assert!(!schema.parent().unwrap().exists(), "{call}");
    }
}
