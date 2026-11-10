//! Pipeline documents: a versioned TOML state machine (R7). [`Pipeline::load`]
//! parses and validates one document and computes its semantic hash. A
//! document that does not validate is never loaded, so the engine only ever
//! evaluates gates it understood.

mod error;
mod eval;
mod facts;
mod hash;
mod load;

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

pub use error::PipelineError;
pub use eval::Denied;
pub use eval::sha_prefix_eq;
pub use facts::{FACTS, Fact, FactBundle};
pub(crate) use hash::canonical_string;

/// Transition ids the host calls by name. A document without them does not load.
pub const REQUIRED_TRANSITIONS: &[&str] = &["handoff"];

/// Event names `event_before` may compare, with the fact path that carries
/// each one's time. Any other name would make the gate vacuously pass, so it
/// is a load error. Widen it when the projection times more events.
pub const EVENTS: &[(&str, &str)] = &[
    ("pr.opened", "pr.opened_at"),
    ("quality_gate", "quality_gate.at"),
];

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub id: String,
    /// 1, or 2 for a document whose checks the harness runs: on a lane it
    /// stamps, the write takes those checks from the harness only.
    pub schema: i64,
    /// SHA-256 of the canonical JSON of the limits, gates and transitions
    /// (schema 2: and the schema, runners and checks), lowercase hex.
    pub hash: String,
    pub limits: BTreeMap<String, i64>,
    pub gates: BTreeMap<String, Gate>,
    /// Rows may share an id; `(id, route, to)` is unique and picks the row.
    /// Their order is not behaviour (the hash sorts them).
    pub transitions: Vec<Transition>,
    /// Schema 2: the headless agents the harness starts, by name.
    pub runners: BTreeMap<String, Runner>,
    /// Schema 2: the checks the harness runs itself, by name.
    pub checks: BTreeMap<String, Check>,
}

/// How one kind of headless session is started. Its numbers are `[limits]`
/// entries (`budget_limit` in US cents, `timeout_limit` in seconds),
/// resolved here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Runner {
    pub cli: Cli,
    /// None is the CLI's own default.
    pub model: Option<String>,
    /// The built-in tools the session may use, sorted, each once.
    pub tools: Vec<String>,
    pub budget_cents: i64,
    pub timeout_secs: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cli {
    Claude,
    Codex,
}

/// A check the harness runs with a runner and a prompt file this binary
/// carries. The prompt's digest is part of the semantic hash, so a changed
/// prompt is a new, measurable pipeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub runner: String,
    pub prompt: String,
    pub prompt_sha256: String,
    /// The prompt itself: the session is given it. Hashed by its digest.
    #[serde(skip)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub id: String,
    pub pred: Pred,
}

/// A host predicate with its arguments resolved: limits are numbers here.
/// Fields are fact paths. An absent fact denies, and so does `not` over a
/// gate that read an absent fact. Two read absence on purpose: `present`
/// is false on it (so `not(present)` holds), and `event_before` holds when
/// the later event is absent.
#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    Present {
        field: String,
    },
    Eq {
        field: String,
        value: Operand,
    },
    Lte {
        field: String,
        value: i64,
    },
    Gt {
        field: String,
        value: i64,
    },
    /// `sha_prefix_eq`: `left` is a prefix of `right`. `sha_same_commit`
    /// (`both_ways`): the shorter of the two is a prefix of the longer.
    ShaPrefixEq {
        left: String,
        right: String,
        min_len: i64,
        max_len: i64,
        both_ways: bool,
    },
    OneOf {
        field: String,
        values: Vec<Literal>,
    },
    PlanCheckMatchesLane {
        reasons: Vec<String>,
    },
    QualityGateMatchesLane {
        reasons: Vec<String>,
    },
    /// The time fact paths of the two events (from [`EVENTS`]).
    EventBefore {
        earlier: String,
        later: String,
    },
    All(Vec<String>),
    Any(Vec<String>),
    Not(String),
}

