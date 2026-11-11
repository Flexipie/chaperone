//! `chaperone check`, `verify`, `pr open`, `merge`, `reviews`, `jobs`, `usage`, `attention`, `finding` and `dossier`. `reviews record` runs
//! `daisy-cli reviews get`, the CLI's only call into daisy, before the
//! write opens its transaction. `check run` reads git in the lane's
//! worktree, and runs tests and the review's and the verdict's sessions in
//! a checkout of its commit, before its write, under the lane's run lock.

use std::ffi::{OsStr, OsString};
use std::fs::{DirBuilder, File};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::checks::{
    Attached, AttentionInput, CheckInput, FindingMove, RunInput, evidence_ok,
};
use chaperone_core::child;
use chaperone_core::dossier::NoteInput;
use chaperone_core::git::{self, ChangedFile};
use chaperone_core::ledger::{self, Actor, Evidence, Kept, Ledger, LedgerError, Refusal};
use chaperone_core::merge::{self, Done};
use chaperone_core::pipeline::{self, Cli, Fact, Pipeline};
use chaperone_core::plan;
use chaperone_core::review::{self, Agents, Facts};
use chaperone_core::runner::{Agent, session_var};
use chaperone_core::usage::Roots;
use chaperone_core::verdict;
use chaperone_core::work::{UpdateInput, WorkItem, is_terminal};
use chaperone_graph::{Companion, code_globs, commands_in, declares, glob};
use serde_json::{Map, Value, json};

use crate::cli::{
    AttentionCmd, CheckCmd, DossierCmd, FindingCmd, JobsCmd, ReviewsCmd, RunKind, UsageCmd,
    WaiveKind,
};
use crate::output::{CliError, ErrorBody, Meta, Outcome};
use crate::work::{items, observed_head, one};

/// Where spawn and the skills call daisy (R15); `CHAPERONE_DAISY_CLI`
/// overrides it.
const DAISY_CLI: &str = "/Applications/daisy.app/Contents/MacOS/daisy-cli";
const DAISY_TIMEOUT: Duration = Duration::from_secs(30);
/// Not code: docs and Markdown, unless the base profile's `[diff] code`
/// names the file. A lock file is code: a dependency bump changes what
/// runs.
const NOT_CODE: [&str; 2] = ["docs/**", "**/*.md"];
/// The git calls that read the lane's commit, its tree and its numstat.
const RUN_TIMEOUT: Duration = Duration::from_secs(10);
/// `check run tests`, when the profile names no `timeout_secs`.
const TESTS_TIMEOUT: Duration = Duration::from_secs(1800);
/// `check run review`'s checkout of the lane's commit, which fetches its
/// whole history.
const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(600);
/// `pr open`'s push and each of its gh calls.
const FORGE_TIMEOUT: Duration = Duration::from_secs(120);
/// Of each test command's stdout and stderr, the end kept as evidence.
const TAIL_BYTES: usize = 16 * 1024;
/// A plan file: a document a person reads, far above any real one.
const PLAN_BYTES: usize = 64 * 1024;
/// A smoke evidence file: a log or an output; the store keeps its end.
const EVIDENCE_BYTES: usize = 1024 * 1024;
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
            evidence,
            run,
            actor,
        } => {
            let mut input = CheckInput {
                work,
                check,
                head,
                outcome,
                reason,
                decision,
                findings,
                run: None,
            };
            let actor = actor.resolve();
            if evidence.is_empty() && run.is_none() {
                return one(ledger.check_record(&actor, &input)??);
            }
            // Every rule before the script runs or any file is stored: the
            // files are read and named, the record is tried and rolled back
            // (the run as it would be if it passed), and only then does the
            // script run and are they written, before the record that names
            // them.
            evidence_ok(&input.check, evidence.len() + usize::from(run.is_some()))?;
            let item = ledger.work_get(work, &ledger.now()?)??;
            let wt = item.worktree_path.as_deref();
            let mut read: Vec<_> = run
                .iter()
                .chain(&evidence)
                .map(|path| attach(wt, path))
                .collect::<Result<_, _>>()?;
            let argv = || vec!["bash".into(), "--".into(), read[0].0.path.clone()];
            if run.is_some() {
                input.run = Some(json!({"argv": argv(), "exit": 0}));
            }
            let files: Vec<_> = read.iter().map(|(file, _)| file.clone()).collect();
            ledger.check_record_with(&actor, &input, &files, true)??;
            if run.is_some() {
                let (ran, exit) = run_script(wt, argv(), &read[0])?;
                input.run = Some(json!({"argv": argv(), "exit": exit}));
                read[0] = ran;
            }
            let store = Evidence::beside(db);
            for (_, kept) in &read {
                store.write(kept).map_err(LedgerError::from)?;
            }
            let files: Vec<_> = read.iter().map(|(file, _)| file.clone()).collect();
            one(ledger.check_record_with(&actor, &input, &files, false)??)
        }
        CheckCmd::Run {
            kind,
            work,
            file,
            actor,
        } => {
            let actor = actor.resolve();
            let _lock = ledger::lock_lane(db, work)??;
            let _ = sweep(ledger, db);
            match (kind, file) {
                (RunKind::Plan, Some(file)) => run_plan(ledger, db, work, &file, &actor),
                (RunKind::Plan, None) | (_, Some(_)) => Err(refuse(
                    "--file <plan> goes with plan, and is required there".into(),
                )),
                (RunKind::Diff, None) => run_diff(ledger, db, work, &actor),
                (RunKind::Tests, None) => run_tests(ledger, db, work, &actor, "tests"),
                (RunKind::Smoke, None) => run_tests(ledger, db, work, &actor, "smoke"),
                (RunKind::Review, None) => run_review(ledger, db, work, &actor),
                (RunKind::Verdict, None) => run_verdict(ledger, db, work, &actor),
            }
        }
        CheckCmd::Waive {
            kind: WaiveKind::Plan,
            work,
            reason,
            actor,
        } => {
            let item = ledger.work_get(work, &ledger.now()?)??;
            plan::plan(lane_pipeline(&item)?).map_err(|e| {
                refuse(format!(
                    "{e}; a lane on it records a skipped plan check with `check record`"
                ))
            })?;
            let head = observed_head(item.worktree_path.as_deref(), item.branch.as_deref())?;
            let head = head.ok_or_else(|| {
                refuse(format!(
                    "work item {work} has no worktree to read its commit in"
                ))
            })?;
            one(ledger.plan_waive(&actor.resolve(), work, &head, &reason)??)
        }
        CheckCmd::Prune => one(sweep(ledger, db)?),
    }
}

