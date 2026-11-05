//! `chaperone check`, `reviews` and `jobs`. `reviews record` runs
//! `daisy-cli reviews get`, Chaperone's only call into daisy, before the
//! write opens its transaction.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chaperone_core::checks::CheckInput;
use chaperone_core::ledger::Ledger;
use serde_json::{Value, json};

use crate::cli::{CheckCmd, JobsCmd, ReviewsCmd};
use crate::output::{CliError, ErrorBody, Outcome};
use crate::work::one;

/// Where spawn and the skills call daisy (R15); `CHAPERONE_DAISY_CLI`
/// overrides it.
const DAISY_CLI: &str = "/Applications/daisy.app/Contents/MacOS/daisy-cli";
const DAISY_TIMEOUT: Duration = Duration::from_secs(30);

pub fn check(ledger: &Ledger, cmd: CheckCmd) -> Result<Outcome, CliError> {
    match cmd {
        CheckCmd::Record {
            work,
            check,
            head,
            outcome,
            reason,
            decision,
            findings,
            actor,
        } => {
            let input = CheckInput {
                work,
                check,
                head,
                outcome,
                reason,
                decision,
                findings,
            };
            one(ledger.check_record(&actor.resolve(), &input)??)
        }
    }
}

pub fn reviews(ledger: &Ledger, cmd: ReviewsCmd) -> Result<Outcome, CliError> {
    match cmd {
        ReviewsCmd::Record { work, job, actor } => {
            let view = daisy_review(job)?;
            one(ledger.review_record(&actor.resolve(), work, job, &view)??)
        }
        ReviewsCmd::Get { job, .. } => one(ledger.review_get(job)??),
    }
}

pub fn jobs(ledger: &Ledger, cmd: JobsCmd) -> Result<Outcome, CliError> {
    match cmd {
        JobsCmd::Get { id } => one(ledger.job_get(id)??),
    }
}

/// The `data` of `daisy-cli reviews get <job>`. Daisy's exit 5 and
/// `not_found` pass through; any other failure is `internal`, with what
/// daisy said in `details`, so daisy's exit codes never take Chaperone's
/// meanings (3 is this ledger's schema).
fn daisy_review(job: i64) -> Result<Value, CliError> {
    let bin = std::env::var_os("CHAPERONE_DAISY_CLI").unwrap_or_else(|| DAISY_CLI.into());
    let failed = |message: String, details: Value| {
        CliError::Daisy(ErrorBody {
            code: "internal",
            message: format!("daisy-cli reviews get {job}: {message}"),
            retryable: false,
            details: Some(details),
        })
    };
    let mut child = Command::new(&bin)
        .args(["reviews", "get", &job.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| failed(format!("could not run {}: {e}", bin.display()), json!({})))?;
    // Read both pipes while waiting, so a large answer cannot fill one and
    // stall daisy until the timeout.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = Instant::now() + DAISY_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(
                    format!("no answer in {}s", DAISY_TIMEOUT.as_secs()),
                    json!({}),
                ));
            }
            Err(e) => return Err(failed(e.to_string(), json!({}))),
        }
    };
    let (out, err) = (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    );
    if status.success() {
        let env: Value = serde_json::from_slice(&out).unwrap_or(Value::Null);
        return match env.get("data") {
            Some(data) if env["ok"] == true => Ok(data.clone()),
            _ => Err(failed("not a daisy-cli envelope".into(), json!({}))),
        };
    }
    let exit = status.code();
    let error = serde_json::from_slice::<Value>(&err)
        .ok()
        .and_then(|v| v.get("error").cloned());
    let field = |k: &str| error.as_ref().and_then(|e| e.get(k)).cloned();
    let message = field("message")
        .and_then(|m| m.as_str().map(String::from))
        .unwrap_or_else(|| String::from_utf8_lossy(&err).trim().to_string());
    let code = field("code");
    let passed = if exit == Some(5) {
        Some("runtime_unavailable")
    } else if code.as_ref().and_then(Value::as_str) == Some("not_found") {
        Some("not_found")
    } else {
        None
    };
    match passed {
        Some(code) => Err(CliError::Daisy(ErrorBody {
            code,
            message,
            retryable: field("retryable")
                .and_then(|r| r.as_bool())
                .unwrap_or(false),
            details: field("details"),
        })),
        None => Err(failed(message, json!({ "daisy": error, "exit": exit }))),
    }
}
