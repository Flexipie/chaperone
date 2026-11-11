//! `review::run`'s lenses, `review::verify`, `review::record`,
//! `verdict::record` and `plan::record` against the scripted agent, linked
//! as both `claude` and `codex`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::checks::{BlockingFinding, CheckInput, FindingMove, Recorded, RunInput};
use chaperone_core::git::ChangedFile;
use chaperone_core::ledger::{Actor, Evidence, Findings, Kept, Ledger, Refusal};
use chaperone_core::pipeline::{Fact, Pipeline};
use chaperone_core::review::{self, Agents, Facts, Lens, PROMPT_DIFF_BYTES, Review, Ruling};
use chaperone_core::runner::Agent;
use chaperone_core::work::{Brief, ClaimInput, CreateInput, HandoffInput, LookAt};
use chaperone_core::{plan, verdict};
use serde_json::{Value, json};

const PIPELINE: &str = r#"
schema = 2
id = "review-test"
limits = { budget = 150, timeout = 20, review_attempts = 2, verdict_attempts = 9, plan_attempts = 2 }
runners.lens = { cli = "claude", model = "opus", tools = ["Read", "Grep"], budget_limit = "budget", timeout_limit = "timeout" }
runners.second = { cli = "codex", model = "gpt-5.5", budget_limit = "budget", timeout_limit = "timeout" }
checks.review_correctness = { runner = "lens", prompt = "correctness.md" }
checks.review_design_tests = { runner = "lens", prompt = "design_tests.md" }
checks.review_intent = { runner = "lens", prompt = "intent.md" }
checks.review_codex = { runner = "second", prompt = "codex.md" }
checks.review_verifier = { runner = "lens", prompt = "verifier.md" }
checks.verdict = { runner = "lens", prompt = "verdict.md" }
checks.plan = { runner = "lens", prompt = "plan.md" }
gate = [{ id = "a", fn = "eq", args = { field = "verdict.decision", value = "yes" } }]
transition = [{ id = "handoff", routes = ["code_pr"], to = "done", requires = ["a"] }]
"#;
const PROMPTS: &[(&str, &str)] = &[
    ("correctness.md", "LENS correctness"),
    ("design_tests.md", "LENS design_tests"),
    ("intent.md", "LENS intent"),
    ("codex.md", "LENS codex"),
    ("verifier.md", "LENS verifier"),
    ("verdict.md", "LENS verdict"),
    ("plan.md", "LENS plan"),
];

/// A result as Claude Code prints it, less the fields not read.
fn result(structured: Value) -> String {
    session("s", structured)
}