/// `verify`: `check run` diff, tests, review and verdict on the lane's
/// commit under one hold of its run lock, until a step is refused or does
/// not pass, or the lane moves off the diff's commit; the verdict runs only
/// after a complete review that leaves no finding blocking, as the gate
/// counts them. One result: each step, and where and why it stopped.
pub fn verify(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let _lock = ledger::lock_lane(db, work)??;
    let _ = sweep(ledger, db);
    let (mut steps, mut stopped, mut from) = (Vec::new(), Value::Null, None::<String>);
    for check in ["diff", "tests", "review", "verdict"] {
        if let Some(from) = &from {
            let item = ledger.work_get(work, &ledger.now()?)??;
            let (wt, branch) = (item.worktree_path.as_deref(), item.branch.as_deref());
            if let Some(now) = observed_head(wt, branch)?
                && now != *from
            {
                let why = format!("the lane moved from {from} to {now}");
                stopped = json!({"check": check, "why": why});
                break;
            }
        }
        let ran = match check {
            "diff" => run_diff(ledger, db, work, actor),
            "tests" => run_tests(ledger, db, work, actor, check),
            "review" => run_review(ledger, db, work, actor),
            _ => run_verdict(ledger, db, work, actor),
        };
        let recorded = match ran {
            Ok(Outcome::One(recorded)) => recorded,
            Err(CliError::Refused(Refusal::InvalidInput(why))) => {
                stopped = json!({"check": check, "why": why});
                break;
            }
            other => return other,
        };
        let p = &recorded["payload"];
        let head = p["head_sha"].as_str().unwrap_or_default();
        from.get_or_insert_with(|| head.into());
        let why = match check {
            "tests" if p["result"] != "pass" => "tests failed".into(),
            // The gate's own count: complete harness reviews' findings only.
            "review" => match ledger
                .project(work, Some(head))?
                .get("findings.open_blocking")
            {
                Fact::Int(0) => String::new(),
                Fact::Int(n) => format!("{n} finding(s) still block"),
                _ => "review not complete".into(),
            },
            "verdict" if p["decision"] != "yes" => "the verdict is not yes".into(),
            _ => String::new(),
        };
        steps.push(
            json!({"check": check, "head": head, "event_id": recorded["event_id"],
            "replayed": recorded["replayed"], "passed": why.is_empty()}),
        );
        if !why.is_empty() {
            stopped = json!({"check": check, "why": why});
            break;
        }
    }
    one(json!({"work_id": work, "steps": steps, "stopped": stopped}))
}

