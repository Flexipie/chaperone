//! `check run verdict`: one fresh headless session decides whether the
//! lane's commit should merge, in the harness's checkout of it, from facts
//! the harness gathered or holds: the change (as the review shows it) and
//! the harness's own results on it. Of what the lane wrote it is shown only
//! what the change is for (as the intent lens is) and a session's smoke
//! claim, labelled as one; never the lane's summary, merge case or what to
//! look at.
//!
//! A decision stands for the tree it judged: the same tree and pipeline
//! replay it, yes or no, and start no session, so a verdict cannot be run
//! again until it says yes. A NO is reopened only by a waive or a
//! refutation recorded after it, or by changing the code. It is a
//! `merge_verdict` with the harness producer, so the facts and gates that
//! read verdicts read it.

use std::io;
use std::path::Path;

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::checks::{BODY_CHARS, NOTE_CHARS, Recorded, RunInput, session_of};
use crate::ledger::{Actor, Checked, Evidence, Ledger, LedgerError, Payload, Refusal};
use crate::pipeline::{Pipeline, canonical_string};
use crate::review::{self, Agents, Facts, Planned, TRAILER, cut, fenced};

/// The pipeline's check that names the verdict's runner and prompt.
pub const CHECK: &str = "verdict";
/// The event's check: the session verdict's, with the harness producer.
pub const EVENT_CHECK: &str = "merge_verdict";
/// Of `proven` and `not_proven`, the items kept.
const ITEMS: usize = 20;
/// Of each file a smoke claim attached, the end shown.
const CLAIM_BYTES: usize = 4 * 1024;

/// The limit on verdict sessions that gave no answer on one tree,
/// pipeline and set of facts: each costs its runner's budget.
pub const ATTEMPTS: &str = "verdict_attempts";

/// The verdict's check and runner and its [`ATTEMPTS`], or why the
/// pipeline cannot run one.
pub fn plan(pipeline: &Pipeline) -> Result<(Planned<'_>, i64), String> {
    let planned = review::planned(pipeline, "verdict", CHECK)?;
    Ok((planned, review::limit(pipeline, ATTEMPTS)?))
}

/// The harness's own results on one commit, as the verdict is shown them.
pub struct Harness {
    /// The key fields of the newest harness `diff`, `tests` and complete
    /// `review` on the commit, and the lane's findings that still block.
    /// Nothing in it names the commit, so the same results on an amended
    /// commit read the same.
    pub shown: Value,
    /// The sha256 of `shown`'s canonical JSON, less the smoke claim: kept
    /// on the event as information, never a reason for a new session.
    pub sha256: String,
    /// The ids of the events it was read from.
    pub read: Value,
}

