//! `check run verdict`: whether the lane's commit should merge, judged
//! from facts the harness gathered or holds, here the harness's own
//! results on the commit.
//!
//! A decision stands for the tree it judged: the same tree, pipeline and
//! harness facts replay it, yes or no, and start no session, so a verdict
//! cannot be run again until it says yes. It is a `merge_verdict` with the
//! harness producer, so the facts and gates that read verdicts read it.

use std::io;

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::checks::{Recorded, RunInput};
use crate::ledger::{Actor, Checked, Ledger, LedgerError, Payload};
use crate::pipeline::canonical_string;

/// The event's check: the session verdict's, with the harness producer.
pub const EVENT_CHECK: &str = "merge_verdict";

/// The harness's own results on one commit, as the verdict is shown them.
pub struct Harness {
    /// The key fields of the newest harness `diff`, `tests` and complete
    /// `review` on the commit, and the lane's findings that still block.
    /// Nothing in it names the commit, so the same results on an amended
    /// commit read the same.
    pub shown: Value,
    /// The sha256 of `shown`'s canonical JSON.
    pub sha256: String,
    /// The ids of the events it was read from.
    pub read: Value,
}

/// What the harness holds on lane `work`'s commit `head`, or the check it
/// still lacks there: a verdict needs a `diff`, a `tests` result (pass or
/// fail) and a `complete` review, so its facts can only change by the
/// harness doing more work, never by the lane running it early.
pub fn harness(
    ledger: &Ledger,
    work: i64,
    head: &str,
) -> Result<Result<Harness, &'static str>, LedgerError> {
    let newest = |check: &str, complete: bool| -> Result<Option<(i64, Payload)>, LedgerError> {
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
    };
    let Some(diff) = newest("diff", false)? else {
        return Ok(Err("diff"));
    };
    let Some(tests) = newest("tests", false)? else {
        return Ok(Err("tests"));
    };
    let Some(review) = newest("review", true)? else {
        return Ok(Err("complete review"));
    };
    let pick = |p: &Payload, keys: &[&str]| -> Value {
        let got = keys
            .iter()
            .map(|k| (k.to_string(), p.rest.get(*k).cloned()));
        Value::Object(got.map(|(k, v)| (k, v.unwrap_or(Value::Null))).collect())
    };
    let mut tests_shown = pick(&tests.1, &["result", "timed_out"]);
    // Without `ms`, which differs on every run of the same tests.
    let ran = tests.1.rest.get("commands").and_then(Value::as_array);
    let ran = ran.into_iter().flatten();
    let ran = ran.map(|c| json!({"argv": c["argv"], "exit": c["exit"]}));
    tests_shown["commands"] = Value::Array(ran.collect());
    // Once each: an amend's review raises the same findings again.
    let mut blocking: Vec<_> = ledger
        .blocking_findings(work)?
        .into_iter()
        .map(|f| (f.finding_key, f.severity, f.status, f.title))
        .collect();
    blocking.sort();
    blocking.dedup();
    let blocking = blocking.into_iter().map(|(key, severity, status, title)| {
        json!({"key": key, "severity": severity, "status": status, "title": title})
    });
    let shown = json!({
        "diff": pick(&diff.1, &["files", "added", "removed", "code_lines", "changes_code", "uncounted_files"]),
        "tests": tests_shown,
        "review": {"complete": true, "findings": review.1.finding_list().len()},
        "blocking_findings": blocking.collect::<Vec<_>>(),
    });
    Ok(Ok(Harness {
        sha256: format!("{:x}", Sha256::digest(canonical_string(&shown))),
        read: json!({"diff": diff.0, "tests": tests.0, "review": review.0}),
        shown,
    }))
}