/// `pr open`: under the lane's run lock, on a clean worktree at the tip of
/// the lane's branch, `check run` diff and tests on that commit, refused
/// past the pipeline's `code_lines` limit or when the tests fail. Then the
/// push of that commit and `gh pr create`, or `gh pr edit` on the PR the
/// lane holds or gh lists for the branch, with [`pr_body`]; a PR new to the
/// ledger is `pr.opened` on the commit.
pub fn pr_open(
    ledger: &Ledger,
    db: &Path,
    work: i64,
    title: Option<String>,
    draft: bool,
    gh: &Path,
    actor: &Actor,
) -> Result<Outcome, CliError> {
    let _lock = ledger::lock_lane(db, work)??;
    let Run { wt, item, input } = start_run(ledger, work, "diff")?;
    let (head, branch) = (input.head, item.branch.clone().unwrap_or_default());
    // `start_run` reads HEAD when the branch is gone.
    let tip = format!("refs/heads/{branch}");
    let tip = git::commit(Path::new("git"), &wt, &tip, Instant::now() + RUN_TIMEOUT);
    let found = tree_after(&wt, &head, Instant::now() + RUN_TIMEOUT)?;
    if tip.map_err(refuse)?.is_none() || found.dirty || found.head_moved || found.cut {
        return Err(refuse(format!(
            "pr open needs the worktree {} clean at the tip of the lane's branch ({branch}, \
             {head}): commit or stash the change, check out the branch, and run again",
            wt.display()
        )));
    }
    // Before any push: a PR the ledger then refuses would go unrecorded.
    if item.pr_ref.is_none() {
        ledger.work_update(actor, work, &UpdateInput::default(), None, None, true)??;
    }
    let mut steps = Vec::new();
    let mut step = |check: &str, ran: Result<Outcome, CliError>| match ran? {
        Outcome::One(rec) if rec["payload"]["head_sha"] == head => {
            steps.push(
                json!({"check": check, "head": head, "event_id": rec["event_id"],
                "replayed": rec["replayed"], "passed": true}),
            );
            Ok(rec["payload"].clone())
        }
        _ => Err(refuse(format!("the lane moved from {head}; run again"))),
    };
    let diff = step("diff", run_diff(ledger, db, work, actor))?;
    let (lines, uncounted) = (&diff["code_lines"], &diff["uncounted_files"]);
    let over = |limit: &&i64| lines.as_i64().is_none_or(|n| n > **limit) || uncounted != 0;
    if let Some(limit) = lane_pipeline(&item)?.limits.get("code_lines").filter(over) {
        return Err(refuse(format!(
            "the diff has {lines} code lines and {uncounted} uncounted (binary) files; the \
             pipeline allows {limit} and none: split the lane"
        )));
    }
    let base = git::base_rev(diff["base"].as_str().unwrap_or_default());
    let args = ["rev-parse", "--abbrev-ref", &base];
    let (named, _) = harness_git(&wt, &args, Instant::now() + RUN_TIMEOUT)?;
    let no_origin = || refuse(format!("the base {base} is no origin branch"));
    let base = named.trim().strip_prefix("origin/").ok_or_else(no_origin)?;
    let tests = step("tests", run_tests(ledger, db, work, actor, "tests"))?;
    if tests["result"] != "pass" {
        return Err(refuse(format!(
            "the tests failed on {head} (`check run tests` evidence {}): fix them and run again",
            tests["evidence_ref"]
        )));
    }
    let gh = |args: &[&str], input: Option<&[u8]>| {
        let failed = |why: &str| CliError::Internal(format!("gh pr {}: {why}", args[1]));
        let mut cmd = Command::new(gh);
        let deadline = Instant::now() + FORGE_TIMEOUT;
        match child::run_input(cmd.args(args).current_dir(&wt), input, deadline, 1 << 20) {
            Ok(done) if done.status.success() => {
                Ok(String::from_utf8_lossy(&done.out).into_owned())
            }
            Ok(done) => Err(failed(String::from_utf8_lossy(&done.err).trim())),
            Err(child::Failed::TimedOut) => Err(failed("timed out")),
            Err(child::Failed::Spawn(e) | child::Failed::Wait(e)) => Err(failed(&e.to_string())),
        }
    };
    // gh names a PR by its URL: https://github.com/<owner>/<repo>/pull/<n>.
    let pr_of = |url: &str| {
        let (repo, n) = url.trim().split_once("/pull/")?;
        let mut parts = repo.rsplit('/');
        let (name, owner) = (parts.next()?, parts.next()?);
        Some(format!("{owner}/{name}#{n}"))
    };
    // One a run opened and never recorded.
    let listed = match item.pr_ref {
        Some(_) => None,
        None => pr_of(&gh(
            &[
                "pr", "list", "--head", &branch, "--json", "url", "--jq", ".[0].url",
            ],
            None,
        )?),
    };
    let held = item.pr_ref.clone().or(listed.clone());
    let body = pr_body(ledger, db, &item, held.as_deref(), &head, &diff, &tests)?;
    let push = format!("{head}:refs/heads/{branch}");
    harness_git(
        &wt,
        &["push", "origin", &push],
        Instant::now() + FORGE_TIMEOUT,
    )?;
    let mut args = match held.as_deref().and_then(|pr| pr.rsplit_once('#')) {
        Some((repo, n)) => vec!["pr", "edit", n, "--repo", repo],
        None => vec!["pr", "create", "--base", base, "--head", &branch],
    };
    let title = title.or(held.is_none().then(|| item.title.clone()));
    if let Some(title) = &title {
        args.extend(["--title", title]);
    }
    args.extend(["--body-file", "-"]);
    args.extend((draft && held.is_none()).then_some("--draft"));
    let out = gh(&args, Some(body.as_bytes()))?;
    let pr = match held {
        Some(pr) => pr,
        None => pr_of(&out)
            .ok_or_else(|| CliError::Internal(format!("gh pr create: no PR URL in {out:?}")))?,
    };
    if item.pr_ref.is_none() {
        let input = UpdateInput {
            pr_ref: Some(pr.clone()),
            head_sha: Some(head.clone()),
            ..UpdateInput::default()
        };
        ledger.work_update(actor, work, &input, None, None, false)??;
    }
    let created = item.pr_ref.is_none() && listed.is_none();
    one(json!({"work_id": work, "pr": pr, "head": head, "created": created, "steps": steps}))
}

