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
pub use facts::{Fact, FactBundle};
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
    /// SHA-256 of the canonical JSON of the limits, gates and transitions,
    /// lowercase hex.
    pub hash: String,
    pub limits: BTreeMap<String, i64>,
    pub gates: BTreeMap<String, Gate>,
    /// Rows may share an id; `(id, route, to)` is unique and picks the row.
    /// Their order is not behaviour (the hash sorts them).
    pub transitions: Vec<Transition>,
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
        load::load(src)
    }
}

/// The documents this binary can load, newest last. A claim stamps the
/// newest; a lane stamped with anything else is `pipeline_unknown`.
static KNOWN: LazyLock<Vec<Pipeline>> = LazyLock::new(|| {
    [
        include_str!("../../../../pipelines/v17.toml"),
        include_str!("../../../../pipelines/v17.1.toml"),
    ]
    .iter()
    .map(|src| Pipeline::load(src).expect("embedded pipelines load (*_loads_with_pinned_hash)"))
    .collect()
});

/// The pipeline a new claim stamps.
pub fn current() -> &'static Pipeline {
    &KNOWN[KNOWN.len() - 1]
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
    fn by_stamp_needs_id_and_hash() {
        assert!(by_stamp("v17", V17_HASH).is_some());
        assert!(by_stamp("v17.1", V17_1_HASH).is_some());
        assert!(by_stamp("v18", V17_HASH).is_none());
        assert_eq!(current().id, "v17.1");
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
        let e = load_err("schema = 1", "schema = 2");
        assert!(matches!(e, PipelineError::Schema(2)), "{e}");
    }
}
