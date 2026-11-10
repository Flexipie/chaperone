//! `chaperone check`, `reviews`, `jobs`, `usage`, `attention` and `dossier`. `reviews record` runs
//! `daisy-cli reviews get`, the CLI's only call into daisy, before the
//! write opens its transaction. `check run` reads git in the lane's
//! worktree before its write, under the lane's run lock.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::checks::{AttentionInput, CheckInput, RunInput};
use chaperone_core::child;
use chaperone_core::dossier::NoteInput;
use chaperone_core::git;
use chaperone_core::ledger::{self, Actor, Evidence, Ledger, LedgerError, Refusal};
use chaperone_core::usage::Roots;
use chaperone_graph::glob;
use serde_json::{Map, Value, json};

use crate::cli::{AttentionCmd, CheckCmd, DossierCmd, JobsCmd, ReviewsCmd, RunKind, UsageCmd};
use crate::output::{CliError, ErrorBody, Outcome};
use crate::work::{observed_head, one};

/// Where spawn and the skills call daisy (R15); `CHAPERONE_DAISY_CLI`
/// overrides it.
const DAISY_CLI: &str = "/Applications/daisy.app/Contents/MacOS/daisy-cli";
const DAISY_TIMEOUT: Duration = Duration::from_secs(30);
/// Not code: docs and Markdown. No profile says what code is yet, so every
/// other changed file counts.
const NOT_CODE: [&str; 2] = ["docs/**", "**/*.md"];
/// Every git call of one `check run diff`, together.
const RUN_TIMEOUT: Duration = Duration::from_secs(10);
/// The numstat's output; a cut list would count too little, so past it the
/// run fails.
const DIFF_BYTES: usize = 16 * 1024 * 1024;

pub fn check(ledger: &Ledger, db: &Path, cmd: CheckCmd) -> Result<Outcome, CliError> {
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
        CheckCmd::Run {
            kind: RunKind::Diff,
            work,
            actor,
        } => run_diff(ledger, db, work, &actor.resolve()),
    }
}

/// `check run diff`: the lane's commit (as a ready handoff reads it)
/// against the merge base, committed files only. The per-file list goes to
/// the evidence store, its totals in the payload.
fn run_diff(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let item = ledger.work_get(work, &ledger.now()?)??;
    let _lock = ledger::lock_lane(db, work)??;
    let refuse = |why: String| CliError::from(Refusal::InvalidInput(why));
    let wt = item.worktree_path.as_deref();
    let head = observed_head(wt, item.branch.as_deref())?.ok_or_else(|| {
        refuse(format!(
            "work item {work} has no worktree to measure (none recorded, not absolute, or gone)"
        ))
    })?;
    let wt = Path::new(wt.unwrap_or_default());
    let (bin, deadline) = (Path::new("git"), Instant::now() + RUN_TIMEOUT);
    let base = item.base_sha.as_deref();
    let (base, files) = git::commit_files(bin, wt, base, &head, deadline, DIFF_BYTES)
        .map_err(|e| refuse(format!("could not diff {head} in {}: {e}", wt.display())))?;
    let tree = format!("{head}^{{tree}}");
    let tree = git::git(bin, wt, &["rev-parse", "--verify", &tree], deadline, 4096)
        .map(|(out, _)| String::from_utf8_lossy(&out).trim().to_string())
        .map_err(|_| refuse(format!("could not read {tree} in {}", wt.display())))?;
    let code: Vec<_> = files
        .iter()
        .filter(|f| !NOT_CODE.iter().any(|g| glob(g, &f.path)))
        .collect();
    let added: u64 = files.iter().filter_map(|f| f.added).sum();
    let removed: u64 = files.iter().filter_map(|f| f.removed).sum();
    let code_lines: u64 = code
        .iter()
        .map(|f| f.added.unwrap_or(0) + f.removed.unwrap_or(0))
        .sum();
    let fields = Map::from_iter([
        ("base".into(), json!(base)),
        ("files".into(), json!(files.len())),
        ("added".into(), json!(added)),
        ("removed".into(), json!(removed)),
        ("code_lines".into(), json!(code_lines)),
        ("changes_code".into(), json!(!code.is_empty())),
    ]);
    let list = serde_json::to_string(&files).map_err(|e| CliError::Internal(e.to_string()))?;
    let stored = Evidence::beside(db).put(&list).map_err(LedgerError::from)?;
    let bin = std::env::current_exe().map_err(LedgerError::from)?;
    let input = RunInput {
        work,
        check: "diff".into(),
        head,
        tree,
        fields,
        evidence_ref: Some(stored.hash),
        bin: bin.display().to_string(),
        version: match env!("CHAPERONE_GIT_SHA") {
            "" => env!("CARGO_PKG_VERSION").into(),
            sha => format!("{}+{sha}", env!("CARGO_PKG_VERSION")),
        },
    };
    one(ledger.check_run(actor, &input)??)
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

pub fn attention(ledger: &Ledger, cmd: AttentionCmd) -> Result<Outcome, CliError> {
    match cmd {
        AttentionCmd::Record {
            work,
            kind,
            by,
            reason,
            round,
            answers,
            note,
            idempotency_key,
            actor,
        } => {
            let input = AttentionInput {
                work,
                kind,
                by,
                reason,
                round,
                answers,
                note,
                idempotency_key,
            };
            one(ledger.attention_record(&actor.resolve(), &input)??)
        }
    }
}

pub fn dossier(ledger: &Ledger, cmd: DossierCmd) -> Result<Outcome, CliError> {
    match cmd {
        DossierCmd::Note {
            key,
            note,
            links,
            supersedes,
            actor,
        } => {
            let input = NoteInput {
                key,
                note,
                links,
                supersedes,
            };
            one(ledger.dossier_note(&actor.resolve(), &input)??)
        }
        DossierCmd::Get { key } => one(ledger.dossier_get(&key)??),
    }
}

pub fn usage(ledger: &Ledger, cmd: UsageCmd) -> Result<Outcome, CliError> {
    match cmd {
        UsageCmd::Report {
            work,
            claude_dir,
            codex_dir,
            actor,
        } => {
            let roots = Roots {
                claude: claude_dir.or_else(|| home("CLAUDE_CONFIG_DIR", ".claude")),
                codex: codex_dir.or_else(|| home("CODEX_HOME", ".codex")),
            };
            one(ledger.usage_report(&actor.resolve(), work, &roots)??)
        }
        UsageCmd::Get { id } => one(ledger.usage_get(id)??),
    }
}

/// A CLI's default home: its variable as given (so a typo is refused, as
/// with a flag), else `~/<dir>` when that folder exists.
fn home(var: &str, dir: &str) -> Option<std::path::PathBuf> {
    std::env::var_os(var)
        .map(Into::into)
        .or_else(|| dirs::home_dir().map(|h| h.join(dir)).filter(|p| p.is_dir()))
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
    let deadline = Instant::now() + DAISY_TIMEOUT;
    let child::Done {
        status, out, err, ..
    } = child::run(
        Command::new(&bin).args(["reviews", "get", &job.to_string()]),
        deadline,
        usize::MAX,
    )
    .map_err(|e| match e {
        child::Failed::Spawn(e) => {
            failed(format!("could not run {}: {e}", bin.display()), json!({}))
        }
        child::Failed::TimedOut => failed(
            format!("no answer in {}s", DAISY_TIMEOUT.as_secs()),
            json!({}),
        ),
        child::Failed::Wait(e) => failed(e.to_string(), json!({})),
    })?;
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
