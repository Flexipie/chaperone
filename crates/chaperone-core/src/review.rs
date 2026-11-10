//! `check run review`'s lenses: four fresh headless sessions, three Claude
//! and one Codex, each reviewing the same commit in the harness's checkout
//! from the same facts, at once. A lens that errors, hits its budget or
//! answers outside the schema did not run: it never reads as "no findings".
//!
//! Only the intent lens is told what the change is for, and no lens is
//! told what the lane says about its own change (its summary, merge case
//! or what to look at): the author does not steer the review. Then one more
//! fresh session, the verifier, tries to refute each finding against the
//! code: triage is its, not the author's. [`record`] writes what they did.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::checks::{BODY_CHARS, FindingMove, Recorded, RunInput};
use crate::git::ChangedFile;
use crate::ledger::{Actor, Checked, Evidence, Finding, Findings, Ledger, LedgerError, Payload};
use crate::pipeline::{Check, Cli, Pipeline, Runner};
use crate::runner::{Agent, Ask, CodexAsk, Run, RunError};
use crate::work::WorkItem;

/// The lenses, each the pipeline's check `review_<lens>`.
pub const LENSES: [&str; 4] = ["correctness", "design_tests", "intent", "codex"];
/// The verifier's check.
pub const VERIFIER: &str = "review_verifier";
/// A finding's severity, in the ledger's words (`NON_BLOCKING_SEVERITIES`
/// says which block).
pub const SEVERITIES: [&str; 6] = [
    "blocker",
    "risk",
    "convention",
    "question",
    "nit",
    "improvement",
];
/// Of the diff, what a prompt carries: a change under the size limit fits
/// many times over. Past it the start is kept, cut at a line, and the
/// prompt says so: the lens can read the rest in the checkout.
pub const DIFF_BYTES: usize = 128 * 1024;

/// What the harness gathered about the commit under review.
pub struct Facts {
    /// The merge base with the lane's base, a commit the checkout has.
    pub base: String,
    pub head: String,
    pub files: Vec<ChangedFile>,
    /// `git diff <base> <head>`, whole.
    pub diff: String,
    /// What the change is for, as the ledger holds it: the intent lens's.
    pub goal: String,
}

/// A lens as the pipeline defines it.
#[derive(Debug)]
pub struct Planned<'a> {
    pub lens: &'static str,
    pub check: &'a Check,
    pub runner: &'a Runner,
}

/// Each lens's check and runner, or why the pipeline cannot run a review:
/// a check is missing, or a Claude runner names no model (`claude -p` is
/// always given one).
pub fn lenses(pipeline: &Pipeline) -> Result<Vec<Planned<'_>>, String> {
    LENSES
        .iter()
        .map(|&lens| planned(pipeline, lens, &format!("review_{lens}")))
        .collect()
}

/// The verifier's check and runner, as [`lenses`] reads a lens's.
pub fn verifier(pipeline: &Pipeline) -> Result<Planned<'_>, String> {
    planned(pipeline, "verifier", VERIFIER)
}

/// The sessions a pipeline runs for a review, and its hash: a changed
/// pipeline (a prompt, a model) is another run.
pub struct Plan<'a> {
    pub lenses: Vec<Planned<'a>>,
    pub verifier: Planned<'a>,
    pub hash: &'a str,
}

/// [`lenses`] and [`verifier`] of `pipeline`, or why it cannot review.
pub fn plan(pipeline: &Pipeline) -> Result<Plan<'_>, String> {
    Ok(Plan {
        lenses: lenses(pipeline)?,
        verifier: verifier(pipeline)?,
        hash: &pipeline.hash,
    })
}

fn planned<'a>(
    pipeline: &'a Pipeline,
    lens: &'static str,
    name: &str,
) -> Result<Planned<'a>, String> {
    let Some(check) = pipeline.checks.get(name) else {
        return Err(format!("pipeline {} has no check {name}", pipeline.id));
    };
    // The loader refuses a check whose runner is not there.
    let runner = &pipeline.runners[&check.runner];
    if runner.cli == Cli::Claude && runner.model.is_none() {
        let runner = &check.runner;
        return Err(format!("check {name}: runner {runner} names no model"));
    }
    Ok(Planned {
        lens,
        check,
        runner,
    })
}

