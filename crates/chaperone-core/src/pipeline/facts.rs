use std::collections::BTreeMap;

/// Every path the projection sets (`ledger/projection.rs`; its tests hold
/// it to this list). A schema 2 gate that reads any other path does not
/// load: it could never pass. Listed is not always set: `commit.sha` is
/// the head the caller passes, which only a handoff does today, and the
/// harness's results on that commit (`commit.code_lines`, `tests.passed`,
/// `review.complete`, ...) are set only with it.
pub const FACTS: &[&str] = &[
    "commit.changes_code",
    "commit.code_lines",
    "commit.sha",
    "daisy_review.state",
    "findings.open_blocking",
    "kept_tests.recorded",
    "lane.changes_code",
    "pipeline.hash",
    "pipeline.id",
    "plan_check.outcome",
    "plan_check.reason",
    "pr.opened_at",
    "quality_gate.at",
    "quality_gate.outcome",
    "quality_gate.reason",
    "review.local_head",
    "review.local_rounds",
    "review.complete",
    "review.previous_round_closed",
    "smoke.claimed",
    "smoke.passed",
    "smoke.recorded",
    "tests.passed",
    "verdict.decision",
    "verdict.sha",
];

/// One fact's value. A path with no value reads as [`Fact::Absent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Absent,
    Bool(bool),
    Int(i64),
    Str(String),
    /// An event time as the ledger writes it, RFC 3339 UTC with milliseconds
    /// (`2026-11-03T05:35:23.321Z`). That format orders as text.
    Time(String),
}

impl From<bool> for Fact {
    fn from(b: bool) -> Self {
        Fact::Bool(b)
    }
}

impl From<i64> for Fact {
    fn from(i: i64) -> Self {
        Fact::Int(i)
    }
}

impl From<&str> for Fact {
    fn from(s: &str) -> Self {
        Fact::Str(s.into())
    }
}

/// What the projection knows about a lane, by dotted path (`verdict.sha`).
/// Predicates read these, never events or prose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactBundle(BTreeMap<String, Fact>);

impl FactBundle {
    /// Sets one fact, replacing any value the path had.
    pub fn set(&mut self, path: &str, fact: impl Into<Fact>) -> &mut Self {
        self.0.insert(path.into(), fact.into());
        self
    }

    /// The paths that were set, in order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The fact at `path`, [`Fact::Absent`] when it has none.
    pub fn get(&self, path: &str) -> &Fact {
        self.0.get(path).unwrap_or(&Fact::Absent)
    }
}
