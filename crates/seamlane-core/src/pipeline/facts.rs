use std::collections::BTreeMap;

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
    pub fn set(&mut self, path: &str, fact: impl Into<Fact>) -> &mut Self {
        self.0.insert(path.into(), fact.into());
        self
    }

    pub fn get(&self, path: &str) -> &Fact {
        self.0.get(path).unwrap_or(&Fact::Absent)
    }
}