/// What the harness holds on lane `work`'s commit `head`, or what it still
/// lacks there: a verdict runs only on a clean commit, with a `diff`, a
/// newest `tests` that passed and a `complete` review (a carried one
/// counts), and when the base profile declares a smoke (`smoke_declared`)
/// a newest harness smoke that passed; when it does not, no harness smoke
/// is read. So a NO cannot be caused by running the verdict early.
/// Blocking findings do not stop it: they are shown. The smoke is shown as [`smoke`] reads it; the lane's claim is
/// left out of the digest, so a new claim cannot buy a new session.
pub fn harness(
    ledger: &Ledger,
    store: &Evidence,
    work: i64,
    head: &str,
    smoke_declared: bool,
) -> Result<Result<Harness, &'static str>, LedgerError> {
    let Some(diff) = newest(ledger, work, "diff", head, false)? else {
        return Ok(Err("diff"));
    };
    let passed = |p: &Payload| p.rest.get("result") == Some(&json!("pass"));
    let Some(tests) = newest(ledger, work, "tests", head, false)? else {
        return Ok(Err("tests"));
    };
    if !passed(&tests.1) {
        return Ok(Err("tests that pass (the newest failed)"));
    }
    let Some(review) = newest(ledger, work, "review", head, true)? else {
        return Ok(Err("complete review"));
    };
    if smoke_declared {
        match newest(ledger, work, "smoke", head, false)? {
            None => return Ok(Err("smoke (the base profile declares [smoke])")),
            Some((_, p)) if !passed(&p) => {
                return Ok(Err("smoke that passes (the newest failed)"));
            }
            Some(_) => {}
        }
    }
    let pick = |p: &Payload, keys: &[&str]| -> Value {
        let got = keys
            .iter()
            .map(|k| (k.to_string(), p.rest.get(*k).cloned()));
        Value::Object(got.map(|(k, v)| (k, v.unwrap_or(Value::Null))).collect())
    };
    // Once each. A disputed finding reads open: the dispute is the lane's
    // word, not a harness fact, so it neither steers nor buys a session.
    let mut blocking: Vec<_> = ledger
        .blocking_findings(work)?
        .into_iter()
        .map(|f| {
            let status = if f.status == "disputed" {
                "open".into()
            } else {
                f.status
            };
            (f.finding_key, f.severity, status, f.title)
        })
        .collect();
    blocking.sort();
    blocking.dedup();
    let blocking = blocking.into_iter().map(|(key, severity, status, title)| {
        json!({"key": key, "severity": severity, "status": status, "title": title})
    });
    let mut shown = json!({
        "diff": pick(&diff.1, &["files", "added", "removed", "code_lines", "changes_code", "uncounted_files"]),
        "tests": ran(&tests.1),
        // From the lens rows: a review carried to an amend has no
        // findings of its own.
        "review": {"complete": true, "findings": found(&review.1)},
        "blocking_findings": blocking.collect::<Vec<_>>(),
        "smoke": smoke(ledger, store, work, head)?,
    });
    if !smoke_declared {
        shown["smoke"]["harness"] = Value::Null;
    }
    shown["smoke"]["declared"] = json!(smoke_declared);
    let mut hashed = shown.clone();
    hashed["smoke"]["lane_claim"] = Value::Null;
    Ok(Ok(Harness {
        sha256: format!("{:x}", Sha256::digest(canonical_string(&hashed))),
        read: json!({"diff": diff.0, "tests": tests.0, "review": review.0}),
        shown,
    }))
}

/// The newest harness `check` on lane `work`'s commit `head`, a complete
/// one when `complete`.
fn newest(
    ledger: &Ledger,
    work: i64,
    check: &str,
    head: &str,
    complete: bool,
) -> Result<Option<(i64, Payload)>, LedgerError> {
    let row: Option<(i64, String)> = ledger
        .conn()
        .query_row(
            "SELECT id, payload_json FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
               AND json_extract(payload_json, '$.check') = ?2
               AND json_extract(payload_json, '$.head_sha') = ?3
               AND json_extract(payload_json, '$.producer.kind') = 'harness'
               AND (NOT ?4 OR json_extract(payload_json, '$.complete') IS 1)
             ORDER BY id DESC LIMIT 1",
            params![work, check, head, complete],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(id, p)| Ok((id, serde_json::from_str(&p).map_err(io::Error::other)?)))
        .transpose()
}

/// The findings a review's lenses raised, by their rows.
fn found(review: &Payload) -> u64 {
    let rows = review.rest.get("lenses").and_then(Value::as_array);
    rows.into_iter()
        .flatten()
        .filter_map(|r| r["findings"].as_u64())
        .sum()
}

/// What a `tests` or `smoke` run did, without `ms`, which differs on
/// every run of the same commands.
fn ran(p: &Payload) -> Value {
    let commands = p.rest.get("commands").and_then(Value::as_array);
    let commands = commands.into_iter().flatten();
    let commands = commands.map(|c| json!({"argv": c["argv"], "exit": c["exit"]}));
    let field = |k: &str| p.rest.get(k).cloned().unwrap_or(Value::Null);
    json!({"result": field("result"), "timed_out": field("timed_out"), "commands": commands.collect::<Vec<_>>()})
}

