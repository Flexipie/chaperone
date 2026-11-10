//! `chaperone check`, `reviews`, `jobs`, `usage`, `attention`, `finding` and `dossier`. `reviews record` runs
//! `daisy-cli reviews get`, the CLI's only call into daisy, before the
//! write opens its transaction. `check run` reads git in the lane's
//! worktree, and runs tests in a checkout of its commit, before its write,
//! under the lane's run lock.

use std::ffi::OsString;
use std::fs::{DirBuilder, File};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::checks::{AttentionInput, CheckInput, FindingMove, RunInput};
use chaperone_core::child;
use chaperone_core::dossier::NoteInput;
use chaperone_core::git;
use chaperone_core::ledger::{self, Actor, Evidence, Ledger, LedgerError, Refusal};
use chaperone_core::runner::session_var;
use chaperone_core::usage::Roots;
use chaperone_graph::{glob, tests_in};
use serde_json::{Map, Value, json};

use crate::cli::{
    AttentionCmd, CheckCmd, DossierCmd, FindingCmd, JobsCmd, ReviewsCmd, RunKind, UsageCmd,
};
use crate::output::{CliError, ErrorBody, Meta, Outcome};
use crate::work::{observed_head, one};

/// Where spawn and the skills call daisy (R15); `CHAPERONE_DAISY_CLI`
/// overrides it.
const DAISY_CLI: &str = "/Applications/daisy.app/Contents/MacOS/daisy-cli";
const DAISY_TIMEOUT: Duration = Duration::from_secs(30);
/// Not code: docs, Markdown and lock files (a dependency bump alone is no
/// code change). No profile says what code is yet, so every other changed
/// file counts. The profile should own this list later.
const NOT_CODE: [&str; 3] = ["docs/**", "**/*.md", "**/*.lock"];
/// The git calls that read the lane's commit, its tree and its numstat.
const RUN_TIMEOUT: Duration = Duration::from_secs(10);
/// `check run tests`, when the profile names no `timeout_secs`.
const TESTS_TIMEOUT: Duration = Duration::from_secs(1800);
/// Of each test command's stdout and stderr, the end kept as evidence.
const TAIL_BYTES: usize = 16 * 1024;
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
        CheckCmd::Run {
            kind: RunKind::Tests,
            work,
            actor,
        } => run_tests(ledger, db, work, &actor.resolve()),
    }
}

/// A `check run` under way: the lane's run lock, held until it drops, the
/// lane's worktree and claimed base, and the input with the commit the lane
/// is on (as a ready handoff reads it), its tree and this executable.
struct Run {
    _lock: File,
    wt: PathBuf,
    base_sha: Option<String>,
    input: RunInput,
}

fn start_run(ledger: &Ledger, db: &Path, work: i64, check: &str) -> Result<Run, CliError> {
    let lock = ledger::lock_lane(db, work)??;
    let item = ledger.work_get(work, &ledger.now()?)??;
    let wt = item.worktree_path.as_deref();
    let head = observed_head(wt, item.branch.as_deref())?.ok_or_else(|| {
        refuse(format!(
            "work item {work} has no worktree to measure (none recorded, not absolute, or gone)"
        ))
    })?;
    let wt = PathBuf::from(wt.unwrap_or_default());
    let tree = format!("{head}^{{tree}}");
    let args = ["rev-parse", "--verify", &tree];
    let tree = git::git(
        Path::new("git"),
        &wt,
        &args,
        Instant::now() + RUN_TIMEOUT,
        4096,
    )
    .map(|(out, _)| String::from_utf8_lossy(&out).trim().to_string())
    .map_err(|_| refuse(format!("could not read {tree} in {}", wt.display())))?;
    let bin = std::env::current_exe().map_err(LedgerError::from)?;
    let input = RunInput {
        work,
        check: check.into(),
        head,
        tree,
        bin: bin.display().to_string(),
        version: match env!("CHAPERONE_GIT_SHA") {
            "" => env!("CARGO_PKG_VERSION").into(),
            sha => format!("{}+{sha}", env!("CARGO_PKG_VERSION")),
        },
        ..RunInput::default()
    };
    Ok(Run {
        _lock: lock,
        wt,
        base_sha: item.base_sha,
        input,
    })
}

