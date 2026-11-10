//! `review::run`'s lenses and `review::verify` against the scripted
//! agent, linked as both `claude` and `codex`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::git::ChangedFile;
use chaperone_core::ledger::Evidence;
use chaperone_core::pipeline::Pipeline;
use chaperone_core::review::{self, Agents, DIFF_BYTES, Facts, Lens, Review, Ruling};
use chaperone_core::runner::Agent;
use serde_json::{Value, json};

const PIPELINE: &str = r#"
schema = 2
id = "review-test"
limits = { budget = 150, timeout = 20 }
runners.lens = { cli = "claude", model = "opus", tools = ["Read", "Grep"], budget_limit = "budget", timeout_limit = "timeout" }
runners.second = { cli = "codex", model = "gpt-5.5", budget_limit = "budget", timeout_limit = "timeout" }
checks.review_correctness = { runner = "lens", prompt = "correctness.md" }
checks.review_design_tests = { runner = "lens", prompt = "design_tests.md" }
checks.review_intent = { runner = "lens", prompt = "intent.md" }
checks.review_codex = { runner = "second", prompt = "codex.md" }
checks.review_verifier = { runner = "lens", prompt = "verifier.md" }
gate = [{ id = "a", fn = "eq", args = { field = "verdict.decision", value = "yes" } }]
transition = [{ id = "handoff", routes = ["code_pr"], to = "done", requires = ["a"] }]
"#;
const PROMPTS: &[(&str, &str)] = &[
    ("correctness.md", "LENS correctness"),
    ("design_tests.md", "LENS design_tests"),
    ("intent.md", "LENS intent"),
    ("codex.md", "LENS codex"),
    ("verifier.md", "LENS verifier"),
];

/// A result as Claude Code prints it, less the fields not read.
fn result(structured: Value) -> String {
    json!({"type": "result", "subtype": "success", "is_error": false,
        "structured_output": structured, "total_cost_usd": 0.25, "session_id": "s"})
    .to_string()
}

fn found(severity: &str, title: &str) -> Value {
    json!({"severity": severity, "title": title, "body": "why", "path": "src/a.rs", "line": null})
}

/// The fake as `claude` and `codex` in a new directory with these
/// scripts, the facts of a diff of `diff_bytes`, and the evidence store
/// beside a ledger there.
struct Env {
    dir: tempfile::TempDir,
    claude: Agent,
    codex: Agent,
    facts: Facts,
    store: Evidence,
}

fn env(claude: Value, codex: Value, diff_bytes: usize) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let agent = |name: &str, runs: Value| {
        let bin = dir.path().join(name);
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_chaperone-fake"), &bin).unwrap();
        let script = json!({"version": "1 (fake)", "runs": runs}).to_string();
        std::fs::write(dir.path().join(format!("{name}.json")), script).unwrap();
        Agent::probe(bin, [], Instant::now() + Duration::from_secs(20)).unwrap()
    };
    let (claude, codex) = (agent("claude", claude), agent("codex", codex));
    std::fs::create_dir(dir.path().join("checkout")).unwrap();
    let file = ChangedFile {
        path: "src/a.rs".into(),
        area: None,
        added: Some(3),
        removed: Some(1),
        untracked: false,
    };
    let facts = Facts {
        base: "1".repeat(40),
        head: "2".repeat(40),
        files: vec![file],
        diff: "+x\n".repeat(diff_bytes / 3),
        goal: "GOAL add a widget".into(),
    };
    let store = Evidence::beside(&dir.path().join("ledger.db"));
    Env {
        dir,
        claude,
        codex,
        facts,
        store,
    }
}

impl Env {
    fn agents(&self) -> Agents<'_> {
        Agents {
            claude: &self.claude,
            codex: &self.codex,
        }
    }

    fn checkout(&self) -> PathBuf {
        self.dir.path().join("checkout")
    }
}

fn pipeline() -> Pipeline {
    Pipeline::load_with_prompts(PIPELINE, PROMPTS).unwrap()
}

