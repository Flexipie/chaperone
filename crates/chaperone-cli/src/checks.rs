//! `chaperone check`, `reviews`, `jobs`, `usage`, `attention`, `finding` and `dossier`. `reviews record` runs
//! `daisy-cli reviews get`, the CLI's only call into daisy, before the
//! write opens its transaction. `check run` reads git in the lane's
//! worktree, and runs tests and the review's and the verdict's sessions in
//! a checkout of its commit, before its write, under the lane's run lock.

use std::ffi::OsString;
use std::fs::{DirBuilder, File};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::checks::{AttentionInput, CheckInput, FindingMove, RunInput};
use chaperone_core::child;
use chaperone_core::dossier::NoteInput;
use chaperone_core::git::{self, ChangedFile};
use chaperone_core::ledger::{self, Actor, Evidence, Ledger, LedgerError, Refusal};
use chaperone_core::pipeline::{self, Cli, Pipeline};
use chaperone_core::plan;
use chaperone_core::review::{self, Agents, Facts};
use chaperone_core::runner::{Agent, session_var};
use chaperone_core::usage::Roots;
use chaperone_core::verdict;
use chaperone_core::work::WorkItem;
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
/// `check run review`'s checkout of the lane's commit, which fetches its
/// whole history.
const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(600);
/// Of each test command's stdout and stderr, the end kept as evidence.
const TAIL_BYTES: usize = 16 * 1024;
/// A plan file: a document a person reads, far above any real one.
const PLAN_BYTES: usize = 64 * 1024;
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
            kind,
            work,
            file,
            actor,
        } => {
            let actor = actor.resolve();
            match (kind, file) {
                (RunKind::Plan, Some(file)) => run_plan(ledger, db, work, &file, &actor),
                (RunKind::Plan, None) | (_, Some(_)) => Err(refuse(
                    "--file <plan> goes with plan, and is required there".into(),
                )),
                (RunKind::Diff, None) => run_diff(ledger, db, work, &actor),
                (RunKind::Tests, None) => run_tests(ledger, db, work, &actor),
                (RunKind::Review, None) => run_review(ledger, db, work, &actor),
                (RunKind::Verdict, None) => run_verdict(ledger, db, work, &actor),
            }
        }
    }
}