/// `merge`: [`merge::merge`] of the lane's PR.
pub fn merge(
    ledger: &Ledger,
    work: i64,
    head: Option<String>,
    actor: &Actor,
) -> Result<Outcome, CliError> {
    let item = ledger.work_get(work, &ledger.now()?)??;
    let pr = item
        .pr_ref
        .ok_or_else(|| refuse(format!("work item {work} has no PR")))?;
    let (gh, herdr, head) = (OsStr::new("gh"), OsStr::new("herdr"), head.as_deref());
    let merged = merge::merge(ledger, actor, gh, herdr, work, &pr, head, "chaperone merge")
        .map_err(CliError::Internal)?
        .map_err(refuse)?;
    let (done_by, told, tell_error) = match merged.done {
        Done::Was => ("already", false, None),
        Done::HandedOff => ("merge", false, None),
        Done::ByLane(told) => ("lane", told.is_ok(), told.err()),
    };
    one(
        json!({"work_id": work, "pr": pr, "merge_sha": merged.sha, "done_by": done_by,
        "told": told, "tell_error": tell_error}),
    )
}

/// A PR's body from ledger facts: of the lane's own words only its title,
/// or the goal its newest handoff states. A GitHub lane's issue is closed
/// as `#N` only when `pr` (the PR, once it exists) is in the issue's repo.
fn pr_body(
    ledger: &Ledger,
    db: &Path,
    item: &WorkItem,
    pr: Option<&str>,
    head: &str,
    diff: &Value,
    tests: &Value,
) -> Result<String, CliError> {
    let brief = ledger
        .work_handoffs(item.id)?
        .pop()
        .and_then(|h| h.handoff.brief);
    let goal = brief.map(|b| b.goal).filter(|g| !g.is_empty());
    let mut body = goal.unwrap_or(item.title.clone()) + "\n\n";
    if let Some(key) = &item.source_key {
        body += &format!("Tracker: {key}\n\n");
    }
    let issue = item
        .source_id
        .as_deref()
        .filter(|_| item.source == "github");
    let issue = issue.and_then(|id| id.split_once('#'));
    if let Some((repo, n)) = issue.filter(|(r, n)| r.contains('/') && n.parse::<u64>().is_ok()) {
        let pr_repo = pr.and_then(|pr| pr.split_once('#')).map(|(r, _)| r);
        let local = pr_repo.is_some_and(|r| r.eq_ignore_ascii_case(repo));
        body += &format!("Closes {}#{n}\n\n", if local { "" } else { repo });
    }
    let commands = tests["commands"].as_array().into_iter().flatten();
    let commands: Vec<_> = commands
        .map(|c| {
            let argv = c["argv"].as_array().into_iter().flatten();
            format!(
                "`{}`",
                argv.filter_map(Value::as_str).collect::<Vec<_>>().join(" ")
            )
        })
        .collect();
    let facts = ledger.project(item.id, Some(head))?;
    let plan = match (
        facts.get("plan_check.outcome"),
        facts.get("plan_check.reason"),
    ) {
        (Fact::Str(outcome), Fact::Str(reason)) => format!("{outcome} ({reason})"),
        (Fact::Str(outcome), _) => outcome.clone(),
        _ => "none".into(),
    };
    let smoke = verdict::smoke(ledger, &Evidence::beside(db), item.id, head)?;
    let mut smokes = Vec::new();
    if let Some(result) = smoke["harness"]["result"].as_str() {
        smokes.push(format!("harness run {result}"));
    }
    let claim = &smoke["lane_claim"];
    if let Some(outcome) = claim["outcome"].as_str() {
        let files = claim["evidence_files"].as_array().map_or(0, Vec::len);
        smokes.push(format!("session claim {outcome}, {files} evidence files"));
    }
    body += &format!(
        "- Size: {} code lines in {} files\n- Tests: passed on {head}: {}\n\
         - Plan check: {plan}\n- Smoke: {}\n\nMade by `chaperone pr open` from the ledger.\n",
        diff["code_lines"],
        diff["files"],
        commands.join(", "),
        if smokes.is_empty() {
            "none".into()
        } else {
            smokes.join("; ")
        },
    );
    Ok(body)
}

/// A `check run` under way, its caller holding the lane's run lock: the
/// lane's worktree and work item, and the input with the commit the lane
/// is on (as a ready handoff reads it), its tree and this executable.
struct Run {
    wt: PathBuf,
    item: WorkItem,
    input: RunInput,
}

fn start_run(ledger: &Ledger, work: i64, check: &str) -> Result<Run, CliError> {
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
    Ok(Run { wt, item, input })
}

/// Refuses while a finding of the lane's newest plan check has no answer
/// from the lane, naming each and the commands that answer it.
fn plan_answered(ledger: &Ledger, work: i64) -> Result<(), CliError> {
    let open = ledger.plan_findings(work)?.into_iter();
    let open: Vec<_> = open
        .filter(|f| f.status == "open")
        .map(|f| format!("{}/{} ({})", f.check_event_id, f.finding_key, f.title))
        .collect();
    if open.is_empty() {
        return Ok(());
    }
    Err(refuse(format!(
        "{} plan finding(s) have no answer: {}; answer each with `chaperone finding accept \
         <event> <key> --reason <how>`, `chaperone finding dispute <event> <key> --reason <why>` \
         or, for a question, `chaperone finding answer <event> <key> --reason <answer>`",
        open.len(),
        open.join("; ")
    )))
}

