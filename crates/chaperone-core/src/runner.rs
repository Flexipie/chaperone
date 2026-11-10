//! Headless agent runs for the checks the harness runs itself: one fresh
//! `claude -p` or `codex exec` session per question, answering in JSON
//! under a schema.
//!
//! The agent binary and the environment are the caller's arguments, never
//! read here, and the known variables of the session that started the run
//! are not passed on: a reviewer is no lane (the lane hooks stay quiet),
//! and it is not handed the socket, pane or history of the session it
//! reviews. The list is a speed bump, as every control on the same OS user
//! is; the sandbox settings are the next one.
//!
//! A session runs in a process group of its own ([`child::run_input`]), so
//! a Ctrl-C on chaperone does not reach it: it runs on to its own end.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;

use crate::child::{self, Failed};

/// Variables of the starting session, by name and by prefix.
const SESSION_VARS: [&str; 14] = [
    "AI_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
    "TMUX",
    "TMUX_PANE",
];
const SESSION_PREFIXES: [&str; 5] = ["ATUIN_", "CHAPERONE_", "CODEX_", "DAISY_", "HERDR_"];
/// Kept of the prefixes above: how Codex logs in.
const SESSION_KEEP: [&str; 4] = [
    "CODEX_ACCESS_TOKEN",
    "CODEX_API_KEY",
    "CODEX_CA_CERTIFICATE",
    "CODEX_HOME",
];
/// An answer past this is refused, not cut: half a JSON object is none.
const OUT_BYTES: usize = 16 * 1024 * 1024;
/// Of a failed run's stderr, its end.
const ERR_CHARS: usize = 2000;
/// Claude Code's sandbox, as its settings reference names the keys
/// (code.claude.com/docs/en/settings-reference, `sandbox`): a sandbox that
/// cannot start stops the session instead of running Bash without one,
/// and a sandboxed command still needs an allow rule, as `dontAsk` alone
/// would let the sandbox approve it.
const SANDBOX: &str = r#"{"sandbox":{"enabled":true,"failIfUnavailable":true,"autoAllowBashIfSandboxed":false,"allowUnsandboxedCommands":false,"network":{"strictAllowlist":true}}}"#;

/// The agent CLI a run starts: the path and version go in the event.
#[derive(Debug)]
pub struct Agent {
    pub bin: PathBuf,
    /// The first line of `<bin> --version`.
    pub version: String,
    env: Vec<(OsString, OsString)>,
}

/// One question. `tools` are the built-in tools the session may use (none
/// is a session that only reads its prompt), `dir` where it runs.
pub struct Ask<'a> {
    pub prompt: &'a str,
    pub schema: &'a Value,
    pub model: &'a str,
    pub tools: &'a [&'a str],
    pub dir: &'a Path,
    pub budget_cents: u32,
}

/// One question for Codex, which takes no tool list and no budget: the
/// deadline is its only bound. With no `model`, Codex picks its default.
pub struct CodexAsk<'a> {
    pub prompt: &'a str,
    pub schema: &'a Value,
    pub model: Option<&'a str>,
    pub dir: &'a Path,
}

/// What a session left behind, answered or not: a failed run still cost.
#[derive(Debug, PartialEq)]
pub struct Run {
    /// The CLI's whole result, for the evidence store.
    pub raw: String,
    pub cost_usd: Option<f64>,
    /// Claude's `modelUsage` (tokens and cost per model), or the `usage`
    /// of Codex's turn (tokens only).
    pub model_usage: Value,
    pub session_id: Option<String>,
}

#[derive(Debug)]
pub struct Answer {
    /// The answer: Claude Code checks it against the schema, Codex hands
    /// the schema to the model.
    pub output: Value,
    pub run: Run,
}

/// Why a run gave no answer. Never "nothing found": the caller records
/// the check as not run.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum RunError {
    #[error("could not start {0}")]
    Spawn(String),
    #[error("timed out")]
    TimedOut,
    #[error("{0}")]
    Io(String),
    /// The CLI's own result says it failed: `subtype` is its word
    /// (`error_max_budget_usd`, `error_max_turns`, or `success` on an API
    /// error; Codex's `turn.failed`), `message` what it gave as the reason.
    #[error("the session failed ({subtype}): {message}")]
    Failed {
        subtype: String,
        message: String,
        run: Box<Run>,
    },
    #[error("exit {code:?}: {stderr}")]
    Exit { code: Option<i32>, stderr: String },
    #[error("unreadable answer: {0}")]
    Output(String),
}