/// The facts every lens is given, and whether the diff was cut.
pub fn change(facts: &Facts) -> (String, bool) {
    let (base, head, diff) = (&facts.base, &facts.head, &facts.diff);
    let mut text = format!(
        "## The change\n\nYou are in a checkout of commit {head}; its merge base is {base}.\n\n\
         Files changed (lines added, removed, path; - for a binary file):\n"
    );
    let count = |n: Option<u64>| n.map_or("-".into(), |n| n.to_string());
    for f in &facts.files {
        text += &format!("{}\t{}\t{}\n", count(f.added), count(f.removed), f.path);
    }
    // A newline is never inside a character, so the cut is not either.
    let end = match diff.len() > DIFF_BYTES {
        true => diff.as_bytes()[..DIFF_BYTES]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1),
        false => diff.len(),
    };
    text += "\n## The diff\n\n";
    if end < diff.len() {
        text += &format!(
            "Only its first {end} of {} bytes are below; read the rest with `git diff {base} {head}`.\n\n",
            diff.len()
        );
    }
    text += &format!("```diff\n{}```\n", &diff[..end]);
    (text, end < diff.len())
}

/// Lens `p`'s prompt: its check's prompt, the goal for the intent lens
/// only, then `change`.
pub fn prompt(p: &Planned<'_>, goal: &str, change: &str) -> String {
    let mut prompt = format!("{}\n\n", p.check.text);
    if p.lens == "intent" {
        prompt += &format!("## What the change is for\n\n{goal}\n\n");
    }
    prompt + change
}

/// The answer every lens gives. Codex reads it strictly: every field
/// required, `line` null when there is none.
pub fn schema() -> Value {
    let finding = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["severity", "title", "body", "path", "line"],
        "properties": {
            "severity": {"type": "string", "enum": SEVERITIES},
            "title": {"type": "string"},
            "body": {"type": "string"},
            "path": {"type": "string"},
            "line": {"type": ["integer", "null"]},
        },
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["findings"],
        "properties": {"findings": {"type": "array", "items": finding}},
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Said {
    findings: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    severity: String,
    title: String,
    body: String,
    path: String,
    line: Option<u64>,
}

/// A lens's answer as findings, or why it is none: anything but the
/// schema's shape and words. Each is keyed `<lens>-<12 hex of its path and
/// title>`, so the same finding has the same key on every run; a repeat
/// in one answer takes `-2`, `-3`.
pub fn findings(lens: &str, output: Value) -> Result<Vec<Finding>, String> {
    let said: Said =
        serde_json::from_value(output).map_err(|e| format!("not a lens answer: {e}"))?;
    let mut keys = BTreeSet::new();
    let mut out = Vec::new();
    for f in said.findings {
        if !SEVERITIES.contains(&f.severity.as_str()) {
            return Err(format!("unknown severity {:?}", f.severity));
        }
        let hash = format!("{:x}", Sha256::digest(format!("{}\0{}", f.path, f.title)));
        let first = format!("{lens}-{}", &hash[..12]);
        let (mut key, mut n) = (first.clone(), 1);
        while !keys.insert(key.clone()) {
            n += 1;
            key = format!("{first}-{n}");
        }
        out.push(Finding {
            finding_key: key,
            severity: f.severity,
            title: f.title,
            body: f.body,
            path: Some(f.path),
            rest: Map::from_iter([("lens".into(), json!(lens)), ("line".into(), json!(f.line))]),
        });
    }
    Ok(out)
}

pub struct Agents<'a> {
    pub claude: &'a Agent,
    pub codex: &'a Agent,
}

/// What one session did: a lens, whose outcome is its findings, or the
/// verifier, whose outcome is its rulings.
#[derive(Debug)]
pub struct Lens<T = Vec<Finding>> {
    pub lens: &'static str,
    pub cli: Cli,
    pub runner: String,
    pub model: Option<String>,
    /// The pipeline's prompt file's.
    pub prompt_sha256: String,
    /// The prompt the session was given, then the CLI's whole answer.
    pub evidence_ref: String,
    /// The answer, or why the session did not run.
    pub outcome: Result<T, String>,
    /// What the session left, which still cost when it failed; none when
    /// it never answered.
    pub run: Option<Run>,
}

pub struct Review {
    pub lenses: Vec<Lens>,
    pub diff_bytes: usize,
    pub diff_cut: bool,
}