fn refuse(why: String) -> CliError {
    Refusal::InvalidInput(why).into()
}

/// `check run diff`: the lane's commit (as a ready handoff reads it)
/// against the merge base, committed files only. The per-file list goes to
/// the evidence store, its totals in the payload.
fn run_diff(ledger: &Ledger, db: &Path, work: i64, actor: &Actor) -> Result<Outcome, CliError> {
    let Run {
        wt,
        item,
        mut input,
    } = start_run(ledger, work, "diff")?;
    plan_answered(ledger, work)?;
    let (base, files) = changed(&wt, item.base_sha.as_deref(), &input.head)?;
    let found = code_globs(Path::new("git"), &wt, Instant::now() + RUN_TIMEOUT)?;
    // The profile says what code is, so a changed one measures again.
    input.profile = found.as_ref().map(|f| f.hash.clone());
    let globs = found.as_ref().map_or(&[][..], |f| &f.code);
    let rules = found.as_ref().map_or(&[][..], |f| &f.companions);
    companions(&wt, &base, &input.head, &files, rules)?;
    let (code, code_lines) = code(&files, globs);
    let added: u64 = files.iter().filter_map(|f| f.added).sum();
    let removed: u64 = files.iter().filter_map(|f| f.removed).sum();
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
    if let Some(f) = &found {
        let at = json!({"ref": f.base, "commit": f.commit});
        input.fields.insert("profile_base".into(), at);
    }
    input.evidence_ref = Some(stored.hash);
    one(ledger.check_run(actor, &input)??)
}

/// Refuses a commit whose `files` break a companion rule (a file the
/// rule's `when` names changed, none its `needs` names did) unless a lane
/// commit, `base..head`, waives it with a `No-companion: <rule> <why>`
/// trailer whose why is not empty. The log is read only when a rule is broken.
fn companions(
    wt: &Path,
    base: &str,
    head: &str,
    files: &[ChangedFile],
    rules: &[Companion],
) -> Result<(), CliError> {
    let hit = |globs: &[String]| {
        files
            .iter()
            .find(|f| globs.iter().any(|g| glob(g, &f.path)))
    };
    let broken: Vec<_> = rules
        .iter()
        .filter_map(|r| Some((r, hit(&r.when)?)))
        .filter(|(r, _)| hit(&r.needs).is_none())
        .collect();
    if broken.is_empty() {
        return Ok(());
    }
    let range = format!("{}..{head}", git::base_rev(base));
    let format = "--format=%(trailers:key=No-companion,valueonly,unfold)";
    let args = ["log", format, "--end-of-options", &range, "--"];
    let (log, cut) = harness_git(wt, &args, Instant::now() + RUN_TIMEOUT)?;
    if cut {
        return Err(refuse(
            "the No-companion trailer log is over the output limit, so a waiver in it \
             cannot be read"
                .into(),
        ));
    }
    // One line per trailer (`unfold`: a continuation line is no trailer of
    // its own): the rule's name, then a reason; a bare name records no why.
    let waived = |name: &str| {
        let given = |r: &str| {
            let r = r.trim_start_matches(':');
            r.starts_with(char::is_whitespace) && !r.trim().is_empty()
        };
        log.lines()
            .any(|l| l.trim().strip_prefix(name).is_some_and(given))
    };
    let why: Vec<_> = broken
        .iter()
        .filter(|(r, _)| !waived(&r.name))
        .map(|(r, f)| {
            let needs = r.needs.join(" or ");
            format!(
                "companion rule {}: {} changed but not {needs}",
                r.name, f.path
            )
        })
        .collect();
    if why.is_empty() {
        return Ok(());
    }
    Err(refuse(format!(
        "{}; change it too, or add a trailer `No-companion: <rule> <why>` to a lane commit",
        why.join("; ")
    )))
}