/// The fields read of `claude -p --output-format json`.
#[derive(Deserialize)]
struct ClaudeResult {
    #[serde(default)]
    is_error: bool,
    subtype: Option<String>,
    errors: Option<Vec<Value>>,
    result: Option<String>,
    structured_output: Option<Value>,
    total_cost_usd: Option<f64>,
    #[serde(default, rename = "modelUsage")]
    model_usage: Value,
    session_id: Option<String>,
}

/// The events read of `codex exec --json`, one JSON object a line.
#[derive(Deserialize)]
struct CodexEvent {
    #[serde(rename = "type")]
    kind: String,
    thread_id: Option<String>,
    usage: Option<Value>,
    /// `{"message": ...}` on `turn.failed`; any other shape still fails.
    error: Option<Value>,
}

/// A new owner-only directory for one run's files, as the agent reads
/// the schema and writes its answer by path.
fn scratch() -> std::io::Result<PathBuf> {
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let n = RUNS.fetch_add(1, Ordering::Relaxed);
    let name = format!("chaperone-run-{}-{nanos}-{n}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// The end of a failed run's stderr.
fn tail(err: &[u8]) -> String {
    let err = String::from_utf8_lossy(err);
    let from = err.chars().count().saturating_sub(ERR_CHARS);
    err.chars().skip(from).collect()
}

impl Agent {
    /// The CLI at `bin`, started from `env` less the session's variables.
    /// Asks its version, so a missing binary fails here.
    pub fn probe(
        bin: PathBuf,
        env: impl IntoIterator<Item = (OsString, OsString)>,
        deadline: Instant,
    ) -> Result<Agent, RunError> {
        // Lossy, so a name that is not UTF-8 is still matched.
        let session = |name: &OsString| {
            let name = name.to_string_lossy();
            SESSION_VARS.contains(&&*name)
                || (SESSION_PREFIXES.iter().any(|p| name.starts_with(p))
                    && !SESSION_KEEP.contains(&&*name))
        };
        let env = env.into_iter().filter(|(name, _)| !session(name)).collect();
        let mut agent = Agent {
            bin,
            version: String::new(),
            env,
        };
        let done = agent.run(
            agent.command(Path::new(".")).arg("--version"),
            None,
            deadline,
        )?;
        let out = String::from_utf8_lossy(&done.out);
        agent.version = out.lines().next().unwrap_or_default().trim().to_string();
        if !done.status.success() || agent.version.is_empty() {
            return Err(RunError::Exit {
                code: done.status.code(),
                stderr: tail(&done.err),
            });
        }
        Ok(agent)
    }

    fn command(&self, dir: &Path) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.current_dir(dir)
            .env_clear()
            .envs(self.env.iter().cloned());
        cmd
    }

    /// Runs to the end; an exit that is not 0 is the caller's to read.
    fn run(
        &self,
        cmd: &mut Command,
        input: Option<&[u8]>,
        deadline: Instant,
    ) -> Result<child::Done, RunError> {
        match child::run_input(cmd, input, deadline, OUT_BYTES) {
            Ok(done) if done.cut => Err(RunError::Output(format!("past {OUT_BYTES} bytes"))),
            Ok(done) => Ok(done),
            Err(Failed::TimedOut) => Err(RunError::TimedOut),
            Err(Failed::Spawn(e)) => Err(RunError::Spawn(format!("{}: {e}", self.bin.display()))),
            Err(Failed::Wait(e)) => Err(RunError::Io(e.to_string())),
        }
    }

    /// One fresh Claude Code session answering `ask`, the prompt on stdin.
    /// It loads the user's settings and `CLAUDE.md` only, no project
    /// settings or `CLAUDE.md`, no MCP server, skill or slash command (a
    /// prompt that starts with `/` gets no answer) and no stored session,
    /// and asks nobody for permission: what `tools` and the settings do
    /// not allow is denied. Its Bash runs in Claude Code's sandbox or the
    /// session does not start, with no unsandboxed retry and no host
    /// outside the allowlist; the allowlist, the allow rules and the
    /// writable paths (the directory, a temp directory) are the user
    /// settings', as lists merge and cannot be emptied here. A CLI too old
    /// for a sandbox key drops it without a word in `-p` mode, and nothing
    /// here checks the version yet.
    pub fn claude(&self, ask: &Ask<'_>, deadline: Instant) -> Result<Answer, RunError> {
        let budget = format!("{}.{:02}", ask.budget_cents / 100, ask.budget_cents % 100);
        let mut cmd = self.command(ask.dir);
        cmd.args(["-p", "--output-format", "json", "--json-schema"])
            .arg(ask.schema.to_string())
            .args(["--model", ask.model, "--tools", &ask.tools.join(",")])
            .args(["--permission-mode", "dontAsk", "--setting-sources", "user"])
            .args(["--settings", SANDBOX])
            .args(["--strict-mcp-config", "--disable-slash-commands"])
            .args(["--no-session-persistence", "--max-budget-usd", &budget]);
        let done = self.run(&mut cmd, Some(ask.prompt.as_bytes()), deadline)?;
        let raw = String::from_utf8_lossy(&done.out).into_owned();
        // A failure (the budget, the turns, the API) exits 1 and its
        // result still says why, and what it cost.
        let Ok(result) = serde_json::from_str::<ClaudeResult>(&raw) else {
            return Err(match done.status.success() {
                true => RunError::Output("not the CLI's JSON result".into()),
                false => RunError::Exit {
                    code: done.status.code(),
                    stderr: tail(&done.err),
                },
            });
        };
        let run = Run {
            raw,
            cost_usd: result.total_cost_usd,
            model_usage: result.model_usage,
            session_id: result.session_id,
        };
        let subtype = result.subtype.unwrap_or_default();
        if result.is_error || subtype != "success" || !done.status.success() {
            let errors: Vec<String> = result
                .errors
                .unwrap_or_default()
                .iter()
                .map(|e| e.as_str().map_or_else(|| e.to_string(), String::from))
                .collect();
            let message = match (errors.is_empty(), result.result) {
                (false, _) => errors.join("; "),
                (true, Some(said)) => said,
                (true, None) => tail(&done.err),
            };
            return Err(RunError::Failed {
                subtype,
                message,
                run: Box::new(run),
            });
        }
        let output = result
            .structured_output
            .ok_or_else(|| RunError::Output("no structured_output".into()))?;
        Ok(Answer { output, run })
    }

    /// One fresh Codex session answering `ask`, the prompt on stdin, in
    /// Codex's read-only sandbox and stored nowhere. It loads neither the
    /// user's `config.toml` (its MCP servers run outside the sandbox) nor
    /// any `.rules` file; the login still comes from `CODEX_HOME`. It
    /// reports no cost.
    pub fn codex(&self, ask: &CodexAsk<'_>, deadline: Instant) -> Result<Answer, RunError> {
        let dir = scratch().map_err(|e| RunError::Io(e.to_string()))?;
        let answer = self.codex_in(&dir, ask, deadline);
        let _ = std::fs::remove_dir_all(&dir);
        answer
    }

    fn codex_in(
        &self,
        dir: &Path,
        ask: &CodexAsk<'_>,
        deadline: Instant,
    ) -> Result<Answer, RunError> {
        let (schema, last) = (dir.join("schema.json"), dir.join("last.json"));
        std::fs::write(&schema, ask.schema.to_string()).map_err(|e| RunError::Io(e.to_string()))?;
        let mut cmd = self.command(ask.dir);
        cmd.args(["exec", "--json", "--ephemeral", "--ignore-user-config"])
            .args(["--ignore-rules", "-s", "read-only"]);
        if let Some(model) = ask.model {
            cmd.args(["-m", model]);
        }
        cmd.arg("--output-schema")
            .arg(&schema)
            .arg("-o")
            .arg(&last)
            .arg("-");
        let done = self.run(&mut cmd, Some(ask.prompt.as_bytes()), deadline)?;
        let raw = String::from_utf8_lossy(&done.out).into_owned();
        let (mut session_id, mut model_usage, mut failed) = (None, Value::Null, None);
        for event in raw
            .lines()
            .filter_map(|l| serde_json::from_str::<CodexEvent>(l).ok())
        {
            match event.kind.as_str() {
                "thread.started" => session_id = event.thread_id,
                "turn.completed" => model_usage = event.usage.unwrap_or_default(),
                "turn.failed" => {
                    let said = event.error.as_ref().and_then(|e| e["message"].as_str());
                    failed = Some(said.unwrap_or("no message").to_string());
                }
                _ => {}
            }
        }
        let run = Run {
            raw,
            cost_usd: None,
            model_usage,
            session_id,
        };
        if let Some(message) = failed {
            return Err(RunError::Failed {
                subtype: "turn.failed".into(),
                message,
                run: Box::new(run),
            });
        }
        if !done.status.success() {
            return Err(RunError::Exit {
                code: done.status.code(),
                stderr: tail(&done.err),
            });
        }
        // Missing, empty, cut or not JSON: the session left no answer.
        let mut text = String::new();
        std::fs::File::open(&last)
            .and_then(|f| f.take(OUT_BYTES as u64 + 1).read_to_string(&mut text))
            .map_err(|e| RunError::Output(format!("no last message: {e}")))?;
        if text.len() > OUT_BYTES {
            return Err(RunError::Output(format!("past {OUT_BYTES} bytes")));
        }
        let output = serde_json::from_str(&text)
            .map_err(|e| RunError::Output(format!("the last message is not JSON: {e}")))?;
        Ok(Answer { output, run })
    }
}
