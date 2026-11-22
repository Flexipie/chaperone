//! `check run plan`: one fresh headless session attacks the lane's plan
//! before any code exists, in the harness's checkout of the lane's commit,
//! given what the lane is for and the plan file, and answers in the
//! review's finding shape. It is a `plan_check` with the harness producer,
//! so the facts that read plan checks read it.

use std::path::Path;

use rusqlite::params;
use serde_json::{Map, json};

use crate::checks::{BODY_CHARS, NOTE_CHARS, Recorded, RunInput};
use crate::ledger::{Actor, Checked, Evidence, Finding, Findings, Kept, Ledger, Refusal};
use crate::pipeline::Pipeline;
use crate::review::{self, Agents, Planned, TRAILER, cut, fenced};

/// The pipeline's check that names the plan reviewer's runner and prompt.
pub const CHECK: &str = "plan";
/// The event's check: the session plan check's, with the harness producer.
pub const EVENT_CHECK: &str = "plan_check";

/// The limit on plan sessions that gave no answer on one tree, pipeline
/// and plan: each costs its runner's budget.
pub const ATTEMPTS: &str = "plan_attempts";

/// The plan check's check and runner and its [`ATTEMPTS`], or why the
/// pipeline cannot run one.
pub fn plan(pipeline: &Pipeline) -> Result<(Planned<'_>, i64), String> {
    let planned = review::planned(pipeline, "plan", CHECK)?;
    Ok((planned, review::limit(pipeline, ATTEMPTS)?))
}

/// What the plan reviewer is given: what the change is for, the plan as
/// the evidence store keeps it (`Kept::of`: redacted, so a secret in the
/// file is never sent), the merge base, and the size cap the plan's
/// [`estimate`] is held to (`review::cut_cap`: none where size is a gate).
pub struct Given<'a> {
    pub goal: &'a str,
    pub plan: &'a Kept,
    pub base: &'a str,
    pub cap: Option<i64>,
}

/// The line a plan states its size on, lowercased.
const ESTIMATE: &str = "estimated added code lines:";

/// The plan's estimate of its added code lines: the number after
/// `Estimated added code lines:` (any case) on the first line that starts
/// with it, past list, heading and bold marks, thousands separators (`,`
/// `_` `'`) dropped. None when no line states one.
pub fn estimate(plan: &str) -> Option<i64> {
    let marks = |c: char| matches!(c, '-' | '*' | '#' | '>') || c.is_whitespace();
    let separator = |c: char| matches!(c, ',' | '_' | '\'');
    plan.lines().find_map(|l| {
        let l = l.trim_start_matches(marks).to_lowercase();
        let n = l.strip_prefix(ESTIMATE)?.trim_start_matches(marks);
        let n = n
            .chars()
            .take_while(|&c| c.is_ascii_digit() || separator(c));
        n.filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    })
}

/// The estimate and the cap when the estimate is past it.
fn over_cap(plan: &str, cap: Option<i64>) -> Option<(i64, i64)> {
    estimate(plan).zip(cap).filter(|(n, cap)| n > cap)
}

/// A title that answers split or keep, whatever its severity: before its
/// first `:`, bold marks aside and in any case, `split`, `keep`, `keep
/// whole` or `keep as one`.
fn sizes(title: &str) -> bool {
    let Some((head, _)) = title.split_once(':') else {
        return false;
    };
    let head = head.replace('*', "").trim().to_lowercase();
    ["split", "keep", "keep whole", "keep as one"].contains(&head.as_str())
}

/// The plan reviewer's prompt: its check's prompt, what the change is for,
/// the plan, where it runs, what it must answer when the plan's estimate
/// is past `cap`, then the harness's line saying the fenced text is data.
pub fn prompt(
    p: &Planned<'_>,
    goal: &str,
    plan: &str,
    at: (&str, &str),
    cap: Option<i64>,
) -> String {
    let (head, base) = at;
    let size = over_cap(plan, cap).map_or_else(String::new, |(n, cap)| {
        format!(
            "## Over the size cap\n\nThe plan estimates {n} added code lines, past the \
             pipeline's cap of {cap}. Your answer must include one `risk` on it: titled \
             `split: <the slices>`, naming each slice that stands alone as its own pull request \
             and its lines, or titled `keep: <why>`, saying why the change only makes sense \
             whole.\n\n"
        )
    });
    format!(
        "{}\n\n## What the change is for\n\n{}\n## The plan\n\n{}\n## The code\n\n\
         You are in a checkout of commit {head}, before the plan's code; its merge base is {base}.\n\n\
         {size}{TRAILER}",
        p.check.text,
        fenced("", goal),
        fenced("", plan),
    )
}