/// Of `files`, the code ones (named by `globs`, else not [`NOT_CODE`]),
/// and their lines added plus removed.
fn code<'a>(files: &'a [ChangedFile], globs: &[String]) -> (Vec<&'a str>, u64) {
    let code = files.iter().filter(|f| {
        globs.iter().any(|g| glob(g, &f.path)) || !NOT_CODE.iter().any(|g| glob(g, &f.path))
    });
    code.fold((Vec::new(), 0), |(mut paths, lines), f| {
        paths.push(f.path.as_str());
        (paths, lines + f.added.unwrap_or(0) + f.removed.unwrap_or(0))
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

/// `check run tests` (or `smoke`, `check` naming the profile's table and
/// the event's check): the `[tests]` commands of the base branch's profile,
/// in order until one fails, in a fresh checkout of the lane's commit that
/// the harness makes and removes, then the checkout must still be at that
/// commit with no change or new untracked file. A pass on the same commit
/// and profile replays without running anything. A run that times out
/// failed: the tests did not pass on that tree. No profile or table, a
/// checkout that cannot be made or a command that cannot start writes
/// nothing, nor does `smoke` on a lane whose pipeline has no `verdict`.
fn run_tests(
    ledger: &Ledger,
    db: &Path,
    work: i64,
    actor: &Actor,
    check: &str,
) -> Result<Outcome, CliError> {
    let Run {
        wt,
        item,
        mut input,
    } = start_run(ledger, work, check)?;
    // A harness smoke is named like a session check and passes v19's
    // `smoke_recorded` from the lane's own session, so only a lane whose
    // pipeline judges with harness checks may run one.
    if check == "smoke" {
        let pipeline = lane_pipeline(&item)?;
        if !pipeline.checks.contains_key(verdict::CHECK) {
            return Err(refuse(format!(
                "pipeline {} has no check {}, so a harness smoke would stand in for one a \
                 session that never held the lane records; a lane on it records smoke with \
                 `check record`",
                pipeline.id,
                verdict::CHECK
            )));
        }
    }
    let base = commands_in(Path::new("git"), &wt, check, Instant::now() + RUN_TIMEOUT)?;
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
        wt,
        item,
        mut input,
    } = start_run(ledger, work, "review")?;
    plan_answered(ledger, work)?;
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
        wt,
        item,
        mut input,
    } = start_run(ledger, work, verdict::EVENT_CHECK)?;
    let pipeline = lane_pipeline(&item)?;
    let (planned, attempts) = verdict::plan(pipeline).map_err(|e| {
        refuse(format!(
            "{e}; a lane on it records verdicts with `check record`"
        ))
    })?;
    let (head, store) = (input.head.clone(), Evidence::beside(db));
    let smoke = declares(Path::new("git"), &wt, "smoke", Instant::now() + RUN_TIMEOUT)?;
    let harness = verdict::harness(ledger, &store, work, &head, smoke)?.map_err(|lacks| {
        refuse(format!(
            "commit {head} has no harness {lacks}; a verdict runs only on a clean commit: a \
             `check run diff`, `tests` that pass, a complete `review` and, when the base \
             profile declares `[smoke]`, a `smoke` that passes"
        ))
    })?;
    let key = (&*pipeline.hash, attempts);
    if let Some(stored) = verdict::replay(ledger, actor, &mut input, key, &harness)?? {
        return one(stored);
    }
    let cli = planned.runner.cli;
    let started = agent(cli)?;
    let (checkout, facts) = change(ledger, db, &wt, &item, &head)?;
    let agents = Agents {
        claude: (cli == Cli::Claude).then_some(&started),
        codex: (cli == Cli::Codex).then_some(&started),
    };
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

/// `check run plan`: the pipeline must name a `plan` check, and the lane's
/// commit must change no code file against its base, as `diff` counts
/// code, or the run is refused and names the owner's waiver. Then the plan
/// file is read here and stored as evidence (redacted, which is what the
/// reviewer is sent); the same plan on the same tree and pipeline replays. Else one fresh
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
        wt,
        item,
        mut input,
    } = start_run(ledger, work, plan::EVENT_CHECK)?;
    let pipeline = lane_pipeline(&item)?;
    let (planned, attempts) = plan::plan(pipeline).map_err(|e| {
        refuse(format!(
            "{e}; a lane on it records plan checks with `check record`"
        ))
    })?;
    let head = input.head.clone();
    let (base, files) = changed(&wt, item.base_sha.as_deref(), &head)?;
    let found = code_globs(Path::new("git"), &wt, Instant::now() + RUN_TIMEOUT)?;
    let globs = found.map_or_else(Vec::new, |f| f.code);
    no_code(&files, &globs, &head, work)?;
    // Named as the store keeps it, redacted; written only once a session
    // runs on it (`plan::record`).
    let kept = Kept::of(&plan_text(file)?);
    let key = (&*pipeline.hash, attempts);
    if let Some(stored) = plan::replay(ledger, &mut input, key, &kept.stored.hash)?? {
        return one(stored);
    }
    let cli = planned.runner.cli;
    let started = agent(cli)?;
    let base = merge_base(&wt, &base, &head)?;
    let checkout = lane_checkout(db, &wt, work, &head, Instant::now() + CHECKOUT_TIMEOUT)?;
    let agents = Agents {
        claude: (cli == Cli::Claude).then_some(&started),
        codex: (cli == Cli::Codex).then_some(&started),
    };
    let given = plan::Given {
        goal: &review::goal(ledger, &item)?,
        plan: &kept,
        base: &base,
    };
    let store = Evidence::beside(db);
    one(plan::record(
        ledger,
        actor,
        input,
        &planned,
        &agents,
        &checkout.dir,
        &given,
        &store,
    )??)
}

/// Refuses a commit `head` whose `files` change code, naming up to three
/// of them and the owner's waiver: the plan check runs before any code.
fn no_code(files: &[ChangedFile], globs: &[String], head: &str, work: i64) -> Result<(), CliError> {
    let (code, lines) = code(files, globs);
    if code.is_empty() {
        return Ok(());
    }
    let (n, some) = (code.len(), code[..code.len().min(3)].join(", "));
    Err(refuse(format!(
        "commit {head} changes code against its base ({n} code file(s), {lines} lines: \
         {some}): the plan check runs before any code. The owner can waive it with \
         `check waive plan --work {work}`"
    )))
}

