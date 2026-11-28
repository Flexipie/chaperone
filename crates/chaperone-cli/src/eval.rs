//! `review eval`: the pipeline's lenses and verifier on a checked-in set of
//! past changes, each in a fresh read-only checkout, scored against what
//! the set expects. Writes nothing to the ledger, which it never opens; the
//! sessions' prompts and answers go to the evidence store beside it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use chaperone_core::checks::NON_BLOCKING_SEVERITIES;
use chaperone_core::git;
use chaperone_core::ledger::{Evidence, Finding};
use chaperone_core::pipeline::{self, Cli, Pipeline};
use chaperone_core::review::{self, Agents, Facts, Lens};
use chaperone_core::usage::{SPLIT, split};
use chaperone_graph::code_globs;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::checks::{
    CHECKOUT_TIMEOUT, Checkout, agents, changed, code, diff, refuse, required_lenses,
};
use crate::output::{CliError, Outcome};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    entry: Vec<Entry>,
}

/// A past change: `base` its merge base, an ancestor of `head`; `goal`
/// what it was for, every lens's; `bug` what a review must find, none
/// when the change was clean; `known` real leftovers a review may find,
/// which neither count against it nor for it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    repo: String,
    base: String,
    head: String,
    goal: String,
    bug: Option<Bug>,
    #[serde(default)]
    known: Vec<Bug>,
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

impl Bug {
    fn matches(&self, f: &Finding) -> bool {
        let text = format!("{}\n{}", f.title, f.body).to_lowercase();
        self.paths.iter().any(|p| f.path.as_deref() == Some(p))
            && self.words.iter().any(|w| text.contains(&w.to_lowercase()))
    }
}

/// What an entry scores; the first four are scored. `convention`: a clean
/// entry whose only standing findings are the `design_tests` lens's.
const OUTCOMES: [&str; 7] = [
    "hit",
    "miss",
    "false_alarm",
    "clean",
    "convention",
    "skipped",
    "error",
];