impl Review {
    /// The findings of every lens that ran.
    pub fn found(&self) -> impl Iterator<Item = &Finding> {
        self.lenses
            .iter()
            .filter_map(|l| l.outcome.as_ref().ok())
            .flatten()
    }
}

/// Runs every lens at once in `dir`, the checkout of `facts.head`, each
/// under its runner's model, tools, budget and timeout, and stores each
/// one's prompt and answer in `store`. Fails only when the store does: a
/// lens's own failure is its outcome.
pub fn run(
    planned: &[Planned<'_>],
    agents: &Agents<'_>,
    dir: &Path,
    facts: &Facts,
    store: &Evidence,
) -> io::Result<Review> {
    let (change, diff_cut) = change(facts);
    let schema = schema();
    let lenses = std::thread::scope(|s| {
        let threads: Vec<_> = planned
            .iter()
            .map(|p| {
                let prompt = prompt(p, &facts.goal, &change);
                let schema = &schema;
                let parse = |out| findings(p.lens, out);
                s.spawn(move || one(p, agents, dir, prompt, schema, store, parse))
            })
            .collect();
        threads
            .into_iter()
            .map(|t| {
                t.join()
                    .unwrap_or_else(|_| Err(io::Error::other("a lens panicked")))
            })
            .collect::<io::Result<_>>()
    })?;
    Ok(Review {
        lenses,
        diff_bytes: facts.diff.len(),
        diff_cut,
    })
}

fn one<T>(
    p: &Planned<'_>,
    agents: &Agents<'_>,
    dir: &Path,
    prompt: String,
    schema: &Value,
    store: &Evidence,
    parse: impl FnOnce(Value) -> Result<T, String>,
) -> io::Result<Lens<T>> {
    let r = p.runner;
    let deadline = Instant::now() + Duration::from_secs(r.timeout_secs as u64);
    let model = r.model.as_deref();
    let answer = match r.cli {
        Cli::Claude => {
            let tools: Vec<&str> = r.tools.iter().map(String::as_str).collect();
            let ask = Ask {
                prompt: &prompt,
                schema,
                // `lenses` refused a Claude runner with none.
                model: model.unwrap_or_default(),
                tools: &tools,
                dir,
                budget_cents: u32::try_from(r.budget_cents).unwrap_or(u32::MAX),
            };
            agents.claude.claude(&ask, deadline)
        }
        Cli::Codex => {
            let ask = CodexAsk {
                prompt: &prompt,
                schema,
                model,
                dir,
            };
            agents.codex.codex(&ask, deadline)
        }
    };
    let (outcome, run) = match answer {
        Ok(a) => (parse(a.output), Some(a.run)),
        Err(e) => {
            let why = Err(e.to_string());
            match e {
                RunError::Failed { run, .. } => (why, Some(*run)),
                _ => (why, None),
            }
        }
    };
    let raw = run.as_ref().map_or("", |r| r.raw.as_str());
    let stored = store.put(&format!("{prompt}\n--- answer ---\n{raw}"))?;
    Ok(Lens {
        lens: p.lens,
        cli: r.cli,
        runner: p.check.runner.clone(),
        model: r.model.clone(),
        prompt_sha256: p.check.prompt_sha256.clone(),
        evidence_ref: stored.hash,
        outcome,
        run,
    })
}

impl<T> Lens<Vec<T>> {
    /// The session's row in the review's `lenses` table (for the verifier,
    /// `findings` counts its rulings).
    pub fn row(&self) -> Value {
        let mut row = json!({
            "lens": self.lens,
            "ran": self.outcome.is_ok(),
            "findings": self.outcome.as_ref().map_or(0, Vec::len),
            "runner": self.runner,
            "model": self.model,
            "prompt_sha256": self.prompt_sha256,
            "evidence_ref": self.evidence_ref,
        });
        if let Err(e) = &self.outcome {
            row["error"] = json!(e);
        }
        if let Some(cost) = self.run.as_ref().and_then(|r| r.cost_usd) {
            row["cost_usd"] = json!(cost);
        }
        row
    }
}

/// The verifier's word on one finding, by its key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ruling {
    pub key: String,
    /// `verified` or `refuted`: the finding's status it moves to.
    pub verdict: String,
    pub why: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rulings {
    rulings: Vec<Ruling>,
}

/// One fresh session that tries to refute every finding of the review
/// against the code in `dir`, from the same facts as the lenses but not
/// the goal. One session for all: they share the facts and the checkout,
/// and the runner's budget and timeout bound it like a lens; one that
/// stops rules on nothing. A finding it does not rule on stays open.
pub fn verify(
    p: &Planned<'_>,
    agents: &Agents<'_>,
    dir: &Path,
    facts: &Facts,
    found: &[Finding],
    store: &Evidence,
) -> io::Result<Lens<Vec<Ruling>>> {
    let mut prompt = prompt(p, "", &change(facts).0) + "\n## The findings\n\n";
    // One JSON object a line: the finding as the ledger holds it, its key
    // the one to rule by.
    for f in found {
        let mut line = f.rest.clone();
        line.extend([
            ("key".into(), json!(f.finding_key)),
            ("severity".into(), json!(f.severity)),
            ("title".into(), json!(f.title)),
            ("body".into(), json!(f.body)),
            ("path".into(), json!(f.path)),
        ]);
        prompt += &format!("{}\n", Value::Object(line));
    }
    let keys: BTreeSet<_> = found.iter().map(|f| f.finding_key.as_str()).collect();
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["rulings"],
        "properties": {"rulings": {"type": "array", "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["key", "verdict", "why"],
            "properties": {
                "key": {"type": "string", "enum": keys},
                "verdict": {"type": "string", "enum": ["verified", "refuted"]},
                "why": {"type": "string"},
            },
        }}},
    });
    one(p, agents, dir, prompt, &schema, store, |out| {
        let said: Rulings =
            serde_json::from_value(out).map_err(|e| format!("not a verifier answer: {e}"))?;
        let mut ruled = BTreeSet::new();
        for r in &said.rulings {
            if !keys.contains(r.key.as_str()) || !ruled.insert(&r.key) {
                return Err(format!(
                    "a ruling on {:?}, no finding or one ruled on twice",
                    r.key
                ));
            }
            if !matches!(r.verdict.as_str(), "verified" | "refuted") {
                return Err(format!("unknown verdict {:?}", r.verdict));
            }
        }
        Ok(said.rulings)
    })
}