/// The plan at `path`: a regular file (a link to one is followed) of at
/// most [`PLAN_BYTES`] of UTF-8 text that is not blank. It may sit outside
/// the worktree, as plans are kept in the agent's own folder; it is the
/// lane's text, given to the reviewer as fenced data and stored redacted.
fn plan_text(path: &Path) -> Result<String, CliError> {
    let bad = |why: &str| refuse(format!("plan {}: {why}", path.display()));
    match text_file(path, PLAN_BYTES) {
        Ok(text) if !text.trim().is_empty() => Ok(text),
        Ok(_) => Err(bad("empty")),
        Err(why) => Err(bad(&why)),
    }
}

/// Smoke evidence `path` for the lane whose worktree is `wt`, by R13's
/// rules: its real path (links resolved) inside the worktree's and not
/// under `.git` or `.chaperone` there, a regular file of at most
/// [`EVIDENCE_BYTES`] of UTF-8 text with no NUL. A relative path is read
/// from the worktree's root, whoever records it. Named as the store will
/// keep it (redacted), not yet written.
fn attach(wt: Option<&str>, path: &Path) -> Result<(Attached, Kept), CliError> {
    let bad = |why: &str| refuse(format!("evidence {}: {why}", path.display()));
    let wt = wt.map(Path::new).filter(|w| w.is_absolute());
    let wt = wt.and_then(|w| w.canonicalize().ok());
    let wt = wt.ok_or_else(|| bad("the work item has no worktree to read it in"))?;
    let real = wt
        .join(path)
        .canonicalize()
        .map_err(|e| bad(&e.to_string()))?;
    let inside = real
        .strip_prefix(&wt)
        .map_err(|_| bad("outside the lane's worktree"))?;
    if inside
        .components()
        .any(|c| matches!(c.as_os_str().to_str(), Some(".git" | ".chaperone")))
    {
        return Err(bad("under .git or .chaperone"));
    }
    let text = text_file(&real, EVIDENCE_BYTES).map_err(|why| bad(&why))?;
    if text.contains('\0') {
        return Err(bad("not UTF-8 text"));
    }
    let kept = Kept::of(&text);
    let stored = &kept.stored;
    let file = Attached {
        path: inside.display().to_string(),
        sha256: stored.hash.clone(),
        bytes: text.len() as u64,
        truncated: stored.truncated,
        redactions: stored.redactions,
    };
    Ok((file, kept))
}

/// A smoke claim's `argv` (bash, `--` and its script, `script` as read) run in
/// the worktree `wt`'s root as `check run` runs a command: the script,
/// the command and the end of its stderr and stdout, then its exit code,
/// as the evidence file named for the script, and that code (null when it
/// was killed or timed out). It is truncated when a stream lost its start
/// too, and counts the script's redactions. A script that cannot start
/// writes nothing.
fn run_script(
    wt: Option<&str>,
    argv: Vec<String>,
    (script, read): &(Attached, Kept),
) -> Result<((Attached, Kept), Value), CliError> {
    let dir = Path::new(wt.unwrap_or_default());
    let deadline = Instant::now() + TESTS_TIMEOUT;
    let ran = run_commands(std::slice::from_ref(&argv), dir, None, deadline)?;
    let exit = ran.commands[0]["exit"].clone();
    let end = if ran.timed_out { " (timed out)" } else { "" };
    let text = format!("{}\n{}\nexit {exit}{end}\n", read.text, ran.text);
    let kept = Kept::of(&text);
    let stored = &kept.stored;
    let file = Attached {
        path: argv[2].clone(),
        sha256: stored.hash.clone(),
        bytes: text.len() as u64,
        truncated: stored.truncated || ran.cut,
        redactions: stored.redactions + script.redactions,
    };
    Ok(((file, kept), exit))
}

/// The UTF-8 text of the regular file at `path`, at most `max` bytes, or
/// why not. The type is checked before the open: opening a FIFO would wait
/// for a writer, with the lane's lock held. Between the check and the open
/// a file could be swapped for one; this guards against accidents, not a
/// lane racing its own check (no non-blocking open without libc).
fn text_file(path: &Path, max: usize) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let read = file.take(max as u64 + 1).read_to_end(&mut bytes);
    read.map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err(format!("over {max} bytes"));
    }
    String::from_utf8(bytes).map_err(|_| "not UTF-8 text".into())
}

/// The pipeline the lane is stamped with, as this binary loads it.
pub(crate) fn lane_pipeline(item: &WorkItem) -> Result<&'static Pipeline, CliError> {
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

/// The merge base of `head` with `base` (as `changed` names it).
fn merge_base(wt: &Path, base: &str, head: &str) -> Result<String, CliError> {
    let args = ["merge-base", "--end-of-options", &git::base_rev(base), head];
    let (base, _) = harness_git(wt, &args, Instant::now() + RUN_TIMEOUT)?;
    Ok(base.trim().to_string())
}

