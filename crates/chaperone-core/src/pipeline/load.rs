use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::json;
use toml::Value;

use sha2::{Digest, Sha256};

use super::hash::{canonical_string, semantic_hash};
use super::{
    Check, Cli, EVENTS, FACTS, Gate, Literal, Operand, Pipeline, PipelineError, Pred,
    REQUIRED_TRANSITIONS, Runner, Transition,
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
    #[serde(default)]
    runners: BTreeMap<String, RawRunner>,
    #[serde(default)]
    checks: BTreeMap<String, RawCheck>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRunner {
    cli: Cli,
    model: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    budget_limit: String,
    timeout_limit: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheck {
    runner: String,
    prompt: String,
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

pub(super) fn load(src: &str, prompts: &[(&str, &str)]) -> Result<Pipeline, PipelineError> {
    let raw: RawDoc = toml::from_str(src)?;
    let v2 = match raw.schema {
        1 => false,
        2 => true,
        other => return Err(PipelineError::Schema(other)),
    };
    // Schema 1 hashes cover neither table, so there they would be unstamped.
    if !v2 && !raw.runners.is_empty() {
        return Err(PipelineError::NeedsSchema2("runners"));
    }
    if !v2 && !raw.checks.is_empty() {
        return Err(PipelineError::NeedsSchema2("checks"));
    }
    let runners = runners(raw.runners, &raw.limits)?;
    let checks = checks(raw.checks, &runners, prompts)?;

    let mut gates = BTreeMap::new();
    for g in &raw.gates {
        let pred = parse_pred(g, &raw.limits)?;
        if v2 {
            check_facts(&g.id, &pred)?;
        }
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

    let mut hashed = hash_input(&raw.limits, raw.gates, &transitions);
    if v2 {
        // The schema too: the same rows under schema 1 are checked less.
        hashed["schema"] = json!(2);
        hashed["runners"] = json!(runners);
        hashed["checks"] = json!(checks);
    }
    Ok(Pipeline {
        id: raw.id,
        hash: semantic_hash(&hashed),
        limits: raw.limits,
        gates,
        transitions,
        runners,
        checks,
    })
}

/// Each runner with its two limits resolved, both at least 1.
fn runners(
    raw: BTreeMap<String, RawRunner>,
    limits: &BTreeMap<String, i64>,
) -> Result<BTreeMap<String, Runner>, PipelineError> {
    let mut runners = BTreeMap::new();
    for (name, r) in raw {
        let bad = |message: String| PipelineError::BadRunner {
            runner: name.clone(),
            message,
        };
        let limit = |limit: &str| match limits.get(limit) {
            Some(n) if *n >= 1 => Ok(*n),
            Some(n) => Err(bad(format!("limit {limit} is {n}, not at least 1"))),
            None => Err(bad(format!("unknown limit {limit}"))),
        };
        // Their order and repeats are not behaviour, so not the hash's.
        let mut tools = r.tools;
        tools.sort();
        tools.dedup();
        let runner = Runner {
            cli: r.cli,
            model: r.model,
            tools,
            budget_cents: limit(&r.budget_limit)?,
            timeout_secs: limit(&r.timeout_limit)?,
        };
        runners.insert(name, runner);
    }
    Ok(runners)
}

/// Each check with its prompt's digest; its runner and prompt must exist.
fn checks(
    raw: BTreeMap<String, RawCheck>,
    runners: &BTreeMap<String, Runner>,
    prompts: &[(&str, &str)],
) -> Result<BTreeMap<String, Check>, PipelineError> {
    let mut checks = BTreeMap::new();
    for (name, c) in raw {
        if !runners.contains_key(&c.runner) {
            return Err(PipelineError::UnknownRunner {
                check: name,
                runner: c.runner,
            });
        }
        let Some((_, text)) = prompts.iter().find(|(file, _)| *file == c.prompt) else {
            return Err(PipelineError::UnknownPrompt {
                check: name,
                prompt: c.prompt,
            });
        };
        let check = Check {
            runner: c.runner,
            prompt_sha256: format!("{:x}", Sha256::digest(text)),
            prompt: c.prompt,
            text: text.to_string(),
        };
        checks.insert(name, check);
    }
    Ok(checks)
}

/// Schema 2: every fact path `pred` reads is one the projection sets.
fn check_facts(gate: &str, pred: &Pred) -> Result<(), PipelineError> {
    match pred
        .fact_paths()
        .into_iter()
        .find(|p| !FACTS.contains(&p.as_str()))
    {
        None => Ok(()),
        Some(fact) => Err(PipelineError::UnknownFact {
            gate: gate.into(),
            fact,
        }),
    }
}

/// The hashed document: defaults filled in (`args = {}`, `requires = []`),
/// routes sorted, rows sorted by id and then by content.
fn hash_input(
    limits: &BTreeMap<String, i64>,
    mut gates: Vec<RawGate>,
    transitions: &[Transition],
) -> serde_json::Value {
    gates.sort_by(|a, b| a.id.cmp(&b.id));
    let mut rows: Vec<&Transition> = transitions.iter().collect();
    rows.sort_by_cached_key(|t| (&t.id, canonical_string(&json!(t))));
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
        "sha_prefix_eq" | "sha_same_commit" => {
            let (left, right) = (a.str("left")?, a.str("right")?);
            let (min_len, max_len) = (a.limit("min_limit")?, a.limit("max_limit")?);
            // A floor under 1 lets an empty sha match any head.
            if min_len < 1 || min_len > max_len {
                return Err(a.bad(format!(
                    "sha lengths {min_len}..{max_len} are not 1 <= min <= max"
                )));
            }
            Pred::ShaPrefixEq {
                left,
                right,
                min_len,
                max_len,
                both_ways: g.func == "sha_same_commit",
            }
        }
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

    /// The time fact path of a known event.
    fn event(&mut self, key: &str) -> Result<String, PipelineError> {
        let event = self.str(key)?;
        match EVENTS.iter().find(|(name, _)| *name == event) {
            Some((_, path)) => Ok((*path).into()),
            None => Err(PipelineError::UnknownEvent {
                gate: self.gate.into(),
                event,
            }),
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