impl Pred {
    /// The fact paths this predicate itself reads, in the order a denial
    /// lists them. A combinator reads none: its members do.
    pub(crate) fn fact_paths(&self) -> Vec<String> {
        let lane = |check: &str| {
            let (outcome, reason) = (format!("{check}.outcome"), format!("{check}.reason"));
            vec![outcome, "lane.changes_code".into(), reason]
        };
        match self {
            Pred::Present { field }
            | Pred::Lte { field, .. }
            | Pred::Gt { field, .. }
            | Pred::OneOf { field, .. }
            | Pred::Eq {
                field,
                value: Operand::Literal(_),
            } => vec![field.clone()],
            Pred::Eq {
                field,
                value: Operand::Fact(other),
            } => vec![field.clone(), other.clone()],
            Pred::ShaPrefixEq { left, right, .. } => vec![left.clone(), right.clone()],
            Pred::EventBefore { earlier, later } => vec![earlier.clone(), later.clone()],
            Pred::PlanCheckMatchesLane { .. } => lane("plan_check"),
            Pred::QualityGateMatchesLane { .. } => lane("quality_gate"),
            Pred::All(_) | Pred::Any(_) | Pred::Not(_) => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Literal(Literal),
    /// Another fact path (`value_from`).
    Fact(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Str(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    pub routes: Vec<Route>,
    /// `None` applies from any phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Phase>,
    pub to: Phase,
    /// Evaluated in order; the first failing gate is the denial.
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    CodePr,
    Direct,
    NoPr,
    Investigation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Claimed,
    Planning,
    Building,
    PrePr,
    PrOpen,
    Addressing,
    LocalReview,
    FinalHead,
    Verdict,
    Ready,
    NeedsYou,
    Done,
}

impl Pipeline {
    pub fn load(src: &str) -> Result<Self, PipelineError> {
        load::load(src, &[])
    }

    /// [`Pipeline::load`] for a schema 2 document whose checks name prompt
    /// files: `prompts` is each file's name and text.
    pub fn load_with_prompts(src: &str, prompts: &[(&str, &str)]) -> Result<Self, PipelineError> {
        load::load(src, prompts)
    }
}

/// The prompt files a schema 2 check names, as this binary carries them.
const PROMPT_FILES: &[(&str, &str)] = &[
    (
        "plan.md",
        include_str!("../../../../pipelines/prompts/plan.md"),
    ),
    (
        "review_codex.md",
        include_str!("../../../../pipelines/prompts/review_codex.md"),
    ),
    (
        "review_correctness.md",
        include_str!("../../../../pipelines/prompts/review_correctness.md"),
    ),
    (
        "review_design_tests.md",
        include_str!("../../../../pipelines/prompts/review_design_tests.md"),
    ),
    (
        "review_intent.md",
        include_str!("../../../../pipelines/prompts/review_intent.md"),
    ),
    (
        "review_verifier.md",
        include_str!("../../../../pipelines/prompts/review_verifier.md"),
    ),
    (
        "verdict.md",
        include_str!("../../../../pipelines/prompts/verdict.md"),
    ),
];

/// The documents this binary can load, oldest first; a lane stamped with
/// anything else is `pipeline_unknown`.
static KNOWN: LazyLock<Vec<Pipeline>> = LazyLock::new(|| {
    [
        include_str!("../../../../pipelines/v17.toml"),
        include_str!("../../../../pipelines/v17.1.toml"),
        include_str!("../../../../pipelines/v19.toml"),
        include_str!("../../../../pipelines/v20.toml"),
    ]
    .iter()
    .map(|src| {
        Pipeline::load_with_prompts(src, PROMPT_FILES)
            .expect("embedded pipelines load (*_loads_with_pinned_hash)")
    })
    .collect()
});

/// Where [`current`] is in [`KNOWN`]: v19, while v20 is newer. The owner
/// moves it at the M3 cutover, with no lane running.
const DEFAULT: usize = 2;

/// The pipeline a new claim stamps.
pub fn current() -> &'static Pipeline {
    &KNOWN[DEFAULT]
}

/// The embedded documents newer than [`current`], which a first claim may
/// stamp instead (`work claim --pipeline`): a pilot lane before the cutover.
pub fn newer() -> &'static [Pipeline] {
    &KNOWN[DEFAULT + 1..]
}

/// The loaded document for a lane's stamp, `None` when this binary has no
/// document with that id and hash.
pub fn by_stamp(id: &str, hash: &str) -> Option<&'static Pipeline> {
    KNOWN.iter().find(|p| p.id == id && p.hash == hash)
}

impl Route {
    /// The route a lane takes, chosen once at its first claim (events.md,
    /// lane facts in M1): an investigation, else no PR on a `none` flow,
    /// else direct on a direct flow, else a PR.
    pub fn for_lane(kind: &str, pr_flow: &str) -> Route {
        match (kind, pr_flow) {
            ("investigation", _) => Route::Investigation,
            (_, "none") => Route::NoPr,
            (_, "direct") => Route::Direct,
            _ => Route::CodePr,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Route::CodePr => "code_pr",
            Route::Direct => "direct",
            Route::NoPr => "no_pr",
            Route::Investigation => "investigation",
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const V17: &str = include_str!("../../../../pipelines/v17.toml");
    const V17_HASH: &str = "1a8a599ca26abaf2ab5b1dc9aaa301c019237d45fa3293f4326da0b47b6ea2c5";
    const V17_1: &str = include_str!("../../../../pipelines/v17.1.toml");
    const V17_1_HASH: &str = "d99d9de3d9fe7e59aa046cabdb80a6d34670d5a1dfcab1873a248ba245e32fd5";
    const V19: &str = include_str!("../../../../pipelines/v19.toml");
    const V19_HASH: &str = "d45442e63a1b77001987bb5d82e987d730c26ddf4ba7c2941b6c09622287f5ea";
    const V20: &str = include_str!("../../../../pipelines/v20.toml");
    const V20_HASH: &str = "daeb37cde755148f1420e5900c2aa43983f1d176c84827d0c8ea124e88a0829d";

    const MINI: &str = r#"
schema = 1
id = "t"

[limits]
n = 1

[[gate]]
id = "a"
fn = "eq"
args = { field = "x", value = 1 }

[[transition]]
id = "handoff"
routes = ["code_pr"]
to = "done"
requires = ["a"]
"#;

    fn hash(src: &str) -> String {
        Pipeline::load(src).unwrap().hash
    }

    fn load_err(from: &str, to: &str) -> PipelineError {
        let src = MINI.replacen(from, to, 1);
        assert_ne!(src, MINI, "edit did not apply: {from}");
        Pipeline::load(&src).unwrap_err()
    }

    #[test]
    fn v17_loads_with_pinned_hash() {
        let p = Pipeline::load(V17).unwrap();
        assert_eq!(p.id, "v17");
        assert_eq!(p.hash, V17_HASH);
        assert_eq!(
            p.gates["verdict_on_head"].pred,
            Pred::ShaPrefixEq {
                left: "verdict.sha".into(),
                right: "commit.sha".into(),
                min_len: 7,
                max_len: 40,
                both_ways: false,
            }
        );
        assert_eq!(
            p.gates["code_lines_at_most"].pred,
            Pred::Lte {
                field: "diff.code_lines".into(),
                value: 750,
            }
        );
        let handoff: Vec<_> = p.transitions.iter().filter(|t| t.id == "handoff").collect();
        assert_eq!(handoff.len(), 2);
        assert!(handoff.iter().all(|t| t.from.is_none()));
        let ready = handoff.iter().find(|t| t.to == Phase::Ready).unwrap();
        assert!(!ready.routes.contains(&Route::Investigation));
    }

    #[test]
    fn v17_1_loads_with_pinned_hash() {
        let p = Pipeline::load(V17_1).unwrap();
        assert_eq!((p.id.as_str(), p.hash.as_str()), ("v17.1", V17_1_HASH));
        assert!(matches!(
            p.gates["verdict_on_head"].pred,
            Pred::ShaPrefixEq {
                both_ways: true,
                ..
            }
        ));
    }

    #[test]
    fn v19_loads_with_pinned_hash() {
        let p = Pipeline::load(V19).unwrap();
        assert_eq!((p.id.as_str(), p.hash.as_str()), ("v19", V19_HASH));
        // CI is not a gate.
        assert!(!p.gates.contains_key("ci_not_red"));
    }

    /// v20 names every check the harness asks for by name, and the limits
    /// that cap its runs.
    #[test]
    fn v20_loads_with_pinned_hash() {
        let p = Pipeline::load_with_prompts(V20, PROMPT_FILES).unwrap();
        assert_eq!((p.id.as_str(), p.hash.as_str()), ("v20", V20_HASH));
        let review = crate::review::plan(&p).unwrap();
        assert_eq!(review.attempts, 2);
        crate::verdict::plan(&p).unwrap();
        crate::plan::plan(&p).unwrap();
        for limit in ["verdict_attempts", "plan_attempts"] {
            assert!(p.limits[limit] >= 1, "{limit}");
        }
        assert_eq!(p.limits["code_lines"], 400);
        for (name, check) in &p.checks {
            let file = PROMPT_FILES.iter().find(|(f, _)| *f == check.prompt);
            assert_eq!(check.text, file.unwrap().1, "{name}");
        }
        // Every prompt file is a check's.
        assert_eq!(p.checks.len(), PROMPT_FILES.len());
        // Only the gated handoff runs, so v20 has no other row.
        assert!(p.transitions.iter().all(|t| t.id == "handoff"));
        assert_eq!(by_stamp("v20", V20_HASH).unwrap().id, "v20");
    }

    #[test]
    fn by_stamp_needs_id_and_hash() {
        assert!(by_stamp("v17", V17_HASH).is_some());
        assert!(by_stamp("v17.1", V17_1_HASH).is_some());
        assert!(by_stamp("v19", V19_HASH).is_some());
        assert!(by_stamp("v18", V17_HASH).is_none());
        assert_eq!(current().id, "v19");
        let newer: Vec<_> = newer().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(newer, ["v20"]);
    }

    #[test]
    fn route_for_lane() {
        for (kind, flow, route) in [
            ("code", "pr", Route::CodePr),
            ("docs", "pr", Route::CodePr),
            ("measure", "pr", Route::CodePr),
            ("code", "direct", Route::Direct),
            ("docs", "direct", Route::Direct),
            ("code", "none", Route::NoPr),
            ("measure", "none", Route::NoPr),
            ("investigation", "pr", Route::Investigation),
            ("investigation", "direct", Route::Investigation),
            ("investigation", "none", Route::Investigation),
        ] {
            assert_eq!(Route::for_lane(kind, flow), route, "{kind} {flow}");
            assert_eq!(json!(route), json!(route.as_str()));
        }
    }

    #[test]
    fn hash_ignores_comments_key_order_defaults_published_and_cohort() {
        let base = hash(V17);
        // Move the first gate to the end of the gates.
        let first = V17.find("[[gate]]").unwrap();
        let second = first + 1 + V17[first + 1..].find("[[gate]]").unwrap();
        let moved = format!(
            "{}{}",
            &V17[..first],
            V17[second..].replacen("# --- M1", &format!("{}# --- M1", &V17[first..second]), 1)
        );
        assert_ne!(moved, V17);
        assert_eq!(hash(&moved), base);

        let edited = V17
            .replace("# --- gates", "# --- gates, reworded")
            .replace("schema = 1\n", "schema = 1\n# a new comment\n")
            .replace("2026-11-01T17:30:00Z", "2026-12-01T00:00:00Z")
            .replace("target_lanes = 25", "target_lanes = 40")
            .replace(
                r#"args = { field = "diff.code_lines", limit = "code_lines" }"#,
                r#"args = { limit = "code_lines", field = "diff.code_lines" }"#,
            )
            .replacen(
                "id = \"stamp_id\"\nfn = \"eq\"",
                "fn = \"eq\"\nid = \"stamp_id\"",
                1,
            )
            .replace(
                "routes = [\"code_pr\", \"direct\", \"no_pr\", \"investigation\"]\nfrom = \"claimed\"\nto = \"planning\"\n",
                "routes = [\"investigation\", \"no_pr\", \"direct\", \"code_pr\"]\nfrom = \"claimed\"\nto = \"planning\"\nrequires = []\n",
            );
        assert_eq!(edited.matches("requires = []").count(), 1);
        assert_eq!(hash(&edited), base);
    }

    #[test]
    fn hash_changes_on_argument_limit_or_requires_order_edit() {
        let base = hash(V17);
        let arg = V17.replace(
            r#"args = { field = "verdict.decision", value = "yes" }"#,
            r#"args = { field = "verdict.decision", value = "Yes" }"#,
        );
        let limit = V17.replace("code_lines = 750", "code_lines = 400");
        let order = V17.replace(
            r#"requires = ["previous_round_closed", "rounds_at_most"]"#,
            r#"requires = ["rounds_at_most", "previous_round_closed"]"#,
        );
        for edited in [arg, limit, order] {
            assert_ne!(edited, V17);
            assert_ne!(hash(&edited), base);
        }
    }

    #[test]
    fn unknown_fn_is_a_load_error() {
        let e = load_err(r#"fn = "eq""#, r#"fn = "regex""#);
        assert!(matches!(e, PipelineError::UnknownFn { .. }), "{e}");
    }

    #[test]
    fn unknown_gate_in_requires_or_combinator_is_a_load_error() {
        let e = load_err(r#"requires = ["a"]"#, r#"requires = ["b"]"#);
        assert!(matches!(e, PipelineError::UnknownGate { .. }), "{e}");
        let e = load_err(
            r#"fn = "eq"
args = { field = "x", value = 1 }"#,
            r#"fn = "any"
args = { of = ["b"] }"#,
        );
        assert!(matches!(e, PipelineError::UnknownGate { .. }), "{e}");
    }

    #[test]
    fn cyclic_combinators_are_a_load_error() {
        let e = load_err(
            r#"fn = "eq"
args = { field = "x", value = 1 }"#,
            r#"fn = "all"
args = { of = ["b"] }

[[gate]]
id = "b"
fn = "not"
args = { of = "a" }"#,
        );
        assert!(matches!(e, PipelineError::Cycle(_)), "{e}");
        let e = load_err(
            r#"fn = "eq"
args = { field = "x", value = 1 }"#,
            r#"fn = "any"
args = { of = ["a"] }"#,
        );
        assert!(matches!(e, PipelineError::Cycle(_)), "{e}");
    }

    #[test]
    fn unknown_or_missing_transition_or_phase_is_a_load_error() {
        let e = load_err(r#"id = "handoff""#, r#"id = "hand_off""#);
        assert!(
            matches!(e, PipelineError::MissingTransition("handoff")),
            "{e}"
        );
        let e = load_err(r#"to = "done""#, r#"to = "redy""#);
        assert!(matches!(e, PipelineError::Toml(_)), "{e}");
        let e = load_err(r#"routes = ["code_pr"]"#, r#"routes = ["code-pr"]"#);
        assert!(matches!(e, PipelineError::Toml(_)), "{e}");
    }

    #[test]
    fn duplicate_ids_are_a_load_error() {
        let dup_gate = MINI.replace(
            "[[transition]]",
            "[[gate]]\nid = \"a\"\nfn = \"present\"\nargs = { field = \"x\" }\n\n[[transition]]",
        );
        let e = Pipeline::load(&dup_gate).unwrap_err();
        assert!(matches!(e, PipelineError::DuplicateGate(_)), "{e}");

        let row =
            "[[transition]]\nid = \"handoff\"\nroutes = [\"direct\", \"code_pr\"]\nto = \"done\"\n";
        let e = Pipeline::load(&format!("{MINI}\n{row}")).unwrap_err();
        assert!(
            matches!(e, PipelineError::DuplicateTransition { .. }),
            "{e}"
        );
        // Same id on another route, or to another phase, is a separate row.
        Pipeline::load(&format!(
            "{MINI}\n{}",
            row.replace("\"direct\", \"code_pr\"", "\"direct\"")
        ))
        .unwrap();
        Pipeline::load(&format!("{MINI}\n{}", row.replace("done", "ready"))).unwrap();
    }

    #[test]
    fn bad_arguments_are_a_load_error() {
        let cases = [
            (r#"value = 1 }"#, r#"value = 1, limit = "n" }"#),
            (r#"value = 1 }"#, r#"value = 1.5 }"#),
            (r#"value = 1 }"#, r#"value = 1, extra = 2 }"#),
            (r#"field = "x", "#, ""),
        ];
        for (from, to) in cases {
            let e = load_err(from, to);
            assert!(matches!(e, PipelineError::BadArgs { .. }), "{to}: {e}");
        }
        let sha = |min: i64, max: i64| {
            MINI.replace("n = 1", &format!("n = 1\nlo = {min}\nhi = {max}"))
                .replace(
                    r#"fn = "eq"
args = { field = "x", value = 1 }"#,
                    r#"fn = "sha_prefix_eq"
args = { left = "a", right = "b", min_limit = "lo", max_limit = "hi" }"#,
                )
        };
        Pipeline::load(&sha(7, 40)).unwrap();
        for (min, max) in [(0, 40), (50, 40)] {
            let e = Pipeline::load(&sha(min, max)).unwrap_err();
            assert!(
                matches!(e, PipelineError::BadArgs { .. }),
                "{min}..{max}: {e}"
            );
        }
        let e = load_err(r#"value = 1 }"#, r#"limit = "m" }"#);
        assert!(matches!(e, PipelineError::UnknownLimit { .. }), "{e}");
        let e = load_err(
            r#"fn = "eq"
args = { field = "x", value = 1 }"#,
            r#"fn = "event_before"
args = { earlier = "quality_gate", later = "pr.opend" }"#,
        );
        assert!(matches!(e, PipelineError::UnknownEvent { .. }), "{e}");
    }

    #[test]
    fn unknown_fields_and_schema_are_a_load_error() {
        let e = load_err(r#"fn = "eq""#, "since = \"v17\"\nfn = \"eq\"");
        assert!(matches!(e, PipelineError::Toml(_)), "{e}");
        let e = load_err(r#"id = "t""#, "id = \"t\"\nextends = \"v16\"");
        assert!(matches!(e, PipelineError::Toml(_)), "{e}");
        let e = load_err("schema = 1", "schema = 3");
        assert!(matches!(e, PipelineError::Schema(3)), "{e}");
    }

    const MINI2: &str = r#"
schema = 2
id = "t2"

[limits]
budget = 200
timeout = 900

[runners.lens]
cli = "claude"
model = "opus"
tools = ["Read", "Grep"]
budget_limit = "budget"
timeout_limit = "timeout"

[runners.second]
cli = "codex"
budget_limit = "budget"
timeout_limit = "timeout"

[checks.review]
runner = "lens"
prompt = "review.md"

[[gate]]
id = "a"
fn = "eq"
args = { field = "verdict.decision", value = "yes" }

[[transition]]
id = "handoff"
routes = ["code_pr"]
to = "done"
requires = ["a"]
"#;
    const PROMPTS: &[(&str, &str)] = &[("review.md", "Review the diff.")];

    fn load2(from: &str, to: &str) -> Result<Pipeline, PipelineError> {
        let src = MINI2.replacen(from, to, 1);
        assert_ne!(src, MINI2, "edit did not apply: {from}");
        Pipeline::load_with_prompts(&src, PROMPTS)
    }

    #[test]
    fn schema_2_resolves_runners_and_checks() {
        let p = Pipeline::load_with_prompts(MINI2, PROMPTS).unwrap();
        let lens = Runner {
            cli: Cli::Claude,
            model: Some("opus".into()),
            tools: vec!["Grep".into(), "Read".into()],
            budget_cents: 200,
            timeout_secs: 900,
        };
        assert_eq!(p.runners["lens"], lens);
        assert_eq!(
            (p.runners["second"].cli, &p.runners["second"].model),
            (Cli::Codex, &None)
        );
        let review = &p.checks["review"];
        assert_eq!(
            (review.runner.as_str(), review.prompt.as_str()),
            ("lens", "review.md")
        );
        assert_eq!(review.text, "Review the diff.");
        // sha256 of "Review the diff."
        let digest = "cd952634e4090147e67501128bd1b92e8a54b749568ada27af25a86a31b70059";
        assert_eq!(review.prompt_sha256, digest);
        assert!(Pipeline::load(V19).unwrap().runners.is_empty());
    }

    /// The prompt's text, a runner's model and the schema are behaviour;
    /// a comment is not.
    #[test]
    fn schema_2_hash_covers_prompts_runners_and_the_schema() {
        let base = Pipeline::load_with_prompts(MINI2, PROMPTS).unwrap().hash;
        let edited = Pipeline::load_with_prompts(MINI2, &[("review.md", "Review it.")]);
        assert_ne!(edited.unwrap().hash, base);
        assert_ne!(load2("opus", "haiku").unwrap().hash, base);
        assert_ne!(load2("budget = 200", "budget = 300").unwrap().hash, base);
        assert_eq!(
            load2("id = \"t2\"", "id = \"t2\" # note").unwrap().hash,
            base
        );
        // The order tools are written in is not behaviour.
        let swapped = load2(r#"["Read", "Grep"]"#, r#"["Grep", "Read", "Grep"]"#).unwrap();
        assert_eq!(swapped.hash, base);
        // Pinned: the hashed keys are the structs' field names, so a rename
        // would restamp every schema 2 lane.
        assert_eq!(
            base,
            "7bb88d804e9c9a831e0336800695bd466804fe55eece5eb17b2253260b633e40"
        );
    }

    /// Each fn's fact paths are checked, whichever argument names them.
    #[test]
    fn schema_2_checks_every_fn_s_facts() {
        let gate = |func: &str, args: &str| {
            let old = r#"fn = "eq"
args = { field = "verdict.decision", value = "yes" }"#;
            load2(old, &format!("fn = \"{func}\"\nargs = {{ {args} }}"))
        };
        let limits = r#"min_limit = "budget", max_limit = "timeout""#;
        for (func, good, bad) in [
            (
                "present",
                r#"field = "verdict.sha""#.to_string(),
                r#"field = "x.y""#.to_string(),
            ),
            (
                "lte",
                r#"field = "review.local_rounds", value = 3"#.into(),
                r#"field = "x.y", value = 3"#.into(),
            ),
            (
                "gt",
                r#"field = "review.local_rounds", value = 0"#.into(),
                r#"field = "x.y", value = 0"#.into(),
            ),
            (
                "one_of",
                r#"field = "verdict.decision", values = ["yes"]"#.into(),
                r#"field = "x.y", values = ["yes"]"#.into(),
            ),
            (
                "eq",
                r#"field = "verdict.sha", value_from = "commit.sha""#.into(),
                r#"field = "verdict.sha", value_from = "x.y""#.into(),
            ),
            (
                "sha_prefix_eq",
                format!(r#"left = "verdict.sha", right = "commit.sha", {limits}"#),
                format!(r#"left = "verdict.sha", right = "x.y", {limits}"#),
            ),
            (
                "sha_same_commit",
                format!(r#"left = "verdict.sha", right = "commit.sha", {limits}"#),
                format!(r#"left = "x.y", right = "commit.sha", {limits}"#),
            ),
            (
                "plan_check_matches_lane",
                r#"reasons = ["no-code"]"#.into(),
                String::new(),
            ),
            (
                "event_before",
                r#"earlier = "quality_gate", later = "pr.opened""#.into(),
                String::new(),
            ),
        ] {
            gate(func, &good).unwrap_or_else(|e| panic!("{func}: {e}"));
            if !bad.is_empty() {
                let e = gate(func, &bad).unwrap_err().to_string();
                assert_eq!(
                    e, "gate a reads x.y, a fact the projection never sets",
                    "{func}"
                );
            }
        }
        // The paths the fixed-path fns read are the projection's too.
        for (_, path) in EVENTS {
            assert!(FACTS.contains(path), "{path}");
        }
    }

    #[test]
    fn schema_2_refuses_what_could_not_run() {
        let err = |from: &str, to: &str| load2(from, to).unwrap_err().to_string();
        assert_eq!(
            err("verdict.decision", "diff.code_lines"),
            "gate a reads diff.code_lines, a fact the projection never sets"
        );
        assert_eq!(
            err(
                r#"args = { field = "verdict.decision", value = "yes" }"#,
                r#"args = { field = "verdict.sha", value_from = "head.sha" }"#
            ),
            "gate a reads head.sha, a fact the projection never sets"
        );
        assert_eq!(
            err(r#"runner = "lens""#, r#"runner = "lense""#),
            "check review names unknown runner lense"
        );
        assert_eq!(
            err("review.md", "verdict.md"),
            "check review names prompt verdict.md, which this binary does not carry"
        );
        assert_eq!(
            err(r#"budget_limit = "budget""#, r#"budget_limit = "cash""#),
            "runner lens: unknown limit cash"
        );
        assert_eq!(
            err("timeout = 900", "timeout = 0"),
            "runner lens: limit timeout is 0, not at least 1"
        );
        assert!(matches!(
            load2(r#"cli = "claude""#, r#"cli = "cursor""#),
            Err(PipelineError::Toml(_))
        ));
        // Schema 1 hashes neither table, so it may not carry them.
        let e = load2("schema = 2", "schema = 1").unwrap_err();
        assert!(matches!(e, PipelineError::NeedsSchema2("runners")), "{e}");
        let checks_only = format!(
            "schema = 1\nid = \"t\"\n{}",
            &MINI2[MINI2.find("[checks").unwrap()..]
        );
        let e = Pipeline::load_with_prompts(&checks_only, PROMPTS).unwrap_err();
        assert!(matches!(e, PipelineError::NeedsSchema2("checks")), "{e}");
    }
}