/// What a session is shown of commit `head` and the checkout it reads it
/// in: the files `diff` counts, their merge base, and the diff read in the
/// harness's fresh checkout (removed when it drops), where the lane's git
/// config does not reach, with the lane's goal and the commit's smoke from
/// the ledger.
fn change(
    ledger: &Ledger,
    db: &Path,
    wt: &Path,
    item: &WorkItem,
    head: &str,
) -> Result<(Checkout, Facts), CliError> {
    let (base, files) = changed(wt, item.base_sha.as_deref(), head)?;
    let base = merge_base(wt, &base, head)?;
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
        smoke: verdict::smoke(ledger, &Evidence::beside(db), item.id, head)?,
        plan: review::plan_findings(ledger, item.id)?,
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

/// `check prune`, and the start of every `check run` and `verify`: removes
/// each directory `targets/lane-<id>` and `checkouts/lane-<id>` beside the
/// ledger whose lane is done, cancelled or not in the ledger, holding that
/// lane's run lock. A done lane may still be run, so one whose lock is held
/// (the caller's own lane included) is skipped. A symlink or file of that
/// name is never touched; a lookup that fails ends the sweep.
/// `{removed: [path], skipped: [{path, why}]}`.
fn sweep(ledger: &Ledger, db: &Path) -> Result<Value, CliError> {
    let abs = std::path::absolute(db).map_err(LedgerError::from)?;
    let home = abs.parent().unwrap_or(Path::new("/"));
    let now = ledger.now()?;
    let (mut removed, mut skipped) = (Vec::new(), Vec::new());
    for dir in ["targets", "checkouts"] {
        let Ok(entries) = std::fs::read_dir(home.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let id = name.to_str().and_then(|n| n.strip_prefix("lane-"));
            let id = id.filter(|d| d.bytes().all(|b| b.is_ascii_digit()));
            let Some(id) = id.and_then(|d| d.parse::<i64>().ok()) else {
                continue;
            };
            let finished = match ledger.work_get(id, &now)? {
                Ok(item) => is_terminal(&item.status),
                Err(Refusal::NotFound { .. }) => true,
                Err(refusal) => return Err(refusal.into()),
            };
            if !finished || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = entry.path().display().to_string();
            let why = match ledger::lock_lane(db, id) {
                Ok(Ok(_lock)) => match std::fs::remove_dir_all(entry.path()) {
                    Ok(()) => {
                        removed.push(path);
                        continue;
                    }
                    Err(e) => e.to_string(),
                },
                Ok(Err(_)) => format!("lane {id} has a check run in progress"),
                Err(e) => e.to_string(),
            };
            skipped.push(json!({"path": path, "why": why}));
        }
    }
    Ok(json!({"removed": removed, "skipped": skipped}))
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
        FindingCmd::Accept(args) => ("accepted", args),
        FindingCmd::Answer(args) => ("answered", args),
        FindingCmd::List { work } => return items(ledger.lane_findings(work)?),
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
                claude_project: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan is a regular file of at most 64 KiB of UTF-8 text that is not
    /// blank; a directory or a FIFO is refused before it is opened, so a
    /// FIFO never blocks the run.
    #[test]
    fn a_plan_file_is_bounded_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let fifo = dir.path().join("fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let said = |path: &Path| match plan_text(path) {
            Ok(text) => text,
            Err(CliError::Refused(Refusal::InvalidInput(why))) => why,
            Err(e) => format!("{e:?}"),
        };
        assert_eq!(said(&file("ok.md", b"1. do it\n")), "1. do it\n");
        for (path, why) in [
            (
                file("big.md", &vec![b'x'; PLAN_BYTES + 1]),
                "over 65536 bytes",
            ),
            (file("bin.md", &[0xff, 0xfe]), "not UTF-8 text"),
            (file("blank.md", b" \n"), "empty"),
            (dir.path().to_path_buf(), "not a regular file"),
            (fifo, "not a regular file"),
            (dir.path().join("gone.md"), "No such file"),
        ] {
            let got = said(&path);
            assert!(got.contains(why), "{why}: {got}");
        }
    }

    /// The plan check refuses a commit with code, naming up to three code
    /// files and the waiver; docs and Markdown are no code.
    #[test]
    fn a_plan_check_refuses_code() {
        let file = |path: &str| ChangedFile {
            path: path.into(),
            area: None,
            added: Some(2),
            removed: Some(1),
            untracked: false,
        };
        let docs = [file("docs/a.md"), file("README.md")];
        assert!(no_code(&docs, &[], "abc1234", 7).is_ok());
        let code = [
            file("a.rs"),
            file("b.rs"),
            file("docs/x.md"),
            file("c.rs"),
            file("d.rs"),
        ];
        let Err(CliError::Refused(Refusal::InvalidInput(why))) = no_code(&code, &[], "abc1234", 7)
        else {
            panic!("code is refused");
        };
        let want = "commit abc1234 changes code against its base (4 code file(s), 12 lines: \
            a.rs, b.rs, c.rs): the plan check runs before any code. The owner can waive it with \
            `check waive plan --work 7`";
        assert_eq!(why, want);
    }
}
