//! Headless agent runs for the checks the harness runs itself: one fresh
//! `claude -p` session per question, answering in JSON under a schema.
//!
//! The agent binary and the environment are the caller's arguments, never
//! read here, and the known variables of the session that started the run
//! are not passed on: a reviewer is no lane (the lane hooks stay quiet),
//! and it is not handed the socket, pane or history of the session it
//! reviews. The list is a speed bump, as every control on the same OS user
//! is; the sandbox settings are the next one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

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
const SESSION_PREFIXES: [&str; 4] = ["ATUIN_", "CHAPERONE_", "DAISY_", "HERDR_"];
/// An answer past this is refused, not cut: half a JSON object is none.
const OUT_BYTES: usize = 16 * 1024 * 1024;
/// Of a failed run's stderr, its end.
const ERR_CHARS: usize = 2000;

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

/// What a session left behind, answered or not: a failed run still cost.
#[derive(Debug, PartialEq)]
pub struct Run {
    /// The CLI's whole result, for the evidence store.
    pub raw: String,
    pub cost_usd: Option<f64>,
    /// The CLI's `modelUsage`: tokens and cost per model.
    pub model_usage: Value,
    pub session_id: Option<String>,
}

#[derive(Debug)]
pub struct Answer {
    /// The answer, valid under the schema (the CLI checks it).
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
    /// error), `message` what it gave as the reason.
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
            SESSION_VARS.contains(&&*name) || SESSION_PREFIXES.iter().any(|p| name.starts_with(p))
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
    /// It loads the user's settings only, no project settings, no MCP
    /// server and no stored session, and asks nobody for permission: what
    /// `tools` and the settings do not allow is denied.
    pub fn claude(&self, ask: &Ask<'_>, deadline: Instant) -> Result<Answer, RunError> {
        let budget = format!("{}.{:02}", ask.budget_cents / 100, ask.budget_cents % 100);
        let mut cmd = self.command(ask.dir);
        cmd.args(["-p", "--output-format", "json", "--json-schema"])
            .arg(ask.schema.to_string())
            .args(["--model", ask.model, "--tools", &ask.tools.join(",")])
            .args(["--permission-mode", "dontAsk", "--setting-sources", "user"])
            .args(["--strict-mcp-config", "--no-session-persistence"])
            .args(["--max-budget-usd", &budget]);
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
}
