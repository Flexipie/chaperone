//! `review eval`: the pipeline's lenses and verifier on a checked-in set of
//! past changes, each in a fresh read-only checkout, scored against what
//! the set expects. Writes nothing to the ledger, which it never opens; the
//! sessions' prompts and answers go to the evidence store beside it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use chaperone_core::checks::NON_BLOCKING_SEVERITIES;
use chaperone_core::git;
use chaperone_core::ledger::Evidence;
use chaperone_core::pipeline::{self, Cli};
use chaperone_core::review::{self, Agents, Facts};
use chaperone_graph::code_globs;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::checks::{CHECKOUT_TIMEOUT, Checkout, agent, changed, code, diff, refuse};
use crate::output::{CliError, Outcome};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    entry: Vec<Entry>,
}

/// A past change: `base` its merge base, an ancestor of `head`; `goal`
/// what it was for, the intent lens's; `bug` what a review must find, none
/// when the change was clean.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    repo: String,
    base: String,
    head: String,
    goal: String,
    bug: Option<Bug>,
}

/// Found when a standing finding is on one of `paths` and its title or
/// body has one of `words`, in any case; `what` is shown with the outcome.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bug {
    paths: Vec<String>,
    words: Vec<String>,
    what: String,
}

/// What an entry scores; the first four are scored.
const OUTCOMES: [&str; 6] = ["hit", "miss", "false_alarm", "clean", "skipped", "error"];

pub fn run(db: &Path, set: &Path, repos: &Path, only: Option<String>) -> Result<Outcome, CliError> {
    let text = std::fs::read_to_string(set)
        .map_err(|e| refuse(format!("could not read {}: {e}", set.display())))?;
    let mut set: Set = toml::from_str(&text)
        .map_err(|e| refuse(format!("{} is not a review eval set: {e}", set.display())))?;
    // A name is part of a directory the checkout removes first.
    for name in set.entry.iter().map(|e| &e.name) {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(refuse(format!("entry name {name:?} is not a-z0-9-")));
        }
    }
    if let Some(only) = &only {
        set.entry.retain(|e| &e.name == only);
        if set.entry.is_empty() {
            return Err(refuse(format!("the set has no entry {only:?}")));
        }
    }
    // The newest this binary has: the one lanes with a harness review run.
    let pipeline = pipeline::newer().last().unwrap_or(pipeline::current());
    let plan = review::plan(pipeline).map_err(refuse)?;
    let started = |cli| plan.uses(cli).then(|| agent(cli)).transpose();
    let (claude, codex) = (started(Cli::Claude)?, started(Cli::Codex)?);
    let agents = Agents {
        claude: claude.as_ref(),
        codex: codex.as_ref(),
    };
    let store = Evidence::beside(db);
    let home = std::path::absolute(db).map_err(|e| CliError::Internal(e.to_string()))?;
    let checkouts = home.parent().unwrap_or(Path::new("/")).join("checkouts");
    let mut totals = BTreeMap::from(OUTCOMES.map(|o| (o, 0)));
    let mut entries = Vec::new();
    for e in &set.entry {
        let clone = repos.join(e.repo.rsplit('/').next().unwrap_or_default());
        let (outcome, mut row) = match missing(&clone, e) {
            Some(why) => ("skipped", json!({"why": why})),
            None => {
                // Its own, so a run beside it never removes it.
                let dir = checkouts.join(format!("eval-{}-{}", std::process::id(), e.name));
                let deadline = Instant::now() + CHECKOUT_TIMEOUT;
                let checkout = Checkout::fresh(&clone, dir, &e.head, deadline)?;
                review_one(&plan, &agents, (&clone, &checkout.dir), e, &store)?
            }
        };
        *totals.entry(outcome).or_default() += 1;
        row["name"] = json!(e.name);
        row["outcome"] = json!(outcome);
        entries.push(row);
    }
    let scored: i64 = OUTCOMES[..4].iter().map(|o| totals[o]).sum();
    Ok(Outcome::One(json!({
        "pipeline": {"id": pipeline.id, "hash": pipeline.hash},
        "entries": entries,
        "totals": totals,
        "score": format!("{}/{scored}", totals["hit"] + totals["clean"]),
    })))
}