pub fn run(
    db: &Path,
    set: &Path,
    repos: &Path,
    only: Option<String>,
    runs: usize,
) -> Result<Outcome, CliError> {
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
    let mut names = BTreeSet::new();
    for e in &set.entry {
        if e.bug.iter().chain(&e.known).any(|b| b.paths.is_empty()) {
            return Err(refuse(format!(
                "entry {:?} has a bug or known with no paths",
                e.name
            )));
        }
        let whats: BTreeSet<_> = e.known.iter().map(|k| &k.what).collect();
        if !names.insert(&e.name) || whats.len() < e.known.len() {
            return Err(refuse(format!(
                "entry {:?} repeats a name or a known's what",
                e.name
            )));
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
    let started = agents(plan.sessions(None))?;
    let agents = Agents::of(&started);
    let store = Evidence::beside(db);
    let home = std::path::absolute(db).map_err(|e| CliError::Internal(e.to_string()))?;
    let checkouts = home.parent().unwrap_or(Path::new("/")).join("checkouts");
    let mut totals = BTreeMap::from(OUTCOMES.map(|o| (o, 0)));
    totals.insert("fallbacks", 0);
    let (mut entries, mut per_entry, mut reviewed) = (Vec::new(), BTreeMap::new(), 0);
    let (mut unused_known, mut tokens) = (Vec::new(), Map::new());
    for e in &set.entry {
        let clone = repos.join(e.repo.rsplit('/').next().unwrap_or_default());
        let why = missing(&clone, e);
        // Its own, so a run beside it never removes it.
        let checkout = match why {
            Some(_) => None,
            None => {
                let dir = checkouts.join(format!("eval-{}-{}", std::process::id(), e.name));
                let deadline = Instant::now() + CHECKOUT_TIMEOUT;
                Some(Checkout::fresh(&clone, dir, &e.head, deadline)?)
            }
        };
        reviewed += i64::from(checkout.is_some());
        let counts: &mut BTreeMap<&str, i64> = per_entry.entry(&e.name).or_default();
        let mut matched = BTreeSet::new();
        for n in 1..=runs {
            let (outcome, mut row) = match &checkout {
                Some(c) => review_one(pipeline, &agents, (&clone, &c.dir), e, &store)?,
                None => ("skipped", json!({"why": why})),
            };
            *totals.entry(outcome).or_default() += 1;
            *counts.entry(outcome).or_default() += 1;
            let fallbacks = row["fallbacks"].as_i64().unwrap_or(0);
            if fallbacks > 0 {
                *totals.entry("fallbacks").or_default() += fallbacks;
                *counts.entry("fallbacks").or_default() += fallbacks;
            }
            if let Some(spent) = row["tokens"].as_object() {
                add(&mut tokens, spent);
            }
            let known = row["known"].as_array().into_iter().flatten();
            matched.extend(known.filter_map(|f| f["known"].as_str().map(String::from)));
            row["name"] = json!(e.name);
            row["outcome"] = json!(outcome);
            row["run"] = json!(n);
            entries.push(row);
        }
        if OUTCOMES[..5].iter().any(|o| counts.contains_key(o)) {
            let unused = e.known.iter().filter(|k| !matched.contains(&k.what));
            unused_known.extend(unused.map(|k| json!({"name": e.name, "what": k.what})));
        }
    }
    let scored: i64 = OUTCOMES[..4].iter().map(|o| totals[o]).sum();
    let good = totals["hit"] + totals["clean"];
    Ok(Outcome::One(json!({
        "pipeline": {"id": pipeline.id, "hash": pipeline.hash},
        "runs": runs,
        "entries": entries,
        "per_entry": per_entry,
        "totals": totals,
        "tokens": tokens,
        "score": format!("{good}/{scored}"),
        "mean": mean(good, scored, reviewed),
        "unused_known": unused_known,
    })))
}

/// The score of one run over the `entries` reviewed (not skipped): the rate
/// of `good` (hit plus clean) in the `scored` rows of every run, so a row
/// out of the score (convention, error) counts on neither side.
fn mean(good: i64, scored: i64, entries: i64) -> f64 {
    if scored == 0 {
        return 0.0;
    }
    let mean = (good * entries) as f64 / scored as f64;
    (mean * 100.0).round() / 100.0
}

/// Adds each count of `part` to `total`'s, skipping a null: a count no
/// session reported stays unknown, never 0.
fn add(total: &mut Map<String, Value>, part: &Map<String, Value>) {
    for (k, n) in part {
        let sum = total.entry(k.clone()).or_insert(Value::Null);
        if let Some(n) = n.as_i64() {
            *sum = json!(sum.as_i64().unwrap_or(0) + n);
        }
    }
}

/// The [`SPLIT`] the sessions of `lenses` used, and how many there were,
/// a fallback's down first engine included.
fn spent<'a, T: 'a>(lenses: impl IntoIterator<Item = &'a Lens<T>>) -> Map<String, Value> {
    let mut out: Map<String, Value> = SPLIT.iter().map(|k| (k.to_string(), Value::Null)).collect();
    out.insert("sessions".into(), json!(0));
    for l in lenses {
        let first = l.fallback.as_ref().map(|f| (f.cli, &f.run));
        for (cli, run) in first.into_iter().chain([(l.cli, &l.run)]) {
            let Some(run) = run else { continue };
            let mut one = split(cli == Cli::Codex, &run.model_usage);
            one.insert("sessions".into(), json!(1));
            add(&mut out, &one);
        }
    }
    out
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
    pipeline: &'static Pipeline,
    agents: &Agents<'_>,
    (clone, dir): (&Path, &Path),
    e: &Entry,
    store: &Evidence,
) -> Result<(&'static str, Value), CliError> {
    let (base, files) = changed(dir, Some(&e.base), &e.head)?;
    // As a lane measures it: code and test lines, by the clone's base
    // profile; none with a file git counts no lines of.
    let found = code_globs(Path::new("git"), clone, Instant::now() + CHECKOUT_TIMEOUT)?;
    let (_, (added, removed), _) = code(&files, found.as_ref().map_or(&[][..], |f| &f.code), &[]);
    let lines = added + removed;
    let counted = files.iter().all(|f| f.added.is_some());
    // The repo's required lenses too, as a lane's review runs them.
    let required = review::required(pipeline, &required_lenses(clone, &files)?).map_err(refuse)?;
    let mut plan = review::plan(pipeline).map_err(refuse)?;
    plan.require(pipeline, &required);
    let facts = Facts {
        diff: diff(dir, &base, &e.head)?,
        base,
        head: e.head.clone(),
        files,
        goal: e.goal.clone(),
        plan_text: None,
        smoke: Value::Null,
        plan: String::new(),
        code_lines: counted.then_some(lines as i64),
        assets: Vec::new(),
        over_cap: None,
        since: None,
    };
    let io = |e: std::io::Error| CliError::Internal(e.to_string());
    let review = review::run(
        &plan.lenses,
        plan.disputes.is_some(),
        agents,
        dir,
        &facts,
        store,
    )
    .map_err(io)?;
    let found: Vec<_> = review.found().cloned().collect();
    let verify = || review::verify(&plan.verifier, agents, dir, &facts, &found, &[], store);
    let verifier = (!found.is_empty()).then(verify).transpose().map_err(io)?;
    let lenses = review
        .lenses
        .iter()
        .map(|l| (&l.lens, l.outcome.as_ref().err()));
    let mut ran = lenses.chain(verifier.iter().map(|v| (&v.lens, v.outcome.as_ref().err())));
    let fallbacks = review
        .lenses
        .iter()
        .filter(|l| l.fallback.is_some())
        .count()
        + verifier.iter().filter(|v| v.fallback.is_some()).count();
    let mut tokens = spent(&review.lenses);
    add(&mut tokens, &spent(&verifier));
    if let Some((lens, Some(why))) = ran.find(|s| s.1.is_some()) {
        let why = format!("{lens} did not run: {why}");
        return Ok((
            "error",
            json!({ "why": why, "fallbacks": fallbacks, "tokens": tokens }),
        ));
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
    let shown = |f: &Finding| json!({"lens": f.rest["lens"], "severity": f.severity, "title": f.title, "path": f.path});
    let (mut findings, mut known) = (Vec::new(), Vec::new());
    for f in &standing {
        match e.known.iter().find(|k| k.matches(f)) {
            Some(k) => {
                let mut row = shown(f);
                row["known"] = json!(k.what);
                known.push(row);
            }
            None => findings.push(*f),
        }
    }
    let outcome = outcome(e.bug.as_ref(), &findings);
    let findings: Vec<_> = findings.into_iter().map(shown).collect();
    let bug = e.bug.as_ref().map(|b| &b.what);
    Ok((
        outcome,
        json!({ "findings": findings, "known": known, "bug": bug, "fallbacks": fallbacks, "tokens": tokens }),
    ))
}

/// An entry's outcome from the standing findings no `known` of it matches.
fn outcome(bug: Option<&Bug>, left: &[&Finding]) -> &'static str {
    match bug {
        Some(b) if left.iter().any(|f| b.matches(f)) => "hit",
        Some(_) => "miss",
        None if left.is_empty() => "clean",
        None if left.iter().all(|f| f.rest["lens"] == "design_tests") => "convention",
        None => "false_alarm",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn finding(lens: &str, path: &str, title: &str) -> Finding {
        let said = json!({"findings": [{"severity": "risk", "title": title, "body": "b",
            "path": path, "line": null}]});
        review::findings(lens, said).unwrap().remove(0)
    }

    /// A `known` drops what it matches; a clean entry left with only
    /// design_tests findings is `convention`, with any other `false_alarm`.
    #[test]
    fn known_and_convention_findings() {
        let toml = r#"
            name = "e"
            repo = "o/r"
            base = "a"
            head = "b"
            goal = "g"
            known = [{ paths = ["a.rs"], words = ["STALE"], what = "w" }]
        "#;
        let e: Entry = toml::from_str(toml).unwrap();
        let stale = finding("correctness", "a.rs", "a stale todo");
        let (other_path, other_word) = (
            finding("correctness", "b.rs", "stale"),
            finding("correctness", "a.rs", "fresh"),
        );
        let tests = finding("design_tests", "a.rs", "no test");
        assert!(e.known[0].matches(&stale));
        assert!(!e.known[0].matches(&other_path) && !e.known[0].matches(&other_word));
        assert_eq!(outcome(None, &[]), "clean");
        assert_eq!(outcome(None, &[&tests]), "convention");
        assert_eq!(outcome(None, &[&tests, &other_word]), "false_alarm");
        let bug = &e.known[0];
        assert_eq!(outcome(Some(bug), &[&tests, &stale]), "hit");
        assert_eq!(outcome(Some(bug), &[&tests]), "miss");
    }

    /// Rows out of the score count on neither side of the mean.
    #[test]
    fn mean_leaves_out_unscored_rows() {
        assert_eq!(mean(26, 45, 15), 8.67);
        assert_eq!(mean(26, 43, 15), 9.07);
        assert_eq!(mean(0, 0, 0), 0.0);
    }
}
