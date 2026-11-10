//! `check run review`'s lenses: four fresh headless sessions, three Claude
//! and one Codex, each reviewing the same commit in the harness's checkout
//! from the same facts, at once. A lens that errors, hits its budget or
//! answers outside the schema did not run: it never reads as "no findings".
//!
//! Only the intent lens is told what the change is for, and no lens is
//! told what the lane says about its own change (its summary, merge case
//! or what to look at): the author does not steer the review. Then one more
//! fresh session, the verifier, tries to refute each finding against the
//! code: triage is its, not the author's.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::git::ChangedFile;
use crate::ledger::{Evidence, Finding};
use crate::pipeline::{Check, Cli, Pipeline, Runner};
use crate::runner::{Agent, Ask, CodexAsk, Run, RunError};

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
}