fn refuse(why: String) -> CliError {
    Refusal::InvalidInput(why).into()
}

/// `check run diff`: the lane's commit (as a ready handoff reads it)
/// against the merge base, committed files only. The per-file list goes to
/// the evidence store, its totals in the payload.
fn run_diff(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let Run {
        _lock,
        wt,
        base_sha,
        mut input,
    } = start_run(ledger, db, work, "diff")?;
    let (bin, deadline) = (Path::new("git"), Instant::now() + RUN_TIMEOUT);
    let head = &input.head;
    let (base, files) =
        git::commit_files(bin, &wt, base_sha.as_deref(), head, deadline, DIFF_BYTES)
            .map_err(|e| refuse(format!("could not diff {head} in {}: {e}", wt.display())))?;
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
    input.fields = Map::from_iter([
        ("base".into(), json!(base)),
        ("files".into(), json!(files.len())),
        ("added".into(), json!(added)),
        ("removed".into(), json!(removed)),
        ("code_lines".into(), json!(code_lines)),
        ("changes_code".into(), json!(!code.is_empty())),
        // Binary, or text an attributes rule (`-diff`) hides: no lines
        // counted, so a size gate can refuse rather than trust the totals.
        (
            "uncounted_files".into(),
            json!(files.iter().filter(|f| f.added.is_none()).count()),
        ),
    ]);
    // One row per line: the store keeps a long text's end, and a cut list
    // still reads line by line.
    let list: String = files
        .iter()
        .map(|f| json!({"path": f.path, "added": f.added, "removed": f.removed}).to_string() + "\n")
        .collect();
    let stored = Evidence::beside(db).put(&list).map_err(LedgerError::from)?;
    let truncated = json!(stored.truncated);
    input.fields.insert("files_truncated".into(), truncated);
    input.evidence_ref = Some(stored.hash);
    one(ledger.check_run(actor, &input)??)
}

/// `check run tests`: the `[tests]` commands of the base branch's profile,
/// in order until one fails, in a checkout of the lane's commit that the
/// harness owns, then the checkout must still be clean. A pass on the same
/// commit and profile replays without running anything. A run that times
/// out failed: the tests did not pass on that tree. A command that cannot
/// start, a checkout that cannot be made, or no profile writes nothing.
fn run_tests(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let Run {
        _lock,
        wt,
        mut input,
        ..
    } = start_run(ledger, db, work, "tests")?;
    let bin = Path::new("git");
    let (tests, profile) =
        tests_in(bin, &wt, Instant::now() + RUN_TIMEOUT).map_err(|e| match e {
            chaperone_graph::Error::Invalid(why) => refuse(why),
            chaperone_graph::Error::Internal(why) => CliError::Internal(why),
        })?;
    input.profile = Some(profile);
    let key = ledger.run_key(&input)?;
    if let Some(stored) = ledger.run_replay(&key)? {
        return one(stored);
    }
    let timeout = tests
        .timeout_secs
        .map_or(TESTS_TIMEOUT, Duration::from_secs);
    let deadline = Instant::now() + timeout;
    // Beside the ledger, one per lane: the lane's lock keeps it to one run,
    // and the same path each time lets a build cache match it again.
    let home =
        std::path::absolute(db.parent().unwrap_or(Path::new(""))).map_err(LedgerError::from)?;
    let dir = home.join("checkouts").join(format!("lane-{work}"));
    let checkout = Checkout::add(&wt, dir, &input.head, deadline)?;
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !session_var(k) && !git::GIT_REPO_VARS.iter().any(|v| k == v))
        .collect();
    if let Some(var) = &tests.target_dir {
        let target = home.join("targets").join(format!("lane-{work}"));
        env.push((var.into(), target.into()));
    }
    let (mut commands, mut text, mut timed_out, mut passed) =
        (Vec::new(), String::new(), false, true);
    for argv in &tests.commands {
        let started = Instant::now();
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(&checkout.dir)
            .env_clear()
            .envs(env.iter().cloned());
        text += &format!("$ {argv:?}\n");
        let exit = match child::run_input(&mut cmd, None, deadline, usize::MAX) {
            Ok(done) => {
                text += &(tail(&done.err) + &tail(&done.out));
                done.status.code()
            }
            Err(child::Failed::TimedOut) => {
                timed_out = true;
                None
            }
            Err(child::Failed::Spawn(e)) => {
                return Err(refuse(format!("could not start {argv:?}: {e}")));
            }
            Err(child::Failed::Wait(e)) => {
                return Err(CliError::Internal(format!("{argv:?}: {e}")));
            }
        };
        let ms = started.elapsed().as_millis() as u64;
        commands.push(json!({"argv": argv, "exit": exit, "ms": ms}));
        if exit != Some(0) {
            passed = false;
            break;
        }
    }
    // Only a tree the commands passed on is checked: a failure already fails.
    let mut dirty = None;
    if passed {
        let args = ["status", "--porcelain", "--untracked-files=all"];
        let status = git::git(bin, &checkout.dir, &args, deadline, 1024 * 1024)
            .map_err(|_| refuse("could not read the checkout's status".into()))?;
        let status = String::from_utf8_lossy(&status.0).into_owned();
        passed = status.is_empty();
        dirty = Some(!passed);
        text += &format!("$ git status --porcelain\n{status}");
    }
    drop(checkout);
    let stored = Evidence::beside(db).put(&text).map_err(LedgerError::from)?;
    input.evidence_ref = Some(stored.hash);
    input.fields = Map::from_iter([
        ("result".into(), json!(if passed { "pass" } else { "fail" })),
        ("commands".into(), json!(commands)),
        ("timed_out".into(), json!(timed_out)),
        ("dirty".into(), json!(dirty)),
        ("output_truncated".into(), json!(stored.truncated)),
    ]);
    one(ledger.check_run(actor, &input)??)
}