/// The newest smoke on lane `work`'s commit `head`, as the verdict and the
/// intent lens are shown it: `harness`, what `check run smoke` ran, and
/// `lane_claim`, what a session recorded with `check record --check smoke`
/// (on `head` or a short form of it) and the id and end of each evidence
/// file it attached, read from `store`; null for none.
pub fn smoke(
    ledger: &Ledger,
    store: &Evidence,
    work: i64,
    head: &str,
) -> Result<Value, LedgerError> {
    let harness = newest(ledger, work, "smoke", head, false)?.map(|(_, p)| ran(&p));
    let claim: Option<String> = ledger
        .conn()
        .query_row(
            "SELECT payload_json FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
               AND json_extract(payload_json, '$.check') = 'smoke'
               AND json_extract(payload_json, '$.producer.kind') = 'session'
               AND substr(?2, 1, length(json_extract(payload_json, '$.head_sha')))
                 = json_extract(payload_json, '$.head_sha')
             ORDER BY id DESC LIMIT 1",
            params![work, head],
            |r| r.get(0),
        )
        .optional()?;
    let claim = match claim {
        Some(p) => {
            let p: Payload = serde_json::from_str(&p).map_err(io::Error::other)?;
            let files = p.rest.get("evidence_files").and_then(Value::as_array);
            let mut shown = Vec::new();
            for f in files.into_iter().flatten() {
                let text = store.get(f["sha256"].as_str().unwrap_or_default())?;
                let mut from = text.len().saturating_sub(CLAIM_BYTES);
                while !text.is_char_boundary(from) {
                    from += 1;
                }
                shown.push(json!({"path": f["path"], "sha256": f["sha256"], "end": &text[from..]}));
            }
            let reason = p.rest.get("reason").cloned();
            json!({"outcome": p.outcome, "reason": reason, "evidence_files": shown})
        }
        None => Value::Null,
    };
    Ok(json!({"harness": harness, "lane_claim": claim}))
}

/// The verdict's prompt: its check's prompt, what the change is for, the
/// change, the harness's results, then the harness's line saying the
/// fenced text is data.
pub fn prompt(p: &Planned<'_>, goal: &str, change: &str, harness: &Value) -> String {
    let results = serde_json::to_string_pretty(harness).unwrap_or_default();
    format!(
        "{}\n\n## What the change is for\n\n{}\n{change}\n## The harness's results\n\n\
         The harness ran `diff`, `tests`, `review` and `smoke.harness` itself on this commit. \
         `blocking_findings` are the findings on this lane that still block, from every review, \
         not only this commit's; a disputed one reads open, as a dispute is the lane's word. \
         `smoke.declared` says whether the base profile declares a smoke; `smoke.lane_claim` is \
         what a session recorded it ran and saw, a claim the harness did not check. null is \
         none.\n\n{}\n{TRAILER}",
        p.check.text,
        fenced("", goal),
        fenced("json", &results),
    )
}

