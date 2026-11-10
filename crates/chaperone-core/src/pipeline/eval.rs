use super::{Fact, FactBundle, Literal, Operand, Phase, Pipeline, Pred, Route};

/// The fact `stamp_hash` compares the lane's stamp with. The evaluator
/// answers it from the loaded document, so no bundle can supply it.
const LOADED_HASH: &str = "loaded.hash";

/// Why a transition did not apply: the first gate that did not hold, and
/// every fact path that gate read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    pub gate: String,
    pub fact_paths: Vec<String>,
}

impl Denied {
    /// The denial for a lane with no stamp, or no row for its route
    /// (docs/cli-contract.md).
    pub fn unstamped() -> Denied {
        Denied {
            gate: "stamp_ok".into(),
            fact_paths: vec!["lane.pipeline_hash".into()],
        }
    }
}

impl Pipeline {
    /// Decides transition `transition` to `to` for a lane on `route`. Each
    /// required gate of that row is checked in order. A transition with no
    /// row for this route and target is denied as an unstamped lane is
    /// (docs/cli-contract.md), never passed. Whether the lane is in the row's
    /// `from` phase is the caller's to check.
    pub fn evaluate(
        &self,
        transition: &str,
        route: Route,
        to: Phase,
        facts: &FactBundle,
    ) -> Result<(), Denied> {
        let (gates, eval) = self.row(transition, route, to, facts)?;
        match gates.into_iter().find(|(_, passed)| !passed) {
            Some((gate, _)) => Err(eval.deny(&gate)),
            None => Ok(()),
        }
    }

    /// Each gate transition `transition` to `to` requires on `route`, in
    /// order, and whether it holds on `facts`. No row is denied as in
    /// [`Pipeline::evaluate`].
    pub fn gates(
        &self,
        transition: &str,
        route: Route,
        to: Phase,
        facts: &FactBundle,
    ) -> Result<Vec<(String, bool)>, Denied> {
        self.row(transition, route, to, facts)
            .map(|(gates, _)| gates)
    }

    fn row<'a>(
        &'a self,
        transition: &str,
        route: Route,
        to: Phase,
        facts: &'a FactBundle,
    ) -> Result<(Vec<(String, bool)>, Eval<'a>), Denied> {
        let Some(row) = self
            .transitions
            .iter()
            .find(|t| t.id == transition && t.to == to && t.routes.contains(&route))
        else {
            return Err(Denied::unstamped());
        };
        let eval = Eval {
            pipeline: self,
            facts,
            loaded: Fact::Str(self.hash.clone()),
        };
        let gates = row
            .requires
            .iter()
            .map(|gate| (gate.clone(), eval.gate(gate) == Some(true)))
            .collect();
        Ok((gates, eval))
    }
}

struct Eval<'a> {
    pipeline: &'a Pipeline,
    facts: &'a FactBundle,
    loaded: Fact,
}