/// What the lane is for, as the ledger holds it, for the intent lens: its
/// title, its slice of the spec when it has one, and the goal its newest
/// handoff states. Nothing else the lane wrote (its summary, merge case,
/// what to look at) reaches a lens.
pub fn goal(ledger: &Ledger, item: &WorkItem) -> Result<String, LedgerError> {
    let mut goal = format!("The work item: {}\n", item.title);
    if let (Some(spec), Some(key)) = (item.spec_id, &item.lane_key)
        && let Ok(slice) = ledger.spec_slice(Some(&spec.to_string()), key, None)?
    {
        let (s, v) = (&slice.section, slice.version);
        let title = &s.title;
        goal += &format!(
            "\nIts slice of spec {} v{v}, {title}:\n\n{}\n",
            slice.parent_key, s.body
        );
    }
    let brief = ledger
        .work_handoffs(item.id)?
        .pop()
        .and_then(|h| h.handoff.brief);
    if let Some(brief) = brief.filter(|b| !b.goal.is_empty()) {
        goal += &format!(
            "\nThe goal the lane's newest handoff states:\n\n{}\n",
            brief.goal
        );
    }
    Ok(goal)
}

/// The start of `check run review`, before anything is run: the rulings a
/// crash left unwritten on the lane's newest review, then `input` keyed
/// for a review by `plan`, and the complete review of this commit by this
/// pipeline when one is stored, which a run replays.
pub fn replay(
    ledger: &Ledger,
    actor: &Actor,
    input: &mut RunInput,
    plan: &Plan<'_>,
) -> Checked<Option<Recorded>> {
    let newest: Option<(i64, String)> = ledger
        .conn()
        .query_row(
            "SELECT id, payload_json FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
               AND json_extract(payload_json, '$.check') = 'review' ORDER BY id DESC LIMIT 1",
            [input.work],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, payload)) = newest {
        let payload: Payload = serde_json::from_str(&payload).map_err(io::Error::other)?;
        if let Err(r) = rule(ledger, actor, id, &payload)? {
            return Ok(Err(r));
        }
    }
    input.check = "review".into();
    input.profile = Some(plan.hash.into());
    Ok(Ok(ledger.run_replay(&ledger.run_key(input)?)?))
}