/// The lenses run with `claude` and `codex` scripts, and how long they
/// took.
fn run(claude: Value, codex: Value, diff_bytes: usize) -> (Env, Review, Duration) {
    let env = env(claude, codex, diff_bytes);
    let pipeline = pipeline();
    let planned = review::lenses(&pipeline).unwrap();
    let started = Instant::now();
    let (dir, facts) = (env.checkout(), &env.facts);
    let review = review::run(&planned, &env.agents(), &dir, facts, &env.store).unwrap();
    (env, review, started.elapsed())
}

/// Each lens's call, by the lens its prompt names.
fn call(dir: &Path, cli: &str, lens: &str) -> Value {
    let log = std::fs::read_to_string(dir.join(format!("{cli}.log"))).unwrap();
    let starts = |c: &Value| {
        c["prompt"]
            .as_str()
            .unwrap()
            .starts_with(&format!("LENS {lens}"))
    };
    log.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .find(starts)
        .unwrap()
}

/// Every lens ran at once, in the checkout, with its runner's flags; only
/// the intent lens got the goal; each prompt and answer is stored.
#[test]
fn every_lens_reviews_the_same_facts() {
    let ran = |lens: &str, said: Value| json!({"when": format!("LENS {lens}"), "sleep_ms": 500, "stdout": result(said)});
    let claude = json!([
        ran("correctness", json!({"findings": [found("risk", "r")]})),
        ran(
            "design_tests",
            json!({"findings": [found("nit", "n"), found("blocker", "b")]})
        ),
        ran("intent", json!({"findings": [found("question", "q")]})),
    ]);
    let said = json!({"findings": [found("improvement", "i")]}).to_string();
    let codex = json!([{"when": "LENS codex", "sleep_ms": 500, "write": {"-o": said}}]);
    let (env, review, took) = run(claude, codex, 2 * DIFF_BYTES);
    // Four sessions of 500 ms each: one after another would take 2 s.
    assert!(took < Duration::from_millis(1500), "{took:?}");
    let rows: Vec<_> = review.lenses.iter().map(|l| l.row()).collect();
    let counts: Vec<_> = rows
        .iter()
        .map(|r| json!([r["lens"], r["ran"], r["findings"]]))
        .collect();
    let want = [
        json!(["correctness", true, 1]),
        json!(["design_tests", true, 2]),
        json!(["intent", true, 1]),
        json!(["codex", true, 1]),
    ];
    assert_eq!(counts, want);
    let design = review.lenses[1].outcome.as_ref().unwrap();
    assert_eq!((&*design[1].severity, &*design[1].title), ("blocker", "b"));
    assert_eq!(
        (&rows[0]["cost_usd"], &rows[3]["model"]),
        (&json!(0.25), &json!("gpt-5.5"))
    );
    assert!(review.diff_cut && review.diff_bytes > 2 * DIFF_BYTES - 3);

    let checkout = env.checkout().canonicalize().unwrap();
    for (i, cli) in ["claude", "claude", "claude", "codex"]
        .into_iter()
        .enumerate()
    {
        let lens = review.lenses[i].lens;
        let call = call(env.dir.path(), cli, lens);
        let prompt = call["prompt"].as_str().unwrap();
        assert_eq!(Path::new(call["cwd"].as_str().unwrap()), checkout);
        assert_eq!(
            prompt.contains("GOAL add a widget"),
            lens == "intent",
            "{lens}"
        );
        let args = call["args"].to_string();
        let flags = match cli {
            "claude" => r#""--model","opus","--tools","Grep,Read""#,
            _ => r#""-m","gpt-5.5""#,
        };
        assert!(args.contains(flags), "{args}");
        assert_eq!(
            cli == "claude",
            args.contains(r#""--max-budget-usd","1.50""#)
        );
        let stored = env
            .store
            .get(rows[i]["evidence_ref"].as_str().unwrap())
            .unwrap();
        assert!(stored.starts_with(prompt) && stored.contains("\n--- answer ---\n"));
    }
}

/// A lens that exits non-zero, hits its budget, or answers in prose or
/// with a severity the ledger does not know did not run, and says why; a
/// stop still says what it cost; the lens that ran keeps every finding.
#[test]
fn a_lens_with_no_answer_did_not_run() {
    let budget = json!({"type": "result", "subtype": "error_max_budget_usd", "is_error": true,
        "total_cost_usd": 1.52, "errors": ["Reached maximum budget ($1.5)"]});
    let claude = json!([
        {"when": "LENS correctness", "stderr": "boom", "exit": 3},
        {"when": "LENS design_tests", "stdout": budget.to_string(), "exit": 1},
        {"when": "LENS intent", "stdout": result(json!({"findings": [found("critical", "c")]}))},
    ]);
    let said = json!({"findings": [found("risk", "a"), found("risk", "a"), found("nit", "b")]});
    let codex = json!([{"when": "LENS codex", "write": {"-o": said.to_string()}}]);
    let (_dir, review, _) = run(claude.clone(), codex, 10);
    let rows: Vec<_> = review.lenses.iter().map(|l| l.row()).collect();
    let ran: Vec<_> = rows.iter().map(|r| json!([r["ran"], r["error"]])).collect();
    let budget = "the session failed (error_max_budget_usd): Reached maximum budget ($1.5)";
    let want = [
        json!([false, "exit Some(3): boom"]),
        json!([false, budget]),
        json!([false, r#"unknown severity "critical""#]),
        json!([true, null]),
    ];
    assert_eq!(ran, want);
    assert_eq!(
        (&rows[1]["cost_usd"], &rows[0]["findings"]),
        (&json!(1.52), &json!(0))
    );
    assert!(!review.diff_cut);
    let codex = review.lenses[3].outcome.as_ref().unwrap();
    let titles: Vec<_> = codex.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["a", "a", "b"]);

    let prose = json!([{"when": "LENS codex", "write": {"-o": "Looks fine to me."}}]);
    let (_dir, review, _) = run(claude, prose, 10);
    let error = review.lenses[3].outcome.as_ref().unwrap_err();
    assert!(
        error.starts_with("unreadable answer: the last message is not JSON"),
        "{error}"
    );
}

/// The verifier is shown every finding by its key, with the lenses'
/// facts but not the goal, and rules by key; a ruling on a finding it was
/// not shown, or a session that fails, rules on nothing.
#[test]
fn the_verifier_rules_by_key() {
    let said = json!({"findings": [found("risk", "a"), found("nit", "b")]});
    let found = review::findings("correctness", said).unwrap();
    let (a, b) = (&found[0].finding_key, &found[1].finding_key);
    let verify = |run: Value| -> (Env, Lens<Vec<Ruling>>) {
        let env = env(json!([run]), json!([]), 10);
        let pipeline = pipeline();
        let p = review::verifier(&pipeline).unwrap();
        let (dir, facts) = (env.checkout(), &env.facts);
        let v = review::verify(&p, &env.agents(), &dir, facts, &found, &env.store);
        (env, v.unwrap())
    };
    let ruled = json!({"rulings": [
        {"key": a, "verdict": "verified", "why": "it reads past the end"},
        {"key": b, "verdict": "refuted", "why": "the test covers it"},
    ]});
    let answer = |said: Value| json!({"when": "LENS verifier", "stdout": result(said)});
    let (env, v) = verify(answer(ruled.clone()));
    assert_eq!(json!(v.outcome.unwrap()), ruled["rulings"]);
    let call = call(env.dir.path(), "claude", "verifier");
    let prompt = call["prompt"].as_str().unwrap();
    assert!(!prompt.contains("GOAL") && prompt.contains("```diff\n+x\n"));
    let line = format!(r#""key":"{a}","lens":"correctness","line":null,"path":"src/a.rs""#);
    assert!(prompt.contains(&line), "{prompt}");
    // The schema names the keys it may rule on, and no other.
    let schema: Value = serde_json::from_str(call["args"][4].as_str().unwrap()).unwrap();
    let mut keys = [a, b];
    keys.sort();
    assert_eq!(
        schema["properties"]["rulings"]["items"]["properties"]["key"]["enum"],
        json!(keys)
    );

    let stray = json!({"rulings": [{"key": "correctness-0", "verdict": "verified", "why": "w"}]});
    let error = verify(answer(stray)).1.outcome.unwrap_err();
    assert!(
        error.starts_with(r#"a ruling on "correctness-0""#),
        "{error}"
    );
    let failed = verify(json!({"when": "LENS verifier", "exit": 2})).1;
    assert_eq!(failed.row()["ran"], false);
}