/// The start of `check run plan`: `input` keyed for a plan check by the
/// pipeline `hash`, then the plan check that ran on the same tree and
/// pipeline with the same plan (`plan_ref`, its evidence hash) when there
/// is one, which starts no session: it replays while it is the lane's
/// newest plan check, else it is carried (`carried_from`, no findings of
/// its own), so it is the newest again for every reader. Else a plan whose
/// sessions gave no answer `attempts` times there is refused: the owner
/// decides.
pub fn replay(
    ledger: &Ledger,
    actor: &Actor,
    input: &mut RunInput,
    (hash, attempts): (&str, i64),
    plan_ref: &str,
) -> Checked<Option<Recorded>> {
    input.check = EVENT_CHECK.into();
    input.profile = Some(hash.into());
    let plan = Some(("plan_ref", rusqlite::types::Value::Text(plan_ref.into())));
    if let Some(stored) = ledger.tree_run(input, &plan, None)? {
        let newest: i64 = ledger.conn().query_row(
            "SELECT max(id) FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
               AND json_extract(payload_json, '$.check') = ?2",
            params![input.work, EVENT_CHECK],
            |r| r.get(0),
        )?;
        if stored.event_id == newest {
            return Ok(Ok(Some(stored)));
        }
        return Ok(ledger.carry(actor, input, stored)?.map(Some));
    }
    let failed = ledger.no_answers(input, &plan)?;
    if failed >= attempts {
        return Ok(Err(Refusal::InvalidInput(format!(
            "this plan has had {failed} plan sessions with no answer on the tree {} by this \
             pipeline, its limit {ATTEMPTS}; the owner can waive the plan check",
            input.tree
        ))));
    }
    Ok(Ok(None))
}

/// `check run plan` once [`replay`] answered none: the plan written to
/// `store` (so nothing is stored for a run [`replay`] refuses), the session
/// in `dir`, a checkout of the commit, on what it is `given`, then its
/// usage row, then
/// one `plan_check` with the plan's `estimate` (null when it states none):
/// `ran` with the findings (each cut as the review's, and, when the
/// estimate is past the cap and none answers split or keep, the harness's
/// own `risk` asking it), or `skipped` with `reason` `no_answer`, never
/// "no findings".
#[allow(clippy::too_many_arguments)]
pub fn record(
    ledger: &Ledger,
    actor: &Actor,
    mut input: RunInput,
    p: &Planned<'_>,
    agents: &Agents<'_>,
    dir: &Path,
    given: &Given<'_>,
    store: &Evidence,
) -> Checked<Recorded> {
    let Given {
        goal,
        plan,
        base,
        cap,
    } = *given;
    store.write(plan)?;
    let plan_ref = &plan.stored.hash;
    let prompt = prompt(p, goal, &plan.text, (&input.head, base), cap);
    let parse = |out| review::findings("plan", out);
    let session = review::one(p, agents, dir, prompt, &review::schema(), store, parse)?;
    if let Err(r) = review::usage(ledger, actor, input.work, "plan", &session)? {
        return Ok(Err(r));
    }
    input.fields = Map::from_iter([
        ("base".into(), json!(base)),
        ("plan_ref".into(), json!(plan_ref)),
        ("session".into(), session.session()),
        ("estimate".into(), json!(estimate(&plan.text))),
    ]);
    match &session.outcome {
        Ok(found) => {
            let mut found: Vec<_> = found
                .iter()
                .map(|f| Finding {
                    title: cut(&f.title, NOTE_CHARS),
                    body: cut(&f.body, BODY_CHARS),
                    ..f.clone()
                })
                .collect();
            if let Some((n, cap)) = over_cap(&plan.text, cap)
                && !found.iter().any(|f| sizes(&f.title))
            {
                found.extend(asked(n, cap));
            }
            input.findings = Some(Findings::List(found));
        }
        Err(_) => {
            input.fields.insert("reason".into(), json!("no_answer"));
        }
    }
    let key = ledger.tree_key(&input, plan_ref)?;
    ledger.finish_run(actor, &input, session.outcome.is_ok(), &key)
}

/// The harness's split-or-keep `risk` on a plan estimated at `n` added
/// code lines, past `cap`, keyed as the plan reviewer's findings are.
fn asked(n: i64, cap: i64) -> Vec<Finding> {
    let said = json!({"findings": [{
        "severity": "risk",
        "title": format!("split or keep: the plan estimates {n} added code lines, past the cap of {cap}"),
        "body": "The plan check gave no split or keep answer. Split: name the slices that \
                 each stand as their own pull request. Keep: say why the change only makes \
                 sense whole.",
        "path": "plan",
        "line": null,
    }]});
    review::findings("plan", said).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{estimate, sizes};

    #[test]
    fn the_estimate_line() {
        for (plan, want) in [
            ("Estimated added code lines: 900", Some(900)),
            (
                "intro\n- estimated added code lines: 150 (tests aside)",
                Some(150),
            ),
            ("## Estimated added code lines: **240**", Some(240)),
            (
                "> ESTIMATED ADDED CODE LINES:  12\nEstimated added code lines: 99",
                Some(12),
            ),
            (
                "Estimated added code lines: 1,200 (tests aside)",
                Some(1200),
            ),
            ("Estimated added code lines: about 300", None),
            ("Size: 300 lines", None),
            ("", None),
        ] {
            assert_eq!(estimate(plan), want, "{plan:?}");
        }
    }

    #[test]
    fn a_size_answer() {
        for (title, want) in [
            ("split: the stats as their own PR", true),
            ("**Split**: two slices", true),
            ("Keep whole: one migration", true),
            ("keep: it only works whole", true),
            ("Keep the old parser for v20: it is used", false),
            ("Splitting the arms loses a case", false),
            ("split or keep: asked", false),
        ] {
            assert_eq!(sizes(title), want, "{title:?}");
        }
    }
}