/// The start of `check run verdict`, before anything is run: `input` keyed
/// for a verdict by the pipeline `hash`, then the verdict that ran on the
/// same tree, pipeline and `harness` facts when there is one. On this
/// commit it replays. On another (an amend that kept the tree) it is
/// carried: a new event on this commit with the same decision and
/// `carried_from`, so a gate comparing the verdict's commit with the head
/// reads this one, and no session starts.
pub fn replay(
    ledger: &Ledger,
    actor: &Actor,
    input: &mut RunInput,
    hash: &str,
    harness: &Harness,
) -> Checked<Option<Recorded>> {
    input.check = EVENT_CHECK.into();
    input.profile = Some(hash.into());
    let Some(stored) = ledger.tree_run(input, "facts_sha256", &harness.sha256)? else {
        return Ok(Ok(None));
    };
    if stored.payload.head_sha.as_deref() == Some(input.head.as_str()) {
        return Ok(Ok(Some(stored)));
    }
    let mut fields = stored.payload.rest;
    for k in ["tree", "profile", "producer", "redactions"] {
        fields.remove(k);
    }
    let from = fields.get("carried_from").cloned();
    fields.insert(
        "carried_from".into(),
        from.unwrap_or(json!(stored.event_id)),
    );
    input.fields = fields;
    let key = ledger.tree_key(input, &harness.sha256)?;
    Ok(ledger.finish_run(actor, input, true, &key)?.map(Some))
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

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
        measured(ledger, "review", head, json!({"complete": true}));
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
        let (_dir, ledger) = lane();
        let lacks = |head: &str| harness(&ledger, 1, head).unwrap().err();
        assert_eq!(lacks("abc1234"), Some("diff"));
        measured(
            &ledger,
            "diff",
            "abc1234",
            json!({"files": 1, "code_lines": 4}),
        );
        assert_eq!(lacks("abc1234"), Some("tests"));
        measured(&ledger, "tests", "abc1234", json!({"result": "fail"}));
        assert_eq!(lacks("abc1234"), Some("complete review"));
        measured(&ledger, "review", "abc1234", json!({"complete": false}));
        assert_eq!(lacks("abc1234"), Some("complete review"));
        all(&ledger, "abc1234");
        let first = harness(&ledger, 1, "abc1234").unwrap().unwrap();
        let want = json!({
            "diff": {"files": 1, "added": null, "removed": null, "code_lines": 4,
                     "changes_code": null, "uncounted_files": null},
            "tests": {"result": "pass", "timed_out": null, "commands": [{"argv": ["t"], "exit": 0}]},
            "review": {"complete": true, "findings": 1},
            "blocking_findings": [{"key": "k", "severity": "blocker", "status": "open", "title": "T"}],
        });
        assert_eq!(first.shown, want);
        assert_eq!(first.read, json!({"diff": 3, "tests": 6, "review": 7}));
        all(&ledger, "fed4321");
        let amend = harness(&ledger, 1, "fed4321").unwrap().unwrap();
        assert_eq!(
            (amend.sha256, amend.read["diff"].clone()),
            (first.sha256, json!(8))
        );
    }

    /// A verdict that ran replays on its commit and is carried to an amend
    /// on the same tree; a skipped one, another pipeline or other facts
    /// find none.
    #[test]
    fn a_verdict_stands_for_its_tree() {
        let (_dir, ledger) = lane();
        all(&ledger, "abc1234");
        let facts = harness(&ledger, 1, "abc1234").unwrap().unwrap();
        let mut input = run("", "abc1234", json!({}));
        assert!(
            replay(&ledger, &me(), &mut input, "p", &facts)
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert_eq!(input.check, EVENT_CHECK);
        let fields = |f: Value| f.as_object().cloned().unwrap_or_default();
        input.fields = fields(json!({"reason": "no_answer", "facts_sha256": facts.sha256}));
        let k = ledger.tree_key(&input, &facts.sha256).unwrap();
        ledger
            .finish_run(&me(), &input, false, &k)
            .unwrap()
            .unwrap();
        assert!(
            replay(&ledger, &me(), &mut input, "p", &facts)
                .unwrap()
                .unwrap()
                .is_none()
        );
        input.fields = fields(json!({"decision": "no", "facts_sha256": facts.sha256}));
        let k = ledger.tree_key(&input, &facts.sha256).unwrap();
        let no = ledger.finish_run(&me(), &input, true, &k).unwrap().unwrap();

        let again = replay(&ledger, &me(), &mut input, "p", &facts)
            .unwrap()
            .unwrap();
        let again = again.unwrap();
        assert_eq!((again.replayed, again.event_id), (true, no.event_id));
        let mut amend = run("", "fed4321", json!({}));
        let carried = replay(&ledger, &me(), &mut amend, "p", &facts)
            .unwrap()
            .unwrap();
        let p = carried.unwrap().payload;
        assert_eq!(
            (p.head_sha.as_deref(), &p.rest["decision"]),
            (Some("fed4321"), &json!("no"))
        );
        assert_eq!(
            (&p.rest["carried_from"], &p.rest["tree"]),
            (&json!(no.event_id), &json!(TREE))
        );
        let mut other = run("", "abc1234", json!({}));
        let none = replay(&ledger, &me(), &mut other, "q", &facts)
            .unwrap()
            .unwrap();
        assert!(none.is_none());
        let changed = Harness {
            sha256: "0".repeat(64),
            shown: Map::new().into(),
            read: Value::Null,
        };
        let none = replay(&ledger, &me(), &mut other, "p", &changed)
            .unwrap()
            .unwrap();
        assert!(none.is_none());
    }
}
