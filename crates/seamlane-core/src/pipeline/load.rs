use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::json;
use toml::Value;

use super::hash::{canonical_string, semantic_hash};
use super::{
    EVENTS, Gate, Literal, Operand, Pipeline, PipelineError, Pred, REQUIRED_TRANSITIONS, Transition,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDoc {
    schema: i64,
    id: String,
    #[allow(dead_code)]
    published: Option<String>,
    #[allow(dead_code)]
    cohort: Option<RawCohort>,
    #[serde(default)]
    limits: BTreeMap<String, i64>,
    #[serde(default, rename = "gate")]
    gates: Vec<RawGate>,
    #[serde(default, rename = "transition")]
    transitions: Vec<Transition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCohort {
    #[allow(dead_code)]
    target_lanes: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawGate {
    id: String,
    #[serde(rename = "fn")]
    func: String,
    #[serde(default)]
    args: toml::Table,
}

pub(super) fn load(src: &str) -> Result<Pipeline, PipelineError> {
    let raw: RawDoc = toml::from_str(src)?;
    if raw.schema != 1 {
        return Err(PipelineError::Schema(raw.schema));
    }

    let mut gates = BTreeMap::new();
    for g in &raw.gates {
        let pred = parse_pred(g, &raw.limits)?;
        let gate = Gate {
            id: g.id.clone(),
            pred,
        };
        if gates.insert(g.id.clone(), gate).is_some() {
            return Err(PipelineError::DuplicateGate(g.id.clone()));
        }
    }
    for gate in gates.values() {
        for child in children(&gate.pred) {
            check_gate(&gates, &gate.id, child)?;
        }
    }
    check_acyclic(&gates)?;

    let mut transitions = raw.transitions;
    let mut rows = HashSet::new();
    for t in &mut transitions {
        if t.routes.is_empty() {
            return Err(PipelineError::NoRoutes(t.id.clone()));
        }
        t.routes.sort();
        for route in &t.routes {
            if !rows.insert((t.id.clone(), *route, t.to)) {
                return Err(PipelineError::DuplicateTransition {
                    id: t.id.clone(),
                    route: json!(route).as_str().unwrap_or_default().into(),
                    to: json!(t.to).as_str().unwrap_or_default().into(),
                });
            }
        }
        for gate in &t.requires {
            check_gate(&gates, &t.id, gate)?;
        }
    }
    for id in REQUIRED_TRANSITIONS {
        if !transitions.iter().any(|t| t.id == *id) {
            return Err(PipelineError::MissingTransition(id));
        }
    }

    let hash = semantic_hash(&hash_input(&raw.limits, raw.gates, &transitions));
    Ok(Pipeline {
        id: raw.id,
        hash,
        limits: raw.limits,
        gates,
        transitions,
    })
}

/// The hashed document: defaults filled in (`args = {}`, `requires = []`),
/// routes sorted, rows sorted by id and then by content.
fn hash_input(
    limits: &BTreeMap<String, i64>,
    mut gates: Vec<RawGate>,
    transitions: &[Transition],
) -> serde_json::Value {
    gates.sort_by(|a, b| a.id.cmp(&b.id));
    let mut rows: Vec<(String, serde_json::Value)> = transitions
        .iter()
        .map(|t| (t.id.clone(), json!(t)))
        .collect();
    rows.sort_by_key(|(id, row)| (id.clone(), canonical_string(row)));
    let rows: Vec<_> = rows.into_iter().map(|(_, row)| row).collect();
    json!({ "limits": limits, "gates": gates, "transitions": rows })
}

fn children(pred: &Pred) -> &[String] {
    match pred {
        Pred::All(of) | Pred::Any(of) => of,
        Pred::Not(of) => std::slice::from_ref(of),
        _ => &[],
    }
}

fn check_gate(gates: &BTreeMap<String, Gate>, from: &str, gate: &str) -> Result<(), PipelineError> {
    if gates.contains_key(gate) {
        Ok(())
    } else {
        Err(PipelineError::UnknownGate {
            from: from.into(),
            gate: gate.into(),
        })
    }
}

fn check_acyclic(gates: &BTreeMap<String, Gate>) -> Result<(), PipelineError> {
    // false: on the current path; true: finished.
    fn visit<'a>(
        id: &'a str,
        gates: &'a BTreeMap<String, Gate>,
        seen: &mut HashMap<&'a str, bool>,
    ) -> Result<(), PipelineError> {
        match seen.get(id) {
            Some(true) => return Ok(()),
            Some(false) => return Err(PipelineError::Cycle(id.into())),
            None => {}
        }
        seen.insert(id, false);
        for child in children(&gates[id].pred) {
            visit(child, gates, seen)?;
        }
        seen.insert(id, true);
        Ok(())
    }
    let mut seen = HashMap::new();
    for id in gates.keys() {
        visit(id, gates, &mut seen)?;
    }
    Ok(())
}

fn parse_pred(g: &RawGate, limits: &BTreeMap<String, i64>) -> Result<Pred, PipelineError> {
    let mut a = Args {
        gate: &g.id,
        table: g.args.clone(),
        limits,
    };
    let pred = match g.func.as_str() {
        "present" => Pred::Present {
            field: a.str("field")?,
        },
        "eq" => Pred::Eq {
            field: a.str("field")?,
            value: a.operand()?,
        },
        "lte" => Pred::Lte {
            field: a.str("field")?,
            value: a.int()?,
        },
        "gt" => Pred::Gt {
            field: a.str("field")?,
            value: a.int()?,
        },
        "sha_prefix_eq" => Pred::ShaPrefixEq {
            left: a.str("left")?,
            right: a.str("right")?,
            min_len: a.limit("min_limit")?,
            max_len: a.limit("max_limit")?,
        },
        "one_of" => Pred::OneOf {
            field: a.str("field")?,
            values: a
                .list("values")?
                .into_iter()
                .map(|v| a.literal(v))
                .collect::<Result<_, _>>()?,
        },
        "plan_check_matches_lane" => Pred::PlanCheckMatchesLane {
            reasons: a.strs("reasons")?,
        },
        "quality_gate_matches_lane" => Pred::QualityGateMatchesLane {
            reasons: a.strs("reasons")?,
        },
        "event_before" => Pred::EventBefore {
            earlier: a.event("earlier")?,
            later: a.event("later")?,
        },
        "all" => Pred::All(a.strs("of")?),
        "any" => Pred::Any(a.strs("of")?),
        "not" => Pred::Not(a.str("of")?),
        _ => {
            return Err(PipelineError::UnknownFn {
                gate: g.id.clone(),
                func: g.func.clone(),
            });
        }
    };
    a.done()?;
    Ok(pred)
}

/// Takes each argument out of the table, so leftovers are unknown arguments.
struct Args<'a> {
    gate: &'a str,
    table: toml::Table,
    limits: &'a BTreeMap<String, i64>,
}

impl Args<'_> {
    fn bad(&self, message: String) -> PipelineError {
        PipelineError::BadArgs {
            gate: self.gate.into(),
            message,
        }
    }

    fn take(&mut self, key: &str) -> Result<Value, PipelineError> {
        self.table
            .remove(key)
            .ok_or_else(|| self.bad(format!("missing argument {key}")))
    }

    fn str(&mut self, key: &str) -> Result<String, PipelineError> {
        match self.take(key)? {
            Value::String(s) => Ok(s),
            _ => Err(self.bad(format!("{key} must be a string"))),
        }
    }

    fn list(&mut self, key: &str) -> Result<Vec<Value>, PipelineError> {
        match self.take(key)? {
            Value::Array(items) if !items.is_empty() => Ok(items),
            _ => Err(self.bad(format!("{key} must be a non-empty list"))),
        }
    }

    fn strs(&mut self, key: &str) -> Result<Vec<String>, PipelineError> {
        self.list(key)?
            .into_iter()
            .map(|v| match v {
                Value::String(s) => Ok(s),
                _ => Err(self.bad(format!("{key} must list strings"))),
            })
            .collect()
    }

    fn literal(&self, value: Value) -> Result<Literal, PipelineError> {
        match value {
            Value::Boolean(b) => Ok(Literal::Bool(b)),
            Value::Integer(i) => Ok(Literal::Int(i)),
            Value::String(s) => Ok(Literal::Str(s)),
            _ => Err(self.bad("a value must be a bool, integer or string".into())),
        }
    }

    fn limit(&mut self, key: &str) -> Result<i64, PipelineError> {
        let name = self.str(key)?;
        self.limits
            .get(&name)
            .copied()
            .ok_or_else(|| PipelineError::UnknownLimit {
                gate: self.gate.into(),
                limit: name,
            })
    }

    fn event(&mut self, key: &str) -> Result<String, PipelineError> {
        let event = self.str(key)?;
        if EVENTS.contains(&event.as_str()) {
            Ok(event)
        } else {
            Err(PipelineError::UnknownEvent {
                gate: self.gate.into(),
                event,
            })
        }
    }

    /// Exactly one of `value`, `value_from` or `limit`.
    fn operand(&mut self) -> Result<Operand, PipelineError> {
        let given = ["value", "value_from", "limit"]
            .iter()
            .filter(|k| self.table.contains_key(**k))
            .count();
        if given != 1 {
            return Err(self.bad("give exactly one of value, value_from or limit".into()));
        }
        if let Some(v) = self.table.remove("value") {
            return Ok(Operand::Literal(self.literal(v)?));
        }
        if self.table.contains_key("value_from") {
            return Ok(Operand::Fact(self.str("value_from")?));
        }
        Ok(Operand::Literal(Literal::Int(self.limit("limit")?)))
    }

    /// Exactly one of an integer `value` or `limit`.
    fn int(&mut self) -> Result<i64, PipelineError> {
        match self.operand()? {
            Operand::Literal(Literal::Int(i)) => Ok(i),
            _ => Err(self.bad("value must be an integer or a limit".into())),
        }
    }

    fn done(self) -> Result<(), PipelineError> {
        match self.table.keys().next() {
            None => Ok(()),
            Some(key) => Err(self.bad(format!("unknown argument {key}"))),
        }
    }
}