/// The end of a command's output, as text.
fn tail(out: &[u8]) -> String {
    String::from_utf8_lossy(&out[out.len().saturating_sub(TAIL_BYTES)..]).into_owned()
}

/// A detached checkout of a lane's commit, made with `git worktree add` from
/// the lane's repo into a directory the harness owns. Dropping it removes
/// it, on every way out of the run; one a killed run left is removed first.
struct Checkout {
    repo: PathBuf,
    dir: PathBuf,
}

impl Checkout {
    fn add(repo: &Path, dir: PathBuf, head: &str, deadline: Instant) -> Result<Checkout, CliError> {
        if let Some(parent) = dir.parent() {
            let mut owner_only = DirBuilder::new();
            owner_only
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(LedgerError::from)?;
        }
        let checkout = Checkout {
            repo: repo.into(),
            dir,
        };
        checkout.remove();
        let at = checkout.dir.to_string_lossy();
        let args = ["worktree", "add", "--detach", "--quiet", &at, head];
        git::git(Path::new("git"), repo, &args, deadline, 1024 * 1024)
            .map_err(|_| refuse(format!("could not check out {head} at {at}")))?;
        Ok(checkout)
    }

    fn remove(&self) {
        let (bin, deadline) = (Path::new("git"), Instant::now() + RUN_TIMEOUT);
        let dir = self.dir.to_string_lossy();
        let _ = git::git(
            bin,
            &self.repo,
            &["worktree", "remove", "--force", &dir],
            deadline,
            4096,
        );
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = git::git(bin, &self.repo, &["worktree", "prune"], deadline, 4096);
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        self.remove();
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

pub fn finding(ledger: &Ledger, cmd: FindingCmd, meta: &mut Meta) -> Result<Outcome, CliError> {
    let (status, args) = match cmd {
        FindingCmd::Dispute(args) => ("disputed", args),
        FindingCmd::Waive(args) => ("waived", args),
    };
    meta.dry_run = args.dry_run.then_some(true);
    let input = FindingMove {
        check_event_id: args.event,
        finding_key: args.key,
        status: status.into(),
        reason: Some(args.reason),
        head_sha: None,
    };
    one(ledger.finding_update(&args.actor.resolve(), &input, args.dry_run)??)
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