fn session(id: &str, structured: Value) -> String {
    json!({"type": "result", "subtype": "success", "is_error": false,
        "structured_output": structured, "total_cost_usd": 0.25, "session_id": id})
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
        smoke: json!({"harness": null, "lane_claim": {"outcome": "ran", "evidence_files": [
            {"path": "out.log", "end": "SMOKE SAW"}]}}),
        plan: String::new(),
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
            claude: Some(&self.claude),
            codex: Some(&self.codex),
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
/// the intent lens got the goal; each prompt and answer is stored, and
/// each row names the agent binary and its version.
#[test]
fn every_lens_reviews_the_same_facts() {
    let ran = |lens: &str, said: Value| json!({"when": format!("LENS {lens}"), "sleep_ms": 800, "stdout": result(said)});
    let claude = json!([
        ran("correctness", json!({"findings": [found("risk", "r")]})),
        ran(
            "design_tests",
            json!({"findings": [found("nit", "n"), found("blocker", "b")]})
        ),
        ran("intent", json!({"findings": [found("question", "q")]})),
    ]);
    let said = json!({"findings": [found("improvement", "i")]}).to_string();
    let codex = json!([{"when": "LENS codex", "sleep_ms": 800, "write": {"-o": said}}]);
    let (env, review, took) = run(claude, codex, 2 * PROMPT_DIFF_BYTES);
    // Four sessions of 800 ms each: one after another would take 3.2 s.
    assert!(took < Duration::from_millis(2400), "{took:?}");
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
    assert!(review.diff_cut && review.diff_bytes > 2 * PROMPT_DIFF_BYTES - 3);

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
        assert_eq!(prompt.contains("SMOKE SAW"), lens == "intent", "{lens}");
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
        let stored = |field: &str| env.store.get(rows[i][field].as_str().unwrap()).unwrap();
        assert_eq!(stored("prompt_ref"), prompt);
        assert_eq!(
            stored("answer_ref"),
            review.lenses[i].run.as_ref().unwrap().raw
        );
        assert_eq!(rows[i]["evidence_truncated"], false);
        let bin = rows[i]["bin"].as_str().unwrap();
        assert!(bin.ends_with(&format!("/{cli}")), "{bin}");
        assert_eq!(rows[i]["version"], "1 (fake)");
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
        let v = review::verify(&p, &env.agents(), &dir, facts, &found, &[], &env.store);
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
    // The findings are fenced data, and the harness's line comes last.
    assert!(prompt.contains("```jsonl\n{") && prompt.ends_with("in the schema you were given.\n"));
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
    let blank = json!({"rulings": [{"key": a, "verdict": "verified", "why": " "}]});
    let error = verify(answer(blank)).1.outcome.unwrap_err();
    assert!(
        error.starts_with("not a verifier answer: no why"),
        "{error}"
    );
    let failed = verify(json!({"when": "LENS verifier", "exit": 2})).1;
    assert_eq!(failed.row()["ran"], false);
}

/// The session that claimed lane 1.
fn lane_session() -> Actor {
    Actor {
        agent: Some("a".into()),
        session: Some("s1".into()),
    }
}

/// The ledger beside `env`'s evidence, with lane 1 claimed by
/// [`lane_session`].
fn ledger(env: &Env) -> Ledger {
    let ledger = Ledger::open_and_migrate(&env.dir.path().join("ledger.db")).unwrap();
    let input = CreateInput {
        title: "t".into(),
        source: Some("manual".into()),
        kind: "code".into(),
        pr_flow: "pr".into(),
        ..CreateInput::default()
    };
    let me = lane_session();
    ledger
        .work_create(&me, &input, None, false)
        .unwrap()
        .unwrap();
    let claim = ClaimInput::default();
    ledger
        .work_claim(&me, 1, &claim, None, false)
        .unwrap()
        .unwrap();
    ledger
}

/// The review of lane 1's commit, by `pipeline`.
fn input(pipeline: &Pipeline) -> RunInput {
    RunInput {
        work: 1,
        check: "review".into(),
        head: "2".repeat(40),
        tree: "3".repeat(40),
        profile: Some(pipeline.hash.clone()),
        bin: "chaperone".into(),
        version: "0".into(),
        ..RunInput::default()
    }
}

/// `check run review` as the CLI runs it, started by the lane's own
/// session: [`review::replay`], then [`review::record`].
fn record(env: &Env, ledger: &Ledger) -> Result<Recorded, Refusal> {
    reviewed(env, ledger, &env.facts.head, &"3".repeat(40))
}

/// [`record`] on lane 1's commit `head` of `tree`.
fn reviewed(env: &Env, ledger: &Ledger, head: &str, tree: &str) -> Result<Recorded, Refusal> {
    let pipeline = pipeline();
    let plan = review::plan(&pipeline).unwrap();
    let (dir, agents, me) = (env.checkout(), env.agents(), lane_session());
    let mut input = RunInput {
        head: head.into(),
        tree: tree.into(),
        ..input(&pipeline)
    };
    if let Some(stored) = review::replay(ledger, &me, &mut input, &plan).unwrap()? {
        return Ok(stored);
    }
    let got = review::record(
        ledger, &me, input, &plan, &agents, &dir, &env.facts, &env.store,
    );
    got.unwrap()
}

/// Sessions started so far, by both CLIs.
fn calls(env: &Env) -> usize {
    let log = |cli: &str| std::fs::read_to_string(env.dir.path().join(format!("{cli}.log")));
    ["claude", "codex"]
        .map(|c| log(c).unwrap_or_default().lines().count())
        .iter()
        .sum()
}

/// The lane's findings that still block, as (title, status).
fn blocking(ledger: &Ledger) -> Vec<(String, String)> {
    let found = ledger.blocking_findings(1).unwrap();
    found.into_iter().map(|f| (f.title, f.status)).collect()
}

/// Scripts where correctness finds a, b and c, and the verifier (whose
/// answer is `verifier`) is shown them; the other lenses find nothing.
fn scripts(verifier: impl Fn(&str, &str) -> Value) -> (Value, Value) {
    let said = json!({"findings": [found("risk", "a"), found("blocker", "b"), found("risk", "c")]});
    let keys = review::findings("correctness", said.clone()).unwrap();
    let (a, b) = (&keys[0].finding_key, &keys[1].finding_key);
    let none = json!({"findings": []});
    let ran = |lens: &str, said: &Value| json!({"when": format!("LENS {lens}"), "stdout": session(lens, said.clone())});
    let claude = json!([
        ran("correctness", &said),
        ran("design_tests", &none),
        ran("intent", &none),
        verifier(a, b)
    ]);
    let codex = json!([{"when": "LENS codex", "write": {"-o": none.to_string()}}]);
    (claude, codex)
}

fn rules(a: &str, b: &str) -> Value {
    let rulings = json!({"rulings": [
        {"key": a, "verdict": "verified", "why": "it reads past the end"},
        {"key": b, "verdict": "refuted", "why": "the test covers it"},
    ]});
    json!({"when": "LENS verifier", "stdout": session("verifier", rulings)})
}

/// One review event with every lens's row and the findings; the verifier,
/// run from the lane's own session, verifies a and refutes b, and c it
/// does not rule on stays open; each session's cost is a usage row. The
/// same commit again replays and starts no session.
#[test]
fn a_review_is_recorded_then_ruled() {
    let (claude, codex) = scripts(rules);
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let first = record(&env, &ledger).unwrap();
    let p = &first.payload;
    assert_eq!(
        (p.check.as_deref(), &p.rest["complete"]),
        (Some("review"), &json!(true))
    );
    let ran: Vec<_> = p.rest["lenses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| &l["ran"])
        .collect();
    assert_eq!(ran, [true; 4]);
    assert_eq!(p.rest["verifier"]["rulings"].as_array().unwrap().len(), 2);
    assert!(matches!(&p.findings, Some(Findings::List(l)) if l.len() == 3));
    let want = [("a", "verified"), ("c", "open")].map(|(t, s)| (t.to_string(), s.to_string()));
    assert_eq!(blocking(&ledger), want);
    let usage = ledger.usage_get(1).unwrap().unwrap();
    // Three Claude lenses and the verifier; Codex left no session id.
    assert_eq!(usage["totals"]["harness"]["messages"], 4);

    let before = calls(&env);
    let again = record(&env, &ledger).unwrap();
    assert_eq!((again.replayed, again.event_id), (true, first.event_id));
    assert_eq!(calls(&env), before);
}

/// A verifier that fails rules on nothing: every finding stays open and
/// the review is not complete, so the same commit runs again, up to the
/// pipeline's `review_attempts`; past it the run is refused and starts
/// nothing.
#[test]
fn a_verifier_that_fails_leaves_findings_open() {
    let (claude, codex) = scripts(|_, _| json!({"when": "LENS verifier", "exit": 2}));
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let first = record(&env, &ledger).unwrap();
    assert_eq!(first.payload.rest["complete"], false);
    assert_eq!(first.payload.rest["verifier"]["ran"], false);
    let open = ["a", "b", "c"].map(|t| (t.to_string(), "open".to_string()));
    assert_eq!(blocking(&ledger)[..3], open);
    let again = record(&env, &ledger).unwrap();
    assert!(!again.replayed && again.event_id > first.event_id);
    let before = calls(&env);
    let Refusal::InvalidInput(said) = record(&env, &ledger).unwrap_err() else {
        panic!("not a refusal of the input");
    };
    let head = "2".repeat(40);
    assert_eq!(
        said,
        format!(
            "the tree {} of commit {head} has had 2 reviews by this pipeline, its limit \
             review_attempts; the owner can waive the findings, or the lane can push a commit \
             that changes the tree",
            "3".repeat(40)
        )
    );
    assert_eq!(calls(&env), before);
}

/// One lens that stops on its budget makes the review not complete, and
/// the other lenses' findings are all in the event, ruled on; the stopped
/// session's cost is still a usage row.
#[test]
fn one_lens_failing_keeps_the_others_findings() {
    let (mut claude, codex) = scripts(rules);
    let budget = json!({"type": "result", "subtype": "error_max_budget_usd", "is_error": true,
        "total_cost_usd": 1.52, "session_id": "d", "errors": ["Reached maximum budget ($1.5)"]});
    claude[1] = json!({"when": "LENS design_tests", "stdout": budget.to_string(), "exit": 1});
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let p = record(&env, &ledger).unwrap().payload;
    assert_eq!(
        (&p.rest["complete"], &p.rest["lenses"][1]["ran"]),
        (&json!(false), &json!(false))
    );
    assert!(matches!(&p.findings, Some(Findings::List(l)) if l.len() == 3));
    let want = [("a", "verified"), ("c", "open")].map(|(t, s)| (t.to_string(), s.to_string()));
    assert_eq!(blocking(&ledger), want);
    let usage = ledger.usage_get(1).unwrap().unwrap();
    let stopped = usage["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == "d");
    assert_eq!(stopped.unwrap()["cost_usd"], 1.52);
}

/// A review where no lens found anything starts no verifier session and
/// is complete.
#[test]
fn no_findings_start_no_verifier() {
    let (mut claude, codex) = scripts(rules);
    let none = json!({"findings": []});
    claude[0] = json!({"when": "LENS correctness", "stdout": session("correctness", none)});
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let p = record(&env, &ledger).unwrap().payload;
    assert_eq!(
        (&p.rest["complete"], &p.rest["verifier"]),
        (&json!(true), &Value::Null)
    );
    // Three Claude lenses and Codex: no fifth session.
    assert_eq!(calls(&env), 4);
}

/// A crash after the review event and before its rulings: the next run
/// writes them from the event, and starts no session.
#[test]
fn a_crash_before_the_rulings_is_finished_by_a_rerun() {
    let (claude, codex) = scripts(rules);
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let pipeline = pipeline();
    let plan = review::plan(&pipeline).unwrap();
    let (dir, agents, me) = (env.checkout(), env.agents(), lane_session());
    let run = review::run(&plan.lenses, &agents, &dir, &env.facts, &env.store).unwrap();
    let found: Vec<_> = run.found().cloned().collect();
    let verified = review::verify(
        &plan.verifier,
        &agents,
        &dir,
        &env.facts,
        &found,
        &[],
        &env.store,
    );
    let verified = verified.unwrap();
    let input = input(&pipeline);
    let written = review::finish(&ledger, &me, input, &env.facts, &run, Some(&verified));
    let written = written.unwrap().unwrap();
    assert_eq!(blocking(&ledger).len(), 3);

    let before = calls(&env);
    let again = record(&env, &ledger).unwrap();
    assert_eq!((again.replayed, again.event_id), (true, written.event_id));
    let want = [("a", "verified"), ("c", "open")].map(|(t, s)| (t.to_string(), s.to_string()));
    assert_eq!(blocking(&ledger), want);
    assert_eq!(calls(&env), before);
}

/// The next tree's verifier is shown the earlier review's findings that
/// still block, by that review, and its `fixed` ruling settles one; the
/// one it leaves still blocks.
#[test]
fn the_verifier_rules_an_earlier_finding_fixed() {
    let (claude, codex) = scripts(rules);
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let first = record(&env, &ledger).unwrap().event_id;
    let said = json!({"findings": [found("risk", "a"), found("blocker", "b"), found("risk", "c")]});
    let keys = review::findings("correctness", said).unwrap();
    let earlier = |i: usize| format!("{first}/{}", keys[i].finding_key);
    let none = json!({"findings": []});
    let lens = |l: &str| json!({"when": format!("LENS {l}"), "stdout": session(l, none.clone())});
    let fixed = json!({"rulings": [
        {"key": earlier(0), "verdict": "fixed", "why": "src/a.rs:3 now stops at the end"}]});
    let verifier = json!({"when": "LENS verifier", "stdout": session("verifier", fixed)});
    let runs = [
        lens("correctness"),
        lens("design_tests"),
        lens("intent"),
        verifier,
    ];
    rescript(&env, "claude", json!(runs));
    let codex = json!([{"when": "LENS codex", "write": {"-o": none.to_string()}}]);
    rescript(&env, "codex", codex);
    let head = "4".repeat(40);
    reviewed(&env, &ledger, &head, &"5".repeat(40)).unwrap();
    let log = std::fs::read_to_string(env.dir.path().join("claude.log")).unwrap();
    let call: Value = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .rfind(|c: &Value| c["prompt"].as_str().unwrap().starts_with("LENS verifier"))
        .unwrap();
    let schema: Value = serde_json::from_str(call["args"][4].as_str().unwrap()).unwrap();
    let mut shown = [earlier(0), earlier(2)];
    shown.sort();
    let rulings = &schema["properties"]["rulings"]["items"]["properties"];
    assert_eq!(rulings["key"]["enum"], json!(shown));
    assert_eq!(blocking(&ledger), [("c".to_string(), "open".to_string())]);
}

/// The fake `cli`'s script, replaced.
fn rescript(env: &Env, cli: &str, runs: Value) {
    let script = json!({"version": "1 (fake)", "runs": runs}).to_string();
    std::fs::write(env.dir.path().join(format!("{cli}.json")), script).unwrap();
}

fn says(decision: &str) -> Value {
    let said = json!({"decision": decision, "why": "because", "proven": ["p"], "not_proven": []});
    json!([{"when": "LENS verdict", "stdout": session("v", said)}])
}

/// The harness's `checks` on lane 1's commit `head` of `tree`, as `check
/// run` records them from the lane's own session: a diff, passing tests,
/// and a complete review with one blocker.
fn measured(ledger: &Ledger, head: &str, tree: &str, checks: &[&str]) {
    for &check in checks {
        let mut input = RunInput {
            check: check.into(),
            head: head.into(),
            tree: tree.into(),
            ..input(&pipeline())
        };
        input.fields = match check {
            "diff" => json!({"files": 1, "added": 3, "removed": 1, "code_lines": 4}),
            "tests" => json!({"result": "pass", "commands": [{"argv": ["t"], "exit": 0, "ms": 9}]}),
            _ => json!({"complete": true}),
        }
        .as_object()
        .unwrap()
        .clone();
        if check == "review" {
            let said = json!({"findings": [found("blocker", "BLOCKER")]});
            let list = review::findings("correctness", said).unwrap();
            input.findings = Some(Findings::List(list));
        }
        ledger.check_run(&lane_session(), &input).unwrap().unwrap();
    }
}

/// `check run verdict` on lane 1's `head` of `tree`, as the CLI runs it
/// from the lane's own session: the harness's facts, [`verdict::replay`],
/// then [`verdict::record`].
fn judge(env: &Env, ledger: &Ledger, head: &str, tree: &str) -> Recorded {
    judge_with(env, ledger, head, tree, false)
}

/// [`judge`] with whether the base profile declares a smoke.
fn judge_with(env: &Env, ledger: &Ledger, head: &str, tree: &str, smoke: bool) -> Recorded {
    let pipeline = pipeline();
    let (p, attempts) = verdict::plan(&pipeline).unwrap();
    let me = lane_session();
    let harness = verdict::harness(ledger, &env.store, 1, head, smoke);
    let harness = harness.unwrap().unwrap();
    let mut input = RunInput {
        head: head.into(),
        tree: tree.into(),
        ..input(&pipeline)
    };
    let key = (&*pipeline.hash, attempts);
    let stored = verdict::replay(ledger, &me, &mut input, key, &harness);
    if let Some(r) = stored.unwrap().unwrap() {
        return r;
    }
    let item = ledger.work_get(1, &ledger.now().unwrap()).unwrap().unwrap();
    let file = ChangedFile {
        path: "src/a.rs".into(),
        area: None,
        added: Some(1),
        removed: Some(0),
        untracked: false,
    };
    let facts = Facts {
        base: "1".repeat(40),
        head: head.into(),
        files: vec![file],
        diff: "+x\n".into(),
        goal: review::goal(ledger, &item).unwrap(),
        smoke: Value::Null,
        plan: String::new(),
    };
    let (dir, agents) = (env.checkout(), env.agents());
    let got = verdict::record(
        ledger, &me, input, &p, &agents, &dir, &facts, &harness, &env.store,
    );
    got.unwrap().unwrap()
}

/// The verdict is told the goal, the change and the harness's own results
/// (tests, review, the finding that blocks), never the lane's summary,
/// merge case or what to look at; it is recorded from the lane's own
/// session as the harness's `merge_verdict`, naming the events it read,
/// the agent and both evidence files, and its cost is a usage row.
#[test]
fn the_verdict_judges_from_the_harness_facts() {
    let env = env(says("yes"), json!([]), 10);
    let ledger = ledger(&env);
    let brief = Brief {
        goal: "GOAL ship it".into(),
        merge_case: "MERGE".into(),
        look_at: vec![LookAt {
            path: "LOOK".into(),
            ..LookAt::default()
        }],
        ..Brief::default()
    };
    let handoff = HandoffInput {
        summary: "SUMMARY".into(),
        brief: Some(brief),
        ..HandoffInput::default()
    };
    let me = lane_session();
    let done = ledger.work_handoff(&me, 1, &handoff, None, None, false);
    done.unwrap().unwrap();
    let (head, tree) = ("2".repeat(40), "3".repeat(40));
    measured(&ledger, &head, &tree, &["diff", "tests", "review"]);
    let p = judge(&env, &ledger, &head, &tree).payload;
    let got = (
        p.check.as_deref(),
        p.outcome.as_deref(),
        &p.rest["decision"],
    );
    assert_eq!(got, (Some("merge_verdict"), Some("ran"), &json!("yes")));
    assert_eq!(p.rest["producer"]["kind"], "harness");
    assert_eq!(p.rest["facts"], json!({"diff": 4, "tests": 5, "review": 6}));
    assert_eq!(p.rest["facts_sha256"].as_str().unwrap().len(), 64);
    let row = &p.rest["session"];
    assert!(row["bin"].as_str().unwrap().ends_with("/claude"));
    assert_eq!(
        (&row["version"], &row["ran"]),
        (&json!("1 (fake)"), &json!(true))
    );

    let call = call(env.dir.path(), "claude", "verdict");
    let prompt = call["prompt"].as_str().unwrap();
    assert_eq!(
        env.store.get(row["prompt_ref"].as_str().unwrap()).unwrap(),
        prompt
    );
    for said in [
        "GOAL ship it",
        "```diff\n+x\n",
        "\"result\": \"pass\"",
        "BLOCKER",
        "\"lane_claim\": null",
    ] {
        assert!(prompt.contains(said), "{said}: {prompt}");
    }
    for unsaid in ["SUMMARY", "MERGE", "LOOK"] {
        assert!(!prompt.contains(unsaid), "{unsaid}: {prompt}");
    }
    assert!(prompt.ends_with("in the schema you were given.\n"));
    let usage = ledger.usage_get(1).unwrap().unwrap();
    assert_eq!(usage["totals"]["harness"]["messages"], 1);
}

/// A NO stands for its tree: the same tree replays it whatever the model
/// would say now, and an amend that keeps the tree (its tests and review
/// run again) carries it to the new commit. Only a change in the harness's
/// facts, here the owner waiving the blocker, or a new tree starts a
/// session.
#[test]
fn a_no_stands_for_its_tree() {
    let env = env(says("no"), json!([]), 10);
    let ledger = ledger(&env);
    let (h1, h2, h3) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
    let (t1, t2) = ("d".repeat(40), "e".repeat(40));
    let all = ["diff", "tests", "review"];
    measured(&ledger, &h1, &t1, &all);
    let no = judge(&env, &ledger, &h1, &t1);
    let decided = |r: &Recorded| r.payload.rest["decision"].as_str().map(String::from);
    assert_eq!((decided(&no), calls(&env)), (Some("no".into()), 1));
    rescript(&env, "claude", says("yes"));
    let again = judge(&env, &ledger, &h1, &t1);
    assert_eq!((again.replayed, again.event_id), (true, no.event_id));

    measured(&ledger, &h2, &t1, &all);
    let carried = judge(&env, &ledger, &h2, &t1);
    let p = &carried.payload;
    assert!(!carried.replayed && carried.event_id > no.event_id);
    assert_eq!(
        (p.head_sha.as_deref(), decided(&carried)),
        (Some(&*h2), Some("no".into()))
    );
    assert_eq!(p.rest["carried_from"], no.event_id);
    assert_eq!(p.rest["facts_sha256"], no.payload.rest["facts_sha256"]);
    assert_eq!(calls(&env), 1);
    let replayed = judge(&env, &ledger, &h2, &t1);
    assert_eq!(
        (replayed.replayed, replayed.event_id),
        (true, carried.event_id)
    );

    let owner = Actor {
        agent: Some("owner".into()),
        session: Some("o".into()),
    };
    for f in ledger.blocking_findings(1).unwrap() {
        let waive = FindingMove {
            check_event_id: f.check_event_id,
            finding_key: f.finding_key,
            status: "waived".into(),
            reason: Some("not for this PR".into()),
            head_sha: None,
        };
        ledger
            .finding_update(&owner, &waive, false)
            .unwrap()
            .unwrap();
    }
    let waived = judge(&env, &ledger, &h2, &t1);
    assert_eq!((decided(&waived), calls(&env)), (Some("yes".into()), 2));
    measured(&ledger, &h3, &t2, &all);
    judge(&env, &ledger, &h3, &t2);
    assert_eq!(calls(&env), 3);
}

/// A commit without a harness diff, tests result and complete review has
/// no facts to judge. A session that errors or answers outside the schema
/// records a skipped verdict with no decision, which no fact reads as one
/// and the next run does not replay.
#[test]
fn no_answer_is_no_decision() {
    let env = env(json!([]), json!([]), 10);
    let ledger = ledger(&env);
    let (head, tree) = ("2".repeat(40), "3".repeat(40));
    let lacks = || {
        verdict::harness(&ledger, &env.store, 1, &head, false)
            .unwrap()
            .err()
    };
    assert_eq!(lacks(), Some("diff"));
    measured(&ledger, &head, &tree, &["diff"]);
    assert_eq!(lacks(), Some("tests"));
    measured(&ledger, &head, &tree, &["tests"]);
    assert_eq!(lacks(), Some("complete review"));
    let mut partial = input(&pipeline());
    partial.fields = json!({"complete": false}).as_object().unwrap().clone();
    ledger
        .check_run(&lane_session(), &partial)
        .unwrap()
        .unwrap();
    assert_eq!(lacks(), Some("complete review"));
    measured(&ledger, &head, &tree, &["review"]);
    let said = |s: Value| json!([{"when": "LENS verdict", "stdout": session("v", s)}]);
    let answer = |decision: &str, why: &str| json!({"decision": decision, "why": why, "proven": [], "not_proven": []});
    let mut extra = answer("yes", "w");
    extra["score"] = json!(9);
    for (runs, error) in [
        (said(json!("Looks good.")), "not a verdict answer"),
        (said(answer("maybe", "w")), "unknown decision"),
        (said(extra), "unknown field"),
        (said(answer("yes", " ")), "no why"),
        (json!([{"when": "LENS verdict", "exit": 2}]), "exit Some(2)"),
    ] {
        rescript(&env, "claude", runs);
        let p = judge(&env, &ledger, &head, &tree).payload;
        let got = (p.outcome.as_deref(), &p.rest["reason"]);
        assert_eq!(got, (Some("skipped"), &json!("no_answer")));
        assert!(!p.rest.contains_key("decision"), "{error}");
        let row = &p.rest["session"];
        assert!(row["error"].as_str().unwrap().contains(error), "{row}");
        let facts = ledger.project(1, None).unwrap();
        assert_eq!(facts.get("verdict.decision"), &Fact::Absent);
    }
    rescript(&env, "claude", says("yes"));
    let p = judge(&env, &ledger, &head, &tree).payload;
    assert_eq!((p.outcome.as_deref(), calls(&env)), (Some("ran"), 6));
    let facts = ledger.project(1, None).unwrap();
    assert_eq!(facts.get("verdict.decision"), &Fact::Str("yes".into()));
}

/// `check run plan` on lane 1's commit for `text`, as the CLI runs it from
/// the lane's own session: the plan stored, [`plan::replay`], then
/// [`plan::record`].
fn attack(env: &Env, ledger: &Ledger, text: &str) -> Recorded {
    let pipeline = pipeline();
    let (p, attempts) = plan::plan(&pipeline).unwrap();
    let kept = Kept::of(text);
    let mut input = input(&pipeline);
    let key = (&*pipeline.hash, attempts);
    let stored = plan::replay(ledger, &mut input, key, &kept.stored.hash);
    if let Some(stored) = stored.unwrap().unwrap() {
        return stored;
    }
    let (dir, agents) = (env.checkout(), env.agents());
    let given = plan::Given {
        goal: "GOAL add a widget",
        plan: &kept,
        base: "1111111",
    };
    let me = lane_session();
    let got = plan::record(ledger, &me, input, &p, &agents, &dir, &given, &env.store);
    got.unwrap().unwrap()
}

/// The plan reviewer is given the goal and the plan, fenced, and its
/// findings are the plan check's, recorded from the lane's own session as
/// the harness's `plan_check` with the plan as evidence and a usage row;
/// the same plan replays without a session and a new one runs; a session
/// with no answer records a skipped check with no findings.
#[test]
fn the_plan_check_attacks_the_plan() {
    let said = json!({"findings": [found("risk", "it races")]});
    let runs = json!([{"when": "LENS plan", "stdout": session("p", said)}]);
    let env = env(runs, json!([]), 10);
    let ledger = ledger(&env);
    let token = format!("ghp_{}", "a".repeat(20));
    let first = attack(&env, &ledger, &format!("PLAN step one {token}"));
    let p = &first.payload;
    let got = (
        p.check.as_deref(),
        p.outcome.as_deref(),
        &p.rest["producer"]["kind"],
    );
    assert_eq!(got, (Some("plan_check"), Some("ran"), &json!("harness")));
    let Some(Findings::List(found)) = &p.findings else {
        panic!("no findings list: {p:?}");
    };
    assert_eq!((found.len(), &*found[0].title), (1, "it races"));
    assert!(found[0].finding_key.starts_with("plan-"));
    let stored = env.store.get(p.rest["plan_ref"].as_str().unwrap()).unwrap();
    assert_eq!(stored, "PLAN step one [redacted:github_token]");
    let call = call(env.dir.path(), "claude", "plan");
    let prompt = call["prompt"].as_str().unwrap();
    for said in [
        "```\nGOAL add a widget\n```",
        "```\nPLAN step one [redacted:github_token]\n```",
        "merge base is 1111111",
    ] {
        assert!(prompt.contains(said), "{said}: {prompt}");
    }
    assert!(!prompt.contains(&token));
    assert!(prompt.ends_with("in the schema you were given.\n"));
    assert_eq!(
        ledger.usage_get(1).unwrap().unwrap()["totals"]["harness"]["messages"],
        1
    );

    let again = attack(&env, &ledger, &format!("PLAN step one {token}"));
    assert_eq!(
        (again.replayed, again.event_id, calls(&env)),
        (true, first.event_id, 1)
    );
    rescript(
        &env,
        "claude",
        json!([{"when": "LENS plan", "stdout": "not json"}]),
    );
    let failed = attack(&env, &ledger, "PLAN step two").payload;
    let got = (
        failed.outcome.as_deref(),
        &failed.rest["reason"],
        &failed.findings,
    );
    assert_eq!(got, (Some("skipped"), &json!("no_answer"), &None));
    assert_eq!(calls(&env), 2);
    let facts = ledger.project(1, None).unwrap();
    assert_eq!(
        facts.get("plan_check.outcome"),
        &Fact::Str("skipped".into())
    );
    // A second session with no answer reaches plan_attempts (2): the next
    // run on that plan is refused and starts nothing.
    attack(&env, &ledger, "PLAN step two");
    let pipeline = pipeline();
    let plan_ref = Kept::of("PLAN step two").stored.hash;
    let mut input = input(&pipeline);
    let refused = plan::replay(&ledger, &mut input, (&pipeline.hash, 2), &plan_ref).unwrap();
    let Err(Refusal::InvalidInput(why)) = refused else {
        panic!("past plan_attempts is refused: {refused:?}");
    };
    assert!(why.contains("its limit plan_attempts"), "{why}");
    assert_eq!(calls(&env), 3);
}

/// An amend that keeps the tree carries the tree's complete review and
/// the verdict on it, though the lenses would now find something else: no
/// review session, no verdict session, and the NO stands. A change to the
/// tree starts both.
#[test]
fn an_amend_carries_the_review_and_the_verdict() {
    let (mut claude, codex) = scripts(rules);
    claude.as_array_mut().unwrap().push(says("no")[0].clone());
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let (h1, h2, h3) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
    let (t1, t2) = ("d".repeat(40), "e".repeat(40));
    measured(&ledger, &h1, &t1, &["diff", "tests"]);
    let first = reviewed(&env, &ledger, &h1, &t1).unwrap();
    let no = judge(&env, &ledger, &h1, &t1);
    let (before, blocking_before) = (calls(&env), blocking(&ledger));
    assert_eq!(no.payload.rest["decision"], "no");

    // The lenses would now word another finding, and the verdict say yes.
    let said = json!({"findings": [found("blocker", "DIFFERENT")]});
    let key = &review::findings("correctness", said.clone()).unwrap()[0].finding_key;
    let ruled = json!({"rulings": [{"key": key, "verdict": "verified", "why": "it does"}]});
    let none = json!({"findings": []});
    let ran = |lens: &str, said: &Value| json!({"when": format!("LENS {lens}"), "stdout": session(lens, said.clone())});
    let runs = json!([
        ran("correctness", &said),
        ran("design_tests", &none),
        ran("intent", &none),
        ran("verifier", &ruled),
        says("yes")[0],
    ]);
    rescript(&env, "claude", runs);

    measured(&ledger, &h2, &t1, &["diff", "tests"]);
    let carried = reviewed(&env, &ledger, &h2, &t1).unwrap();
    let p = &carried.payload;
    assert_eq!(
        (p.head_sha.as_deref(), &p.rest["carried_from"]),
        (Some(&*h2), &json!(first.event_id))
    );
    assert!(p.findings.is_none() && !carried.replayed);
    let amended = judge(&env, &ledger, &h2, &t1);
    assert_eq!(
        (
            &amended.payload.rest["decision"],
            &amended.payload.rest["carried_from"]
        ),
        (&json!("no"), &json!(no.event_id))
    );
    assert_eq!(
        (calls(&env), blocking(&ledger)),
        (before, blocking_before.clone())
    );

    measured(&ledger, &h3, &t2, &["diff", "tests"]);
    reviewed(&env, &ledger, &h3, &t2).unwrap();
    let changed = judge(&env, &ledger, &h3, &t2);
    assert_eq!(changed.payload.rest["decision"], "yes");
    // Three Claude lenses, Codex, the verifier and the verdict.
    assert_eq!(calls(&env), before + 6);
    assert!(blocking(&ledger).len() > blocking_before.len());
}

/// The owner, a session that never held lane 1.
fn owner() -> Actor {
    Actor {
        agent: Some("owner".into()),
        session: Some("o".into()),
    }
}

fn mv(ledger: &Ledger, who: &Actor, f: &BlockingFinding, to: &str) {
    let input = FindingMove {
        check_event_id: f.check_event_id,
        finding_key: f.finding_key.clone(),
        status: to.into(),
        reason: Some("said so".into()),
        head_sha: None,
    };
    ledger.finding_update(who, &input, false).unwrap().unwrap();
}

/// A NO stands for (lane, tree, pipeline) whatever else moves: an amend,
/// tests and smoke re-run under another base profile, a dispute, another
/// tree's review leaving its blockers on the lane before a return to the
/// first tree (and a ruling on one of them), and a new smoke claim start no
/// session. A waive by the
/// owner, after the NO, of a finding the NO saw lets exactly one session
/// run; its YES then stands, and a later waive does not reopen it.
#[test]
fn a_no_is_reopened_only_by_a_waive_or_a_refutation() {
    let env = env(says("no"), json!([]), 10);
    let ledger = ledger(&env);
    let (h1, h2, h3) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
    let (t1, t2) = ("d".repeat(40), "e".repeat(40));
    let all = ["diff", "tests", "review"];
    measured(&ledger, &h1, &t1, &all);
    let no = judge(&env, &ledger, &h1, &t1);
    assert_eq!(
        (&no.payload.rest["decision"], calls(&env)),
        (&json!("no"), 1)
    );
    rescript(&env, "claude", says("yes"));
    let stands = |head: &str, tree: &str, smoke: bool| {
        let r = judge_with(&env, &ledger, head, tree, smoke);
        assert_eq!(
            (&r.payload.rest["decision"], calls(&env)),
            (&json!("no"), 1),
            "{head}"
        );
        r
    };

    measured(&ledger, &h2, &t1, &all);
    stands(&h2, &t1, false);
    // The base moved to a profile with other tests and a smoke: re-run.
    let mut other = RunInput {
        check: "tests".into(),
        head: h2.clone(),
        tree: t1.clone(),
        profile: Some("another profile".into()),
        ..input(&pipeline())
    };
    other.fields = json!({"result": "pass", "commands": [{"argv": ["other"], "exit": 0}]})
        .as_object()
        .unwrap()
        .clone();
    ledger.check_run(&lane_session(), &other).unwrap().unwrap();
    other.check = "smoke".into();
    ledger.check_run(&lane_session(), &other).unwrap().unwrap();
    stands(&h2, &t1, true);
    let seen = ledger.blocking_findings(1).unwrap();
    let before_no: Vec<_> = seen
        .iter()
        .filter(|f| f.check_event_id < no.event_id)
        .collect();
    mv(&ledger, &lane_session(), before_no[0], "disputed");
    stands(&h2, &t1, true);
    measured(&ledger, &h3, &t2, &all);
    stands(&h1, &t1, false);
    // A ruling on a finding raised after the NO (the other tree's) is no
    // ruling on what the NO saw.
    let after = ledger.blocking_findings(1).unwrap();
    let after = after
        .iter()
        .rfind(|f| f.check_event_id > no.event_id)
        .unwrap();
    mv(&ledger, &owner(), after, "refuted");
    stands(&h1, &t1, false);
    let claim = CheckInput {
        work: 1,
        check: "smoke".into(),
        head: h1.clone(),
        outcome: "ran".into(),
        ..CheckInput::default()
    };
    let fresh = Actor {
        agent: Some("a".into()),
        session: Some("fresh".into()),
    };
    ledger.check_record(&fresh, &claim).unwrap().unwrap();
    stands(&h1, &t1, false);

    mv(&ledger, &owner(), before_no[0], "waived");
    let yes = judge(&env, &ledger, &h1, &t1);
    assert_eq!(
        (&yes.payload.rest["decision"], calls(&env)),
        (&json!("yes"), 2)
    );
    let again = judge(&env, &ledger, &h1, &t1);
    assert_eq!(
        (again.replayed, again.event_id, calls(&env)),
        (true, yes.event_id, 2)
    );
    let later = ledger.blocking_findings(1).unwrap();
    mv(&ledger, &owner(), &later[0], "waived");
    let still = judge(&env, &ledger, &h1, &t1);
    assert_eq!((still.event_id, calls(&env)), (yes.event_id, 2));
}

/// Flipping between two commits of one tree carries the review and the
/// verdict once to each, then every run replays: nothing more is appended
/// and no session starts.
#[test]
fn flipping_between_commits_of_a_tree_appends_nothing() {
    let (mut claude, codex) = scripts(rules);
    claude.as_array_mut().unwrap().push(says("no")[0].clone());
    let env = env(claude, codex, 10);
    let ledger = ledger(&env);
    let (h1, h2, t) = ("a".repeat(40), "b".repeat(40), "d".repeat(40));
    let mut seen = std::collections::BTreeMap::new();
    for (round, head) in [&h1, &h2, &h1, &h2, &h1, &h2].into_iter().enumerate() {
        measured(&ledger, head, &t, &["diff", "tests"]);
        let review = reviewed(&env, &ledger, head, &t).unwrap();
        let verdict = judge(&env, &ledger, head, &t);
        let ids = (review.event_id, verdict.event_id);
        let first = *seen.entry(head.clone()).or_insert(ids);
        assert_eq!(ids, first, "round {round}");
        assert_eq!(
            review.replayed && verdict.replayed,
            round >= 2,
            "round {round}"
        );
    }
    // The first commit's four Claude sessions, Codex and the verdict.
    assert_eq!(calls(&env), 6);
}
