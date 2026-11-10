//! `check run plan`: one fresh headless session attacks the lane's plan
//! before any code exists, in the harness's checkout of the lane's commit,
//! given what the lane is for and the plan file, and answers in the
//! review's finding shape. It is a `plan_check` with the harness producer,
//! so the facts that read plan checks read it.

use std::path::Path;

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
/// the evidence store holds it (redacted, so a secret in the file is not
/// sent), that copy's hash and the merge base.
pub struct Given<'a> {
    pub goal: &'a str,
    pub plan: &'a str,
    pub plan_ref: &'a str,
    pub base: &'a str,
}

/// The plan written to `store` as it keeps it, redacted: that copy, not the
/// file, is what the reviewer is sent, so a secret in the plan stays here.
pub fn keep(store: &Evidence, text: &str) -> std::io::Result<Kept> {
    let kept = Kept::of(text);
    store.write(&kept)?;
    Ok(kept)
}

/// The plan reviewer's prompt: its check's prompt, what the change is for,
/// the plan, where it runs, then the harness's line saying the fenced text
/// is data.
pub fn prompt(p: &Planned<'_>, goal: &str, plan: &str, head: &str, base: &str) -> String {
    format!(
        "{}\n\n## What the change is for\n\n{}\n## The plan\n\n{}\n## The code\n\n\
         You are in a checkout of commit {head}, before the plan's code; its merge base is {base}.\n\n\
         {TRAILER}",
        p.check.text,
        fenced("", goal),
        fenced("", plan),
    )
}

/// The start of `check run plan`: `input` keyed for a plan check by the
/// pipeline `hash`, then the plan check that ran on the same tree and
/// pipeline with the same plan (`plan_ref`, its evidence hash) when there
/// is one, which replays and starts no session. Else a plan whose sessions
/// gave no answer `attempts` times there is refused: the owner decides.
pub fn replay(
    ledger: &Ledger,
    input: &mut RunInput,
    (hash, attempts): (&str, i64),
    plan_ref: &str,
) -> Checked<Option<Recorded>> {
    input.check = EVENT_CHECK.into();
    input.profile = Some(hash.into());
    if let Some(stored) = ledger.tree_run(input, "plan_ref", plan_ref)? {
        return Ok(Ok(Some(stored)));
    }
    let failed = ledger.no_answers(input, "plan_ref", plan_ref)?;
    if failed >= attempts {
        return Ok(Err(Refusal::InvalidInput(format!(
            "this plan has had {failed} plan sessions with no answer on the tree {} by this \
             pipeline, its limit {ATTEMPTS}; the owner can waive the plan check",
            input.tree
        ))));
    }
    Ok(Ok(None))
}

/// `check run plan` once [`replay`] answered none: the session in `dir`, a
/// checkout of the commit, on what it is `given`, then its usage row, then
/// one `plan_check`: `ran` with the findings (each cut as the review's), or
/// `skipped` with `reason` `no_answer`, never "no findings".
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
        plan_ref,
        base,
    } = *given;
    let prompt = prompt(p, goal, plan, &input.head, base);
    let parse = |out| review::findings("plan", out);
    let session = review::one(p, agents, dir, prompt, &review::schema(), store, parse)?;
    let ran = (session.lens, session.cli, &session.model, &session.run);
    if let Err(r) = review::usage(ledger, actor, input.work, "plan", ran)? {
        return Ok(Err(r));
    }
    input.fields = Map::from_iter([
        ("base".into(), json!(base)),
        ("plan_ref".into(), json!(plan_ref)),
        ("session".into(), session.session()),
    ]);
    match &session.outcome {
        Ok(found) => {
            let found = found.iter().map(|f| Finding {
                title: cut(&f.title, NOTE_CHARS),
                body: cut(&f.body, BODY_CHARS),
                ..f.clone()
            });
            input.findings = Some(Findings::List(found.collect()));
        }
        Err(_) => {
            input.fields.insert("reason".into(), json!("no_answer"));
        }
    }
    let key = ledger.tree_key(&input, plan_ref)?;
    ledger.finish_run(actor, &input, session.outcome.is_ok(), &key)
}