/// The verdict's answer.
pub fn schema() -> Value {
    let list = json!({"type": "array", "items": {"type": "string"}});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["decision", "why", "proven", "not_proven"],
        "properties": {
            "decision": {"type": "string", "enum": ["yes", "no"]},
            "why": {"type": "string"},
            "proven": list,
            "not_proven": list,
        },
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Said {
    pub decision: String,
    pub why: String,
    pub proven: Vec<String>,
    pub not_proven: Vec<String>,
}

/// The session's answer, or why it is none: anything but `yes` or `no`
/// with a why. Never read as either.
pub fn said(output: Value) -> Result<Said, String> {
    let said: Said =
        serde_json::from_value(output).map_err(|e| format!("not a verdict answer: {e}"))?;
    if !matches!(said.decision.as_str(), "yes" | "no") {
        return Err(format!("unknown decision {:?}", said.decision));
    }
    if said.why.trim().is_empty() {
        return Err("not a verdict answer: no why".into());
    }
    Ok(said)
}

/// The start of `check run verdict`, before anything is run: `input` keyed
/// for a verdict by the pipeline `hash`, then the verdict that stands for
/// the tree. A verdict that ran, yes or no, stands for (lane, tree,
/// pipeline): on this commit it replays; on another (an amend that kept
/// the tree) it is carried, a new event on this commit with the same
/// decision, this commit's `facts` and `carried_from`, so a gate comparing
/// the verdict's commit with the head reads this one, and no session
/// starts. What the harness shows the verdict does not reopen it: the
/// lane's clone can move the base and its profile. A NO is reopened only
/// by [`reopened`]; then, or with no verdict yet, a tree whose sessions
/// gave no answer `attempts` times is refused: the owner decides.
pub fn replay(
    ledger: &Ledger,
    actor: &Actor,
    input: &mut RunInput,
    (hash, attempts): (&str, i64),
    harness: &Harness,
) -> Checked<Option<Recorded>> {
    input.check = EVENT_CHECK.into();
    input.profile = Some(hash.into());
    if let Some((newest, here)) = ledger.tree_stand(input, &None)? {
        let no = newest.payload.rest.get("decision") == Some(&json!("no"));
        if !(no && reopened(ledger, input.work, &newest)?) {
            if let Some(here) = here {
                return Ok(Ok(Some(here)));
            }
            input.fields = Map::from_iter([("facts".into(), harness.read.clone())]);
            return Ok(ledger.carry(actor, input, newest)?.map(Some));
        }
    }
    let failed = ledger.no_answers(input, &None)?;
    if failed >= attempts {
        return Ok(Err(Refusal::InvalidInput(format!(
            "the tree {} has had {failed} verdict sessions with no answer by this pipeline, its \
             limit {ATTEMPTS}; the owner decides whether it merges",
            input.tree
        ))));
    }
    Ok(Ok(None))
}

/// Whether a NO, `newest` on its tree, is reopened: a finding of the lane
/// raised before the session that decided it was waived or refuted by an
/// event after `newest`. Under the move table a session that has held the
/// lane makes neither move, and a finding raised later (another tree's
/// review) does not count, so the lane cannot reopen its own NO.
pub fn reopened(ledger: &Ledger, work: i64, newest: &Recorded) -> Result<bool, LedgerError> {
    let found: i64 = ledger.conn().query_row(
        "SELECT count(*) FROM events WHERE lane_id = ?1 AND kind = 'finding.updated' AND id > ?2
           AND json_extract(payload_json, '$.status') IN ('waived', 'refuted')
           AND json_extract(payload_json, '$.check_event_id') < ?3",
        params![work, newest.event_id, session_of(newest)],
        |r| r.get(0),
    )?;
    Ok(found > 0)
}

/// `check run verdict` once [`replay`] answered none: the session in `dir`,
/// a checkout of the commit, then its usage row, then one `merge_verdict`:
/// `ran` with the decision, or `skipped` with `reason` `no_answer` when the
/// session errored, stopped or answered outside the schema, which never
/// reads as a decision. Model text is cut: `why` to 600 characters, each
/// of at most 20 `proven` and `not_proven` items to 500.
#[allow(clippy::too_many_arguments)]
pub fn record(
    ledger: &Ledger,
    actor: &Actor,
    mut input: RunInput,
    p: &Planned<'_>,
    agents: &Agents<'_>,
    dir: &Path,
    facts: &Facts,
    harness: &Harness,
    store: &Evidence,
) -> Checked<Recorded> {
    let (change, diff_cut) = review::change(facts);
    let prompt = prompt(p, &facts.goal, &change, &harness.shown);
    let session = review::one(p, agents, dir, prompt, &schema(), store, said)?;
    let ran = (session.lens, session.cli, &session.model, &session.run);
    if let Err(r) = review::usage(ledger, actor, input.work, "verdict", ran)? {
        return Ok(Err(r));
    }
    input.fields = Map::from_iter([
        ("base".into(), json!(facts.base)),
        ("diff_cut".into(), json!(diff_cut)),
        ("facts".into(), harness.read.clone()),
        ("facts_sha256".into(), json!(harness.sha256)),
        (
            "smoke_declared".into(),
            harness.shown["smoke"]["declared"].clone(),
        ),
        ("session".into(), session.session()),
    ]);
    let items = |l: &[String]| -> Value {
        json!(
            l.iter()
                .take(ITEMS)
                .map(|i| cut(i, NOTE_CHARS))
                .collect::<Vec<_>>()
        )
    };
    match &session.outcome {
        Ok(said) => input.fields.extend([
            ("decision".into(), json!(said.decision)),
            ("why".into(), json!(cut(&said.why, BODY_CHARS))),
            ("proven".into(), items(&said.proven)),
            ("not_proven".into(), items(&said.not_proven)),
        ]),
        Err(_) => {
            input.fields.insert("reason".into(), json!("no_answer"));
        }
    }
    let key = ledger.tree_key(&input, &harness.sha256)?;
    ledger.finish_run(actor, &input, session.outcome.is_ok(), &key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::CheckInput;
    use crate::ledger::{Findings, Refusal};
    use crate::work::{ClaimInput, CreateInput};

    const TREE: &str = "def5678";

    /// A ledger with lane 1 claimed by [`me`].
    fn lane() -> (tempfile::TempDir, Ledger) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let input = CreateInput {
            title: "t".into(),
            source: Some("manual".into()),
            kind: "code".into(),
            pr_flow: "pr".into(),
            ..CreateInput::default()
        };
        let item = ledger.work_create(&me(), &input, None, false);
        item.unwrap().unwrap();
        let claim = ledger.work_claim(&me(), 1, &ClaimInput::default(), None, false);
        claim.unwrap().unwrap();
        (dir, ledger)
    }

    /// The lane's own session.
    fn me() -> Actor {
        Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        }
    }

    fn run(check: &str, head: &str, fields: Value) -> RunInput {
        RunInput {
            work: 1,
            check: check.into(),
            head: head.into(),
            tree: TREE.into(),
            profile: Some("p".into()),
            fields: fields.as_object().cloned().unwrap_or_default(),
            ..RunInput::default()
        }
    }

    /// The harness's `check` on `head`, as `check run` writes it.
    fn measured(ledger: &Ledger, check: &str, head: &str, fields: Value) {
        let mut input = run(check, head, fields);
        if check == "review" {
            let f = serde_json::from_value(json!({"finding_key": "k", "severity": "blocker",
                "title": "T", "body": "b"}));
            input.findings = Some(Findings::List(vec![f.unwrap()]));
        }
        ledger.check_run(&me(), &input).unwrap().unwrap();
    }

    /// Diff, tests and a complete review on `head`.
    fn all(ledger: &Ledger, head: &str) {
        measured(ledger, "diff", head, json!({"files": 1, "code_lines": 4}));
        let t = json!({"result": "pass", "commands": [{"argv": ["t"], "exit": 0, "ms": 3}]});
        measured(ledger, "tests", head, t);
        let review = json!({"complete": true, "lenses": [{"lens": "correctness", "findings": 1}]});
        measured(ledger, "review", head, review);
    }

    /// The lane's own session may write the harness's `merge_verdict` (it
    /// starts `check run verdict`), never a session one.
    #[test]
    fn the_harness_verdict_is_the_binarys_whoever_started_it() {
        let (_dir, ledger) = lane();
        let said = CheckInput {
            work: 1,
            check: EVENT_CHECK.into(),
            head: "abc1234".into(),
            outcome: "ran".into(),
            decision: Some("yes".into()),
            ..CheckInput::default()
        };
        let Err(Refusal::InvalidInput(why)) = ledger.check_record(&me(), &said).unwrap() else {
            unreachable!("a session verdict from the lane's session is refused");
        };
        assert!(why.starts_with("session s has held lane 1"), "{why}");
        let input = run(EVENT_CHECK, "abc1234", json!({"decision": "yes"}));
        let written = ledger
            .finish_run(&me(), &input, true, "k")
            .unwrap()
            .unwrap();
        assert_eq!(written.payload.rest["producer"]["kind"], "harness");
    }

    /// A verdict needs the commit's diff, tests and a complete review; the
    /// facts never name the commit, so an amend's same results (its review
    /// raising the same finding again) read the same.
    #[test]
    fn the_facts_need_diff_tests_and_a_complete_review() {
        let (dir, ledger) = lane();
        let store = Evidence::beside(&dir.path().join("ledger.db"));
        let lacks = |head: &str| harness(&ledger, &store, 1, head, false).unwrap().err();
        assert_eq!(lacks("abc1234"), Some("diff"));
        measured(
            &ledger,
            "diff",
            "abc1234",
            json!({"files": 1, "code_lines": 4}),
        );
        assert_eq!(lacks("abc1234"), Some("tests"));
        measured(&ledger, "tests", "abc1234", json!({"result": "fail"}));
        assert_eq!(
            lacks("abc1234"),
            Some("tests that pass (the newest failed)")
        );
        let pass = json!({"result": "pass", "commands": [{"argv": ["t"], "exit": 0, "ms": 3}]});
        measured(&ledger, "tests", "abc1234", pass);
        assert_eq!(lacks("abc1234"), Some("complete review"));
        measured(&ledger, "review", "abc1234", json!({"complete": false}));
        assert_eq!(lacks("abc1234"), Some("complete review"));
        all(&ledger, "abc1234");
        let first = harness(&ledger, &store, 1, "abc1234", false)
            .unwrap()
            .unwrap();
        let want = json!({
            "diff": {"files": 1, "added": null, "removed": null, "code_lines": 4,
                     "changes_code": null, "uncounted_files": null},
            "tests": {"result": "pass", "timed_out": null, "commands": [{"argv": ["t"], "exit": 0}]},
            "review": {"complete": true, "findings": 1},
            "blocking_findings": [{"key": "k", "severity": "blocker", "status": "open", "title": "T"}],
            "smoke": {"harness": null, "lane_claim": null, "declared": false},
        });
        assert_eq!(first.shown, want);
        assert_eq!(first.read, json!({"diff": 3, "tests": 5, "review": 7}));
        all(&ledger, "fed4321");
        let amend = harness(&ledger, &store, 1, "fed4321", false)
            .unwrap()
            .unwrap();
        assert_eq!(
            (&amend.sha256, amend.read["diff"].clone()),
            (&first.sha256, json!(8))
        );
        // The lane's dispute is its word: the facts read the finding open.
        let dispute = crate::checks::FindingMove {
            check_event_id: 7,
            finding_key: "k".into(),
            status: "disputed".into(),
            reason: Some("it is fine".into()),
            head_sha: None,
        };
        ledger
            .finding_update(&me(), &dispute, false)
            .unwrap()
            .unwrap();
        let disputed = harness(&ledger, &store, 1, "fed4321", false)
            .unwrap()
            .unwrap();
        assert_eq!(disputed.sha256, first.sha256);
        assert_eq!(disputed.shown["blocking_findings"][0]["status"], "open");
    }

    /// A verdict that ran stands for (lane, tree, pipeline): it replays on
    /// its commit and is carried once to an amend, whatever the facts show
    /// now. A NO is reopened only by a waive or refutation, after it, of a
    /// finding raised before it; a YES never is. Sessions that gave no
    /// answer count per tree, up to the limit.
    #[test]
    fn a_verdict_stands_for_its_tree() {
        let (dir, ledger) = lane();
        let store = Evidence::beside(&dir.path().join("ledger.db"));
        all(&ledger, "abc1234");
        let facts = harness(&ledger, &store, 1, "abc1234", false)
            .unwrap()
            .unwrap();
        let fields = |f: Value| f.as_object().cloned().unwrap_or_default();
        let replay_on = |head: &str, profile: &str, attempts: i64, facts: &Harness| {
            let mut input = run("", head, json!({}));
            let got = replay(&ledger, &me(), &mut input, (profile, attempts), facts);
            (input, got.unwrap())
        };
        let (mut input, none) = replay_on("abc1234", "p", 2, &facts);
        assert!(none.unwrap().is_none());
        assert_eq!(input.check, EVENT_CHECK);
        input.fields = fields(json!({"reason": "no_answer"}));
        let k = ledger.tree_key(&input, &facts.sha256).unwrap();
        ledger
            .finish_run(&me(), &input, false, &k)
            .unwrap()
            .unwrap();
        assert!(replay_on("abc1234", "p", 2, &facts).1.unwrap().is_none());
        let Err(Refusal::InvalidInput(why)) = replay_on("fed4321", "p", 1, &facts).1 else {
            unreachable!("past verdict_attempts, on any commit of the tree, is refused");
        };
        assert!(
            why.contains("its limit verdict_attempts; the owner decides"),
            "{why}"
        );
        input.fields = fields(json!({"decision": "no", "facts_sha256": facts.sha256}));
        let k = ledger.tree_key(&input, &facts.sha256).unwrap();
        let no = ledger.finish_run(&me(), &input, true, &k).unwrap().unwrap();

        // Other facts (a moved base, another profile) change nothing.
        let other = Harness {
            sha256: "0".repeat(64),
            shown: Value::Null,
            read: facts.read.clone(),
        };
        let again = replay_on("abc1234", "p", 2, &other).1.unwrap().unwrap();
        assert_eq!((again.replayed, again.event_id), (true, no.event_id));
        let carried = replay_on("fed4321", "p", 2, &facts).1.unwrap().unwrap();
        let p = &carried.payload;
        assert_eq!(
            (p.head_sha.as_deref(), &p.rest["decision"]),
            (Some("fed4321"), &json!("no"))
        );
        assert_eq!(
            (&p.rest["carried_from"], &p.rest["tree"], &p.rest["facts"]),
            (&json!(no.event_id), &json!(TREE), &facts.read)
        );
        let none = replay_on("abc1234", "q", 2, &facts).1.unwrap();
        assert!(none.is_none());

        // The owner waives the finding the NO saw: one new session may run;
        // its YES then stands, and a later waive does not reopen a YES.
        let owner = Actor {
            agent: Some("owner".into()),
            session: Some("o".into()),
        };
        let waive = |key: &str| crate::checks::FindingMove {
            check_event_id: 5,
            finding_key: key.into(),
            status: "waived".into(),
            reason: Some("not this PR".into()),
            head_sha: None,
        };
        ledger
            .finding_update(&owner, &waive("k"), false)
            .unwrap()
            .unwrap();
        assert!(replay_on("abc1234", "p", 2, &facts).1.unwrap().is_none());
        input.fields = fields(json!({"decision": "yes"}));
        let k = ledger.tree_key(&input, &facts.sha256).unwrap();
        let yes = ledger.finish_run(&me(), &input, true, &k).unwrap().unwrap();
        let stands = replay_on("abc1234", "p", 2, &facts).1.unwrap().unwrap();
        assert_eq!(stands.event_id, yes.event_id);
    }

    /// A session's smoke claim is shown with the end of each file it
    /// attached, labelled a claim, and leaves the digest as it was; when
    /// the profile declares a smoke the harness's own result is needed and
    /// is a fact like the tests, else it is not read.
    #[test]
    fn a_smoke_claim_is_shown_not_hashed() {
        let (dir, ledger) = lane();
        let store = Evidence::beside(&dir.path().join("ledger.db"));
        all(&ledger, "abc1234");
        let before = harness(&ledger, &store, 1, "abc1234", false)
            .unwrap()
            .unwrap();
        let log = format!("{}SAW", "x".repeat(5000));
        let stored = store.put(&log).unwrap();
        let file = crate::checks::Attached {
            path: "out.log".into(),
            sha256: stored.hash,
            bytes: log.len() as u64,
            truncated: false,
            redactions: 0,
        };
        let claim = CheckInput {
            work: 1,
            check: "smoke".into(),
            head: "abc1234".into(),
            outcome: "ran".into(),
            ..CheckInput::default()
        };
        let fresh = Actor {
            agent: Some("a".into()),
            session: Some("fresh".into()),
        };
        ledger
            .check_record_with(&fresh, &claim, &[file], false)
            .unwrap()
            .unwrap();
        let claimed = harness(&ledger, &store, 1, "abc1234", false)
            .unwrap()
            .unwrap();
        let shown = &claimed.shown["smoke"]["lane_claim"];
        let end = shown["evidence_files"][0]["end"].as_str().unwrap();
        assert_eq!((&shown["outcome"], end.len()), (&json!("ran"), CLAIM_BYTES));
        assert!(end.ends_with("SAW"));
        assert_eq!(claimed.sha256, before.sha256);
        // A profile that declares a smoke needs a passing harness one; one
        // that does not leaves it out of the facts.
        let declared = |on: bool| harness(&ledger, &store, 1, "abc1234", on).unwrap();
        let missing = "smoke (the base profile declares [smoke])";
        assert_eq!(declared(true).err(), Some(missing));
        measured(&ledger, "smoke", "abc1234", json!({"result": "fail"}));
        let failed = "smoke that passes (the newest failed)";
        assert_eq!(declared(true).err(), Some(failed));
        measured(&ledger, "smoke", "abc1234", json!({"result": "pass"}));
        let ran = declared(true).unwrap();
        assert_eq!(ran.shown["smoke"]["harness"]["result"], "pass");
        assert_ne!(ran.sha256, before.sha256);
        assert_eq!(declared(false).unwrap().sha256, before.sha256);
    }
}