/// Why `e` cannot run on this machine: no clone, or a commit it lacks.
fn missing(clone: &Path, e: &Entry) -> Option<String> {
    if !clone.join(".git").exists() {
        return Some(format!("no clone of {} at {}", e.repo, clone.display()));
    }
    let deadline = Instant::now() + CHECKOUT_TIMEOUT;
    [&e.base, &e.head].into_iter().find_map(|sha| {
        match git::commit(Path::new("git"), clone, sha, deadline) {
            Ok(Some(_)) => None,
            Ok(None) => Some(format!("{} has no commit {sha}", clone.display())),
            Err(why) => Some(why),
        }
    })
}

/// The lenses on `e` in `dir`, the verifier on what they found, and the
/// score.
fn review_one(
    plan: &review::Plan<'_>,
    agents: &Agents<'_>,
    (clone, dir): (&Path, &Path),
    e: &Entry,
    store: &Evidence,
) -> Result<(&'static str, Value), CliError> {
    let (base, files) = changed(dir, Some(&e.base), &e.head)?;
    // As a lane measures it: code and test lines, by the clone's base
    // profile; none with a file git counts no lines of.
    let found = code_globs(Path::new("git"), clone, Instant::now() + CHECKOUT_TIMEOUT)?;
    let (_, lines, _) = code(&files, found.as_ref().map_or(&[][..], |f| &f.code), &[]);
    let counted = files.iter().all(|f| f.added.is_some());
    let facts = Facts {
        diff: diff(dir, &base, &e.head)?,
        base,
        head: e.head.clone(),
        files,
        goal: e.goal.clone(),
        smoke: Value::Null,
        plan: String::new(),
        code_lines: counted.then_some(lines as i64),
    };
    let io = |e: std::io::Error| CliError::Internal(e.to_string());
    let review = review::run(&plan.lenses, agents, dir, &facts, store).map_err(io)?;
    let found: Vec<_> = review.found().cloned().collect();
    let verify = || review::verify(&plan.verifier, agents, dir, &facts, &found, &[], store);
    let verifier = (!found.is_empty()).then(verify).transpose().map_err(io)?;
    let lenses = review
        .lenses
        .iter()
        .map(|l| (&l.lens, l.outcome.as_ref().err()));
    let mut ran = lenses.chain(verifier.iter().map(|v| (&v.lens, v.outcome.as_ref().err())));
    if let Some((lens, Some(why))) = ran.find(|s| s.1.is_some()) {
        let why = format!("{lens} did not run: {why}");
        return Ok(("error", json!({ "why": why })));
    }
    let rulings = verifier
        .iter()
        .filter_map(|v| v.outcome.as_ref().ok())
        .flatten();
    let out: BTreeSet<_> = rulings
        .filter(|r| r.verdict == "refuted" || r.verdict == "unproven")
        .map(|r| &r.key)
        .collect();
    let standing: Vec<_> = found
        .iter()
        .filter(|f| !NON_BLOCKING_SEVERITIES.contains(&f.severity.as_str()))
        .filter(|f| !out.contains(&f.finding_key))
        .collect();
    let hits = |b: &Bug| {
        standing.iter().any(|f| {
            let text = format!("{}\n{}", f.title, f.body).to_lowercase();
            b.paths.iter().any(|p| f.path.as_deref() == Some(p))
                && b.words.iter().any(|w| text.contains(&w.to_lowercase()))
        })
    };
    let outcome = match (&e.bug, standing.is_empty()) {
        (Some(b), _) if hits(b) => "hit",
        (Some(_), _) => "miss",
        (None, true) => "clean",
        (None, false) => "false_alarm",
    };
    let shown = standing.iter().map(|f| {
        json!({"lens": f.rest["lens"], "severity": f.severity, "title": f.title, "path": f.path})
    });
    let bug = e.bug.as_ref().map(|b| &b.what);
    Ok((
        outcome,
        json!({"findings": shown.collect::<Vec<_>>(), "bug": bug}),
    ))
}