impl Eval<'_> {
    fn fact(&self, path: &str) -> &Fact {
        if path == LOADED_HASH {
            &self.loaded
        } else {
            self.facts.get(path)
        }
    }

    /// The fact, or `None` when it is absent.
    fn val(&self, path: &str) -> Option<&Fact> {
        match self.fact(path) {
            Fact::Absent => None,
            fact => Some(fact),
        }
    }

    /// `Some(holds)`, or `None` when a fact the gate needs is absent. `None`
    /// denies wherever it lands, `not` included, so absence fails closed.
    fn gate(&self, id: &str) -> Option<bool> {
        Some(match &self.pipeline.gates[id].pred {
            Pred::Present { field } => self.val(field).is_some(),
            Pred::Eq { field, value } => {
                let fact = self.val(field)?;
                match value {
                    Operand::Literal(lit) => *fact == literal(lit),
                    Operand::Fact(other) => fact == self.val(other)?,
                }
            }
            Pred::Lte { field, value } => matches!(self.val(field)?, Fact::Int(n) if n <= value),
            Pred::Gt { field, value } => matches!(self.val(field)?, Fact::Int(n) if n > value),
            Pred::OneOf { field, values } => {
                let fact = self.val(field)?;
                values.iter().any(|lit| *fact == literal(lit))
            }
            Pred::ShaPrefixEq {
                left,
                right,
                min_len,
                max_len,
                both_ways,
            } => match (self.val(left)?, self.val(right)?) {
                (Fact::Str(left), Fact::Str(right)) => {
                    sha_prefix_eq(left, right, *min_len..=*max_len, *both_ways)
                }
                _ => false,
            },
            Pred::PlanCheckMatchesLane { reasons } => self.matches_lane("plan_check", reasons)?,
            Pred::QualityGateMatchesLane { reasons } => {
                self.matches_lane("quality_gate", reasons)?
            }
            // Vacuous when the later event has not happened (no PR yet).
            Pred::EventBefore { earlier, later } => match (self.fact(earlier), self.fact(later)) {
                (_, Fact::Absent) => true,
                (Fact::Time(a), Fact::Time(b)) => a < b,
                (Fact::Absent, _) => return None,
                _ => false,
            },
            // Absent wins over false in `all`, so `not(all(..))` never passes
            // on a missing fact, whatever the member order.
            Pred::All(of) => {
                let checks: Vec<_> = of.iter().map(|g| self.gate(g)).collect();
                if checks.contains(&None) {
                    return None;
                }
                !checks.contains(&Some(false))
            }
            Pred::Any(of) => {
                let checks: Vec<_> = of.iter().map(|g| self.gate(g)).collect();
                if checks.contains(&Some(true)) {
                    true
                } else if checks.contains(&None) {
                    return None;
                } else {
                    false
                }
            }
            Pred::Not(of) => !self.gate(of)?,
        })
    }

    /// A check that ran always matches. A skip matches only on a lane with no
    /// code and with an allowed reason.
    fn matches_lane(&self, check: &str, reasons: &[String]) -> Option<bool> {
        match self.val(&format!("{check}.outcome"))? {
            Fact::Str(o) if o == "ran" => Some(true),
            Fact::Str(o) if o == "skipped" => {
                let code = self.val("lane.changes_code")?;
                let reason = self.val(&format!("{check}.reason"))?;
                Some(
                    *code == Fact::Bool(false)
                        && matches!(reason, Fact::Str(r) if reasons.contains(r)),
                )
            }
            _ => Some(false),
        }
    }

    /// The denial for a gate that did not hold. An `all` names its first
    /// member that did not hold, so the denial is the specific gate.
    fn deny(&self, id: &str) -> Denied {
        if let Pred::All(of) = &self.pipeline.gates[id].pred {
            for g in of {
                if self.gate(g) != Some(true) {
                    return self.deny(g);
                }
            }
        }
        let mut fact_paths = Vec::new();
        self.reads(id, &mut fact_paths);
        Denied {
            gate: id.into(),
            fact_paths,
        }
    }

    fn reads(&self, id: &str, out: &mut Vec<String>) {
        match &self.pipeline.gates[id].pred {
            Pred::All(of) | Pred::Any(of) => {
                for g in of {
                    self.reads(g, out);
                }
            }
            Pred::Not(of) => self.reads(of, out),
            pred => {
                for path in pred.fact_paths() {
                    push(out, &path);
                }
            }
        }
    }
}

fn push(out: &mut Vec<String>, path: &str) {
    if !out.iter().any(|p| p == path) {
        out.push(path.into());
    }
}

fn literal(lit: &Literal) -> Fact {
    match lit {
        Literal::Bool(b) => Fact::Bool(*b),
        Literal::Int(i) => Fact::Int(*i),
        Literal::Str(s) => Fact::Str(s.clone()),
    }
}

/// `left` is a prefix of `right` (with `both_ways`, the shorter of the two
/// is a prefix of the longer), and that prefix is hex of a length in `len`.
pub fn sha_prefix_eq(
    left: &str,
    right: &str,
    len: std::ops::RangeInclusive<i64>,
    both_ways: bool,
) -> bool {
    let (short, head) = if both_ways && left.len() > right.len() {
        (right, left)
    } else {
        (left, right)
    };
    len.contains(&(short.len() as i64))
        && short.bytes().all(|b| b.is_ascii_hexdigit())
        && head.starts_with(short)
}