/// A `check run` under way: the lane's run lock, held until it drops, the
/// lane's worktree and work item, and the input with the commit the lane
/// is on (as a ready handoff reads it), its tree and this executable.
struct Run {
    _lock: File,
    wt: PathBuf,
    item: WorkItem,
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
        item,
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
        item,
        mut input,
    } = start_run(ledger, db, work, "diff")?;
    let (base, files) = changed(&wt, item.base_sha.as_deref(), &input.head)?;
    let (code, code_lines) = code(&files);
    let added: u64 = files.iter().filter_map(|f| f.added).sum();
    let removed: u64 = files.iter().filter_map(|f| f.removed).sum();
    input.fields = Map::from_iter([
        ("base".into(), json!(base)),
        ("files".into(), json!(files.len())),
        ("added".into(), json!(added)),
        ("removed".into(), json!(removed)),
        ("code_lines".into(), json!(code_lines)),
        ("changes_code".into(), json!(code > 0)),
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

/// Of `files`, how many are code (not [`NOT_CODE`]), and their lines
/// added plus removed.
fn code(files: &[ChangedFile]) -> (usize, u64) {
    let code = files
        .iter()
        .filter(|f| !NOT_CODE.iter().any(|g| glob(g, &f.path)));
    code.fold((0, 0), |(n, lines), f| {
        (n + 1, lines + f.added.unwrap_or(0) + f.removed.unwrap_or(0))
    })
}

/// The files the commit `head` changes against the merge base, and the
/// base: `diff`'s numstat.
fn changed(
    wt: &Path,
    base_sha: Option<&str>,
    head: &str,
) -> Result<(String, Vec<ChangedFile>), CliError> {
    let (bin, deadline) = (Path::new("git"), Instant::now() + RUN_TIMEOUT);
    git::commit_files(bin, wt, base_sha, head, deadline, DIFF_BYTES)
        .map_err(|e| refuse(format!("could not diff {head} in {}: {e}", wt.display())))
}

/// `check run tests`: the `[tests]` commands of the base branch's profile,
/// in order until one fails, in a fresh checkout of the lane's commit that
/// the harness makes and removes, then the checkout must still be at that
/// commit with no change or new untracked file. A pass on the same commit
/// and profile replays without running anything. A run that times out
/// failed: the tests did not pass on that tree. No profile, a checkout that
/// cannot be made or a command that cannot start writes nothing.
fn run_tests(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let Run {
        _lock,
        wt,
        mut input,
        ..
    } = start_run(ledger, db, work, "tests")?;
    let base = tests_in(Path::new("git"), &wt, Instant::now() + RUN_TIMEOUT)?;
    input.profile = Some(base.hash.clone());
    let key = ledger.run_key(&input)?;
    if let Some(stored) = ledger.run_replay(&key)? {
        return one(stored);
    }
    let tests = &base.tests;
    let timeout = tests
        .timeout_secs
        .map_or(TESTS_TIMEOUT, Duration::from_secs);
    let deadline = Instant::now() + timeout;
    let db = std::path::absolute(db).map_err(LedgerError::from)?;
    let home = db.parent().unwrap_or(Path::new("/"));
    let checkout = lane_checkout(&db, &wt, work, &input.head, deadline)?;
    let target = tests.target_dir.as_ref().map(|var| {
        let lane_target = home.join("targets").join(format!("lane-{work}"));
        (OsString::from(var), lane_target.into_os_string())
    });
    let ran = run_commands(&tests.commands, &checkout.dir, target, deadline)?;
    let mut after = Map::from_iter(
        ["dirty", "head_moved", "ignored_files", "status_cut"].map(|k| (k.into(), Value::Null)),
    );
    let mut text = String::new();
    let mut passed = ran.passed;
    if passed {
        let found = tree_after(&checkout.dir, &input.head, deadline)?;
        passed = !(found.dirty || found.head_moved || found.cut);
        after.extend([
            ("dirty".into(), json!(found.dirty)),
            ("head_moved".into(), json!(found.head_moved)),
            ("ignored_files".into(), json!(found.ignored)),
            ("status_cut".into(), json!(found.cut)),
        ]);
        // First: the store keeps a long text's end, the commands' output.
        text = found.listing;
    }
    drop(checkout);
    text += &ran.text;
    let stored = Evidence::beside(&db)
        .put(&text)
        .map_err(LedgerError::from)?;
    input.evidence_ref = Some(stored.hash);
    input.fields = Map::from_iter([
        ("result".into(), json!(if passed { "pass" } else { "fail" })),
        ("commands".into(), json!(ran.commands)),
        ("timed_out".into(), json!(ran.timed_out)),
        (
            "output_truncated".into(),
            json!(ran.cut || stored.truncated),
        ),
        (
            "profile_base".into(),
            json!({"ref": base.base, "commit": base.commit}),
        ),
    ]);
    input.fields.extend(after);
    one(ledger.check_run(actor, &input)??)
}

/// `check run review`: the lane's pipeline's lenses and verifier
/// (`review::record`) in the harness's checkout of the lane's commit, on
/// the files `diff` counts and the diff from their merge base, with the
/// lane's goal from the ledger. First the rulings a crash left unwritten
/// (`review::replay`); then a complete review of the commit replays
/// before anything runs. A lane whose pipeline has no review checks (v17
/// to v19) writes nothing; a CLI that cannot start, a checkout that
/// cannot be made and a diff past 16 MiB write nothing more.
fn run_review(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let Run {
        _lock,
        wt,
        item,
        mut input,
    } = start_run(ledger, db, work, "review")?;
    let plan = review::plan(lane_pipeline(&item)?).map_err(|e| {
        refuse(format!(
            "{e}; a lane on it records reviews with `check record` and `reviews record`"
        ))
    })?;
    if let Some(stored) = review::replay(ledger, actor, &mut input, &plan)?? {
        return one(stored);
    }
    let started = |cli| plan.uses(cli).then(|| agent(cli)).transpose();
    let (claude, codex) = (started(Cli::Claude)?, started(Cli::Codex)?);
    let (checkout, facts) = change(ledger, db, &wt, &item, &input.head)?;
    let agents = Agents {
        claude: claude.as_ref(),
        codex: codex.as_ref(),
    };
    let store = Evidence::beside(db);
    one(review::record(
        ledger,
        actor,
        input,
        &plan,
        &agents,
        &checkout.dir,
        &facts,
        &store,
    )??)
}

/// `check run verdict`: one fresh session on the lane's pipeline's
/// `verdict` check (`verdict::record`) in the harness's checkout, given the
/// change as the review is, the lane's goal and the harness's own results
/// on the commit. A verdict on the same tree, pipeline and results replays
/// (`verdict::replay`), carried to an amended commit, and starts nothing.
/// A lane whose pipeline has no `verdict` (v17 to v19), and a commit
/// without a harness `diff`, `tests` result and complete `review`, write
/// nothing.
fn run_verdict(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let Run {
        _lock,
        wt,
        item,
        mut input,
    } = start_run(ledger, db, work, verdict::EVENT_CHECK)?;
    let pipeline = lane_pipeline(&item)?;
    let planned = verdict::plan(pipeline).map_err(|e| {
        refuse(format!(
            "{e}; a lane on it records verdicts with `check record`"
        ))
    })?;
    let head = input.head.clone();
    let harness = verdict::harness(ledger, work, &head)?.map_err(|lacks| {
        refuse(format!(
            "commit {head} has no harness {lacks}; a verdict needs `check run diff`, `tests` \
             and a complete `review` on it first"
        ))
    })?;
    if let Some(stored) = verdict::replay(ledger, actor, &mut input, &pipeline.hash, &harness)?? {
        return one(stored);
    }
    let cli = planned.runner.cli;
    let started = agent(cli)?;
    let (checkout, facts) = change(ledger, db, &wt, &item, &head)?;
    let agents = Agents {
        claude: (cli == Cli::Claude).then_some(&started),
        codex: (cli == Cli::Codex).then_some(&started),
    };
    let store = Evidence::beside(db);
    one(verdict::record(
        ledger,
        actor,
        input,
        &planned,
        &agents,
        &checkout.dir,
        &facts,
        &harness,
        &store,
    )??)
}

/// `check run plan`: the lane's commit must change no code file against
/// its base, as `diff` counts code, or the run is refused and names the
/// owner's waiver. Then the plan file is read here, the pipeline must name
/// a `plan` check, and the plan is stored as evidence; the same plan on the
/// same tree and pipeline replays. Else one fresh
/// session on the pipeline's `plan` check (`plan::record`) in the
/// harness's checkout attacks it. A lane whose pipeline has no `plan`
/// (v17 to v19) writes nothing.
fn run_plan(
    ledger: &Ledger,
    db: &Path,
    work: i64,
    file: &Path,
    actor: &Actor,
) -> Result<Outcome, CliError> {
    let Run {
        _lock,
        wt,
        item,
        mut input,
    } = start_run(ledger, db, work, plan::EVENT_CHECK)?;
    let head = input.head.clone();
    let (_, files) = changed(&wt, item.base_sha.as_deref(), &head)?;
    let (code, lines) = code(&files);
    if code > 0 {
        return Err(refuse(format!(
            "commit {head} changes {code} code files ({lines} lines) against its base: the plan \
             check runs before any code. The owner can waive it with `check waive plan --work {work}`"
        )));
    }
    let text = plan_text(file)?;
    let pipeline = lane_pipeline(&item)?;
    let planned = plan::plan(pipeline).map_err(|e| {
        refuse(format!(
            "{e}; a lane on it records plan checks with `check record`"
        ))
    })?;
    let store = Evidence::beside(db);
    let plan_ref = store.put(&text).map_err(LedgerError::from)?.hash;
    if let Some(stored) = plan::replay(ledger, &mut input, &pipeline.hash, &plan_ref)? {
        return one(stored);
    }
    let cli = planned.runner.cli;
    let started = agent(cli)?;
    let (checkout, facts) = change(ledger, db, &wt, &item, &head)?;
    let agents = Agents {
        claude: (cli == Cli::Claude).then_some(&started),
        codex: (cli == Cli::Codex).then_some(&started),
    };
    let given = (&*facts.goal, &*text, &*plan_ref, &*facts.base);
    one(plan::record(
        ledger,
        actor,
        input,
        &planned,
        &agents,
        &checkout.dir,
        given,
        &store,
    )??)
}

/// The plan at `path`: a regular file (a link to one is followed) of at
/// most [`PLAN_BYTES`] of UTF-8 text that is not blank. It may sit outside
/// the worktree, as plans are kept in the agent's own folder; it is the
/// lane's text, given to the reviewer as fenced data and stored redacted.
fn plan_text(path: &Path) -> Result<String, CliError> {
    let bad = |why: &str| refuse(format!("plan {}: {why}", path.display()));
    let file = File::open(path).map_err(|e| bad(&e.to_string()))?;
    let meta = file.metadata().map_err(|e| bad(&e.to_string()))?;
    if !meta.is_file() {
        return Err(bad("not a regular file"));
    }
    let mut bytes = Vec::new();
    let read = file.take(PLAN_BYTES as u64 + 1).read_to_end(&mut bytes);
    read.map_err(|e| bad(&e.to_string()))?;
    if bytes.len() > PLAN_BYTES {
        return Err(bad(&format!("over {PLAN_BYTES} bytes")));
    }
    match String::from_utf8(bytes) {
        Ok(text) if !text.trim().is_empty() => Ok(text),
        Ok(_) => Err(bad("empty")),
        Err(_) => Err(bad("not UTF-8 text")),
    }
}

/// The pipeline the lane is stamped with, as this binary loads it.
fn lane_pipeline(item: &WorkItem) -> Result<&'static Pipeline, CliError> {
    let (id, hash) = (item.pipeline_id.as_deref(), item.pipeline_hash.as_deref());
    pipeline::by_stamp(id.unwrap_or_default(), hash.unwrap_or_default()).ok_or_else(|| {
        refuse(format!(
            "work item {} has no pipeline this binary loads",
            item.id
        ))
    })
}

/// The agent CLI `cli`, by its full path as a shell finds it, so the event
/// names the binary.
fn agent(cli: Cli) -> Result<Agent, CliError> {
    let bin = match cli {
        Cli::Claude => "claude",
        Cli::Codex => "codex",
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file());
    let deadline = Instant::now() + RUN_TIMEOUT;
    // As for test commands: no variable points its git elsewhere.
    let env = std::env::vars_os().filter(|(k, _)| !git::repo_var(k));
    Agent::probe(found.unwrap_or(bin.into()), env, deadline)
        .map_err(|e| refuse(format!("could not start {bin}: {e}")))
}

/// What a session is shown of commit `head` and the checkout it reads it
/// in: the files `diff` counts, their merge base, and the diff read in the
/// harness's fresh checkout (removed when it drops), where the lane's git
/// config does not reach, with the lane's goal from the ledger.
fn change(
    ledger: &Ledger,
    db: &Path,
    wt: &Path,
    item: &WorkItem,
    head: &str,
) -> Result<(Checkout, Facts), CliError> {
    let (base, files) = changed(wt, item.base_sha.as_deref(), head)?;
    let args = [
        "merge-base",
        "--end-of-options",
        &git::base_rev(&base),
        head,
    ];
    let (base, _) = harness_git(wt, &args, Instant::now() + RUN_TIMEOUT)?;
    let base = base.trim().to_string();
    let checkout = lane_checkout(db, wt, item.id, head, Instant::now() + CHECKOUT_TIMEOUT)?;
    let args = [
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
    ];
    let args = [&args[..], &[&base, head, "--"]].concat();
    let deadline = Instant::now() + RUN_TIMEOUT;
    let diff = match git::git(Path::new("git"), &checkout.dir, &args, deadline, DIFF_BYTES) {
        Ok((out, false)) => String::from_utf8_lossy(&out).into_owned(),
        Ok((_, true)) => return Err(refuse(format!("the diff is past {DIFF_BYTES} bytes"))),
        Err(_) => {
            return Err(refuse(format!(
                "could not diff {base} {head} in the checkout"
            )));
        }
    };
    let facts = Facts {
        base,
        head: head.into(),
        files,
        diff,
        goal: review::goal(ledger, item)?,
    };
    Ok((checkout, facts))
}

/// The harness's fresh checkout of the lane's commit, `checkouts/lane-<id>`
/// beside the ledger: one per lane, which the lane's lock keeps to one run.
fn lane_checkout(
    db: &Path,
    wt: &Path,
    work: i64,
    head: &str,
    deadline: Instant,
) -> Result<Checkout, CliError> {
    let db = std::path::absolute(db).map_err(LedgerError::from)?;
    let home = db.parent().unwrap_or(Path::new("/"));
    let dir = home.join("checkouts").join(format!("lane-{work}"));
    Checkout::fresh(wt, dir, head, deadline)
}

/// A fresh repository the harness owns at `dir` with the lane's commit
/// checked out, removed when it is dropped, however the run ends. Nothing
/// is kept between runs: test commands can write a repository's `.git`
/// (config, sparse checkout, excludes, index flags), so a kept one could
/// make a later commit's run read another tree. The lane's repository is
/// only read: `head` is fetched from it by sha, with no remote and no
/// other ref, so its hooks, config, filters, excludes and replace refs
/// never reach the checkout, and this repository runs no hook (not even
/// one a user's `init.templateDir` copied in).
struct Checkout {
    dir: PathBuf,
}

impl Checkout {
    fn fresh(
        lane: &Path,
        dir: PathBuf,
        head: &str,
        deadline: Instant,
    ) -> Result<Checkout, CliError> {
        // What a killed run left, first; one that cannot go refuses.
        match std::fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(refuse(format!("could not remove {}: {e}", dir.display())));
            }
            _ => {}
        }
        let mut owner_only = DirBuilder::new();
        owner_only.recursive(true).mode(0o700);
        owner_only.create(&dir).map_err(LedgerError::from)?;
        let checkout = Checkout { dir };
        let lane = lane.to_string_lossy();
        for args in [
            &["init", "-q"][..],
            &["config", "core.hooksPath", "/dev/null"],
            &["fetch", "-q", "--no-tags", &lane, head],
            &["checkout", "-q", "--force", "--detach", head],
        ] {
            harness_git(&checkout.dir, args, deadline)?;
        }
        Ok(checkout)
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The checkout after commands that all passed: whether `HEAD` is still
/// the commit under test, and `git status` with untracked and ignored
/// directories as one entry each, so its length does not grow with what a
/// build wrote. It targets accidental writes; an index flag such as
/// `assume-unchanged` hides a change from it.
struct After {
    head_moved: bool,
    dirty: bool,
    ignored: usize,
    /// The listing went past its cap: it is not read, and the run fails.
    cut: bool,
    listing: String,
}

fn tree_after(dir: &Path, head: &str, deadline: Instant) -> Result<After, CliError> {
    let (now, _) = harness_git(dir, &["rev-parse", "--verify", "HEAD"], deadline)?;
    let args = [
        "status",
        "--porcelain",
        "--untracked-files=normal",
        "--ignored",
    ];
    let (status, cut) = harness_git(dir, &args, deadline)?;
    let ignored = status.lines().filter(|l| l.starts_with("!! ")).count();
    Ok(After {
        head_moved: now.trim() != head,
        dirty: !cut && status.lines().count() > ignored,
        ignored,
        cut,
        listing: format!("$ git {}\n{status}", args.join(" ")),
    })
}

/// One git call in the harness's checkout: its output and whether it was
/// cut at 1 MiB. A failure refuses with git's own line.
fn harness_git(dir: &Path, args: &[&str], deadline: Instant) -> Result<(String, bool), CliError> {
    match git::git(Path::new("git"), dir, args, deadline, 1024 * 1024) {
        Ok((out, cut)) => Ok((String::from_utf8_lossy(&out).into_owned(), cut)),
        Err(git::Failure::TimedOut) => Err(refuse(format!("git {}: timed out", args[0]))),
        Err(git::Failure::BadRevision) => Err(refuse(format!("git {}: no such commit", args[0]))),
        Err(git::Failure::Other(e)) => Err(refuse(e)),
    }
}

/// What the test commands did: a row for each that ran, the end of each
/// one's output, and whether all passed and whether the time ran out.
struct Ran {
    commands: Vec<Value>,
    text: String,
    passed: bool,
    timed_out: bool,
    /// Some command's output lost its start.
    cut: bool,
}

/// Each argv in order in `dir`, until one fails, with stdin empty and the
/// environment less the session's variables and those that point git at
/// another repository, plus `target` (the variable and the lane's build
/// directory). Only the end of each stream is kept while it runs.
fn run_commands(
    commands: &[Vec<String>],
    dir: &Path,
    target: Option<(OsString, OsString)>,
    deadline: Instant,
) -> Result<Ran, CliError> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !session_var(k) && !git::repo_var(k))
        .collect();
    env.extend(target);
    let mut ran = Ran {
        commands: Vec::new(),
        text: String::new(),
        passed: true,
        timed_out: false,
        cut: false,
    };
    for argv in commands {
        let started = Instant::now();
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(dir)
            .env_clear()
            .envs(env.iter().cloned());
        ran.text += &format!("$ {argv:?}\n");
        // A command killed by a signal has no exit code: a failure.
        let exit = match child::run_tail(&mut cmd, deadline, TAIL_BYTES) {
            Ok(done) => {
                ran.text += &String::from_utf8_lossy(&done.err);
                ran.text += &String::from_utf8_lossy(&done.out);
                ran.cut |= done.cut;
                done.status.code()
            }
            Err(child::Failed::TimedOut) => {
                ran.timed_out = true;
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
        ran.commands
            .push(json!({"argv": argv, "exit": exit, "ms": ms}));
        if exit != Some(0) {
            ran.passed = false;
            break;
        }
    }
    Ok(ran)
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