/// `check run review` once the CLI has read the lane, under its run lock:
/// `input` the lane's commit, `dir` a checkout of it, `facts` what it
/// changes. [`replay`], else the sessions and [`finish`], then one
/// `finding.updated` per ruling.
#[allow(clippy::too_many_arguments)]
pub fn record(
    ledger: &Ledger,
    actor: &Actor,
    mut input: RunInput,
    plan: &Plan<'_>,
    agents: &Agents<'_>,
    dir: &Path,
    facts: &Facts,
    store: &Evidence,
) -> Checked<Recorded> {
    match replay(ledger, actor, &mut input, plan)? {
        Ok(Some(stored)) => return Ok(Ok(stored)),
        Ok(None) => {}
        Err(r) => return Ok(Err(r)),
    }
    let review = run(&plan.lenses, agents, dir, facts, store)?;
    let found: Vec<Finding> = review.found().cloned().collect();
    // Nothing to rule on starts no session.
    let verifier = match found.is_empty() {
        true => None,
        false => Some(verify(&plan.verifier, agents, dir, facts, &found, store)?),
    };
    let recorded = match finish(ledger, actor, input, facts, &review, verifier.as_ref())? {
        Ok(recorded) => recorded,
        Err(r) => return Ok(Err(r)),
    };
    let ruled = rule(ledger, actor, recorded.event_id, &recorded.payload)?;
    Ok(ruled.map(|()| recorded))
}

/// Writes what the sessions did: a `usage.reported` for each that left a
/// session id, first, as they cost whether or not the review is recorded;
/// then the review's one `check.finished` under `input`'s key. It is
/// `complete` when every lens ran and the verifier ran or had nothing to
/// rule on. Finding bodies are cut to daisy's 600 characters; the whole
/// answer is evidence.
pub fn finish(
    ledger: &Ledger,
    actor: &Actor,
    mut input: RunInput,
    facts: &Facts,
    review: &Review,
    verifier: Option<&Lens<Vec<Ruling>>>,
) -> Checked<Recorded> {
    let lenses = review.lenses.iter();
    let sessions = lenses.map(|l| (l.lens, l.cli, &l.model, &l.run));
    let sessions = sessions.chain(verifier.map(|v| (v.lens, v.cli, &v.model, &v.run)));
    for (lens, cli, model, run) in sessions {
        let Some(run) = run else { continue };
        let labels = Map::from_iter([
            ("phase".into(), json!("review")),
            ("lens".into(), json!(lens)),
            ("cli".into(), json!(cli)),
            ("model".into(), json!(model)),
        ]);
        if let Err(r) = ledger.usage_run(actor, input.work, labels, run)? {
            return Ok(Err(r));
        }
    }
    let complete = review.lenses.iter().all(|l| l.outcome.is_ok())
        && verifier.is_none_or(|v| v.outcome.is_ok());
    let mut verified = Value::Null;
    if let Some(v) = verifier {
        verified = v.row();
        if let Ok(rulings) = &v.outcome {
            verified["rulings"] = json!(rulings);
        }
    }
    let rows: Vec<_> = review.lenses.iter().map(|l| l.row()).collect();
    input.fields = Map::from_iter([
        ("base".into(), json!(facts.base)),
        ("lenses".into(), json!(rows)),
        ("verifier".into(), verified),
        ("complete".into(), json!(complete)),
        ("diff_bytes".into(), json!(review.diff_bytes)),
        ("diff_cut".into(), json!(review.diff_cut)),
    ]);
    let found = review.found().map(|f| Finding {
        body: f.body.chars().take(BODY_CHARS).collect(),
        ..f.clone()
    });
    input.findings = Some(Findings::List(found.collect()));
    ledger.check_run(actor, &input)
}

/// The verifier's rulings stored on review `id`, each a `finding.updated`
/// with the review's harness producer. A finding no longer open is
/// skipped: ruled already (a rerun after a crash), or moved by its lane or
/// the owner since. Its status is read before the write, so a dispute that
/// lands in between is ruled on, a move the verifier may make.
fn rule(ledger: &Ledger, actor: &Actor, id: i64, payload: &Payload) -> Checked<()> {
    let verifier = payload.rest.get("verifier");
    let rulings = verifier.and_then(|v| v.get("rulings")).cloned();
    let rulings: Vec<Ruling> =
        serde_json::from_value(rulings.unwrap_or(json!([]))).map_err(io::Error::other)?;
    for r in rulings {
        let open: bool = ledger.conn().query_row(
            "SELECT status = 'open' FROM findings WHERE check_event_id = ?1 AND finding_key = ?2",
            params![id, r.key],
            |row| row.get(0),
        )?;
        if !open {
            continue;
        }
        let input = FindingMove {
            check_event_id: id,
            finding_key: r.key,
            status: r.verdict,
            ..FindingMove::default()
        };
        let producer = payload.rest.get("producer");
        if let Err(refused) = ledger.finding_move(actor, &input, producer, false)? {
            return Ok(Err(refused));
        }
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIPELINE: &str = r#"
schema = 2
id = "t"
limits = { budget = 150, timeout = 20 }
runners.lens = { cli = "claude", model = "opus", budget_limit = "budget", timeout_limit = "timeout" }
runners.second = { cli = "codex", budget_limit = "budget", timeout_limit = "timeout" }
checks.review_correctness = { runner = "lens", prompt = "p.md" }
checks.review_design_tests = { runner = "lens", prompt = "p.md" }
checks.review_intent = { runner = "lens", prompt = "intent.md" }
checks.review_codex = { runner = "second", prompt = "p.md" }
gate = [{ id = "a", fn = "eq", args = { field = "verdict.decision", value = "yes" } }]
transition = [{ id = "handoff", routes = ["code_pr"], to = "done", requires = ["a"] }]
"#;

    fn pipeline(src: &str) -> Pipeline {
        Pipeline::load_with_prompts(src, &[("p.md", "P"), ("intent.md", "I")]).unwrap()
    }

    fn facts(diff: String) -> Facts {
        let file = |path: &str, lines: Option<u64>| ChangedFile {
            path: path.into(),
            area: None,
            added: lines,
            removed: lines.map(|n| n - 1),
            untracked: false,
        };
        Facts {
            base: "b".repeat(40),
            head: "c".repeat(40),
            files: vec![file("src/a.rs", Some(3)), file("logo.png", None)],
            diff,
            goal: "GOAL".into(),
        }
    }

    #[test]
    fn a_review_needs_every_lens_and_a_claude_model() {
        let p = pipeline(PIPELINE);
        let planned = lenses(&p).unwrap();
        let named: Vec<_> = planned
            .iter()
            .map(|l| (l.lens, l.check.prompt.as_str(), l.runner.cli))
            .collect();
        let claude = |lens, prompt| (lens, prompt, Cli::Claude);
        assert_eq!(
            named,
            [
                claude("correctness", "p.md"),
                claude("design_tests", "p.md"),
                claude("intent", "intent.md"),
                ("codex", "p.md", Cli::Codex),
            ]
        );
        let without = pipeline(&PIPELINE.replace("review_intent", "review_goal"));
        let e = lenses(&without).unwrap_err();
        assert_eq!(e, "pipeline t has no check review_intent");
        let no_model = pipeline(&PIPELINE.replace(r#"model = "opus", "#, ""));
        let e = lenses(&no_model).unwrap_err();
        assert_eq!(e, "check review_correctness: runner lens names no model");
    }

    /// Every lens gets the same facts after its own prompt; only the
    /// intent lens gets the goal.
    #[test]
    fn only_the_intent_lens_is_told_the_goal() {
        let facts = facts("+a\n".into());
        let (change, cut) = change(&facts);
        let (b, c) = (&facts.base, &facts.head);
        let want = format!(
            "## The change\n\nYou are in a checkout of commit {c}; its merge base is {b}.\n\n\
             Files changed (lines added, removed, path; - for a binary file):\n\
             3\t2\tsrc/a.rs\n-\t-\tlogo.png\n\n## The diff\n\n```diff\n+a\n```\n"
        );
        assert_eq!((&*change, cut), (&*want, false));
        let p = pipeline(PIPELINE);
        for l in lenses(&p).unwrap() {
            let intent = "I\n\n## What the change is for\n\nGOAL\n\n";
            let own = if l.lens == "intent" { intent } else { "P\n\n" };
            assert_eq!(prompt(&l, &facts.goal, &change), own.to_string() + &change);
        }
    }

    /// Past the cap the diff is cut after a whole line, never inside a
    /// character, and the prompt says where the rest is.
    #[test]
    fn a_long_diff_is_cut_at_a_line() {
        let diff = format!("x\n{}", "+é\n".repeat(40_000));
        let (text, cut) = change(&facts(diff));
        assert!(cut);
        let (b, c) = ("b".repeat(40), "c".repeat(40));
        let said = format!(
            "Only its first 131070 of 160002 bytes are below; read the rest with `git diff {b} {c}`.\n\n```diff\nx\n+é\n"
        );
        assert!(text.contains(&said), "{}", &text[..400]);
        assert!(text.ends_with("+é\n```\n") && text.len() < DIFF_BYTES + 500);
    }

    fn item(severity: &str, title: &str) -> Value {
        json!({"severity": severity, "title": title, "body": "b", "path": "src/a.rs", "line": 3})
    }

    /// Keys are the lens's and stable; a repeat gets its own.
    #[test]
    fn findings_are_keyed_by_lens_path_and_title() {
        let said = json!({"findings": [item("risk", "t"), item("nit", "t"), item("nit", "u")]});
        let found = findings("intent", said).unwrap();
        let keys: Vec<_> = found.iter().map(|f| f.finding_key.as_str()).collect();
        let first = format!("{:x}", Sha256::digest("src/a.rs\0t"));
        let first = format!("intent-{}", &first[..12]);
        assert_eq!(keys[..2], [first.clone(), format!("{first}-2")]);
        assert!(keys[2].starts_with("intent-") && keys[2] != first);
        assert_eq!(
            (&found[0].rest["line"], &found[0].rest["lens"]),
            (&json!(3), &json!("intent"))
        );
        assert_eq!(found[2].path.as_deref(), Some("src/a.rs"));
    }

    /// Nothing outside the schema is coerced into an answer.
    #[test]
    fn an_answer_outside_the_schema_is_none() {
        let refused = |said: Value| findings("correctness", said).unwrap_err();
        let said = json!({"findings": [item("critical", "t")]});
        assert_eq!(refused(said), r#"unknown severity "critical""#);
        let mut extra = item("risk", "t");
        extra["confidence"] = json!(0.9);
        assert!(refused(json!({"findings": [extra]})).contains("unknown field"));
        assert!(refused(json!({"findings": "none"})).starts_with("not a lens answer"));
        assert!(refused(json!("Looks good to me.")).starts_with("not a lens answer"));
    }

    /// The intent lens's goal is the title, the lane's slice of the spec
    /// and its newest handoff's goal; never what the lane says about its
    /// change.
    #[test]
    fn the_goal_is_what_the_ledger_says_the_lane_is_for() {
        use crate::work::{Brief, ClaimInput, CreateInput, HandoffInput, LookAt};
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let me = Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        };
        let body = "# P\n\n## slice 1: the slice  (O/R#2)\n\ndone when: SPEC\n";
        let spec = ledger.spec_create(&me, "o/r#1", None, body, None, false);
        spec.unwrap().unwrap();
        let input = CreateInput {
            title: "TITLE".into(),
            source: Some("manual".into()),
            kind: "code".into(),
            pr_flow: "pr".into(),
            spec: Some("o/r#1".into()),
            lane_key: Some("O/R#2".into()),
            ..CreateInput::default()
        };
        let item = ledger
            .work_create(&me, &input, None, false)
            .unwrap()
            .unwrap();
        let claim = ClaimInput::default();
        ledger
            .work_claim(&me, item.id, &claim, None, false)
            .unwrap()
            .unwrap();
        for said in ["OLD", "NEW"] {
            let look = LookAt {
                path: "LOOK".into(),
                ..LookAt::default()
            };
            let brief = Brief {
                goal: said.into(),
                merge_case: "MERGE".into(),
                look_at: vec![look],
                ..Brief::default()
            };
            let handoff = HandoffInput {
                summary: "SUMMARY".into(),
                brief: Some(brief),
                ..HandoffInput::default()
            };
            let done = ledger.work_handoff(&me, item.id, &handoff, None, None, false);
            done.unwrap().unwrap();
        }
        let got = goal(&ledger, &item).unwrap();
        for said in ["TITLE", "the slice", "SPEC", "NEW"] {
            assert!(got.contains(said), "{said}: {got}");
        }
        for unsaid in ["OLD", "SUMMARY", "MERGE", "LOOK"] {
            assert!(!got.contains(unsaid), "{unsaid}: {got}");
        }
    }
}
