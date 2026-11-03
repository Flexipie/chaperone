//! The fact bundle for one lane (R7), projected from its events. Gates read
//! these facts, never events or prose. A fact no M1 event produces stays
//! absent, so a gate that reads it denies (events.md, facts in M1).

use std::collections::BTreeSet;

use rusqlite::{Connection, params};
use serde_json::Value;

use super::LedgerError;
use super::event::{Event, EventKind};
use super::write::{EVENT_COLUMNS, event_from_row};
use crate::pipeline::{Fact, FactBundle};

/// Projects lane `lane_id`. `head` is the commit the caller wants to hand
/// off or finish (`commit.sha`): the handoff's commit, or for `update
/// --status done` the latest handoff's. It is never read from stored
/// events, so a new commit cannot borrow an older one's verdict.
pub(crate) fn project(
    conn: &Connection,
    lane_id: i64,
    head: Option<&str>,
) -> Result<FactBundle, LedgerError> {
    let mut f = FactBundle::default();
    let (id, hash, kind): (Option<String>, Option<String>, Option<String>) = conn.query_row(
        "SELECT pipeline_id, pipeline_hash, kind FROM lanes WHERE id = ?1",
        [lane_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if let (Some(id), Some(hash)) = (id, hash) {
        f.set("pipeline.id", id.as_str())
            .set("pipeline.hash", hash.as_str());
    }
    // Until diff_measured exists, only a code lane changes code.
    if let Some(kind) = kind {
        f.set("lane.changes_code", kind == "code");
    }
    if let Some(head) = head {
        f.set("commit.sha", head);
    }

    let mut stmt = conn.prepare(&format!(
        "SELECT {EVENT_COLUMNS} FROM events WHERE lane_id = ?1 ORDER BY id"
    ))?;
    let mut rows = stmt.query([lane_id])?;
    let (mut smoke, mut kept, mut rounds) = (false, false, 0);
    let mut latest_round: Option<i64> = None;
    let (mut requested, mut finished) = (BTreeSet::new(), BTreeSet::new());
    let mut review_state = "absent";
    while let Some(row) = rows.next()? {
        let e: Event = event_from_row(row)?;
        let p = &e.payload;
        let pr_set = p.changed.get("pr_ref").is_some_and(|c| !c.to.is_null());
        if pr_set && matches!(f.get("pr.opened_at"), Fact::Absent) {
            f.set("pr.opened_at", Fact::Time(e.at.clone()));
        }
        let job = p.rest.get("job").map(Value::to_string);
        match (e.kind, p.check.as_deref()) {
            (EventKind::CheckRequested, _) => {
                requested.extend(job);
            }
            (EventKind::CheckFinished, Some(check)) => {
                let ran = p.outcome.as_deref() == Some("ran");
                let text = |key: &str| p.rest.get(key).and_then(Value::as_str);
                match check {
                    "plan_check" | "quality_gate" => {
                        f.set(
                            &format!("{check}.outcome"),
                            p.outcome.as_deref().unwrap_or_default(),
                        );
                        match text("reason") {
                            Some(r) => f.set(&format!("{check}.reason"), r),
                            None => f.set(&format!("{check}.reason"), Fact::Absent),
                        };
                        // The first quality gate is the one the PR must follow.
                        if check == "quality_gate"
                            && matches!(f.get("quality_gate.at"), Fact::Absent)
                        {
                            f.set("quality_gate.at", Fact::Time(e.at.clone()));
                        }
                    }
                    "merge_verdict" => {
                        match text("decision") {
                            Some(d) => f.set("verdict.decision", d),
                            None => f.set("verdict.decision", Fact::Absent),
                        };
                        f.set("verdict.sha", p.head_sha.as_deref().unwrap_or_default());
                    }
                    "smoke" => smoke = true,
                    "kept_tests" => kept = true,
                    "local_review" | "pr_review" => {
                        finished.extend(job);
                        review_state = if ran { "finished" } else { "failed" };
                        if check == "local_review" && ran {
                            rounds += 1;
                            latest_round = Some(e.id);
                            f.set(
                                "review.local_head",
                                p.head_sha.as_deref().unwrap_or_default(),
                            );
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    f.set("smoke.recorded", smoke)
        .set("kept_tests.recorded", kept)
        .set("review.local_rounds", rounds);
    if !requested.is_subset(&finished) {
        review_state = "running";
    }
    f.set("daisy_review.state", review_state);
    if let Some(round) = latest_round {
        let open: i64 = conn.query_row(
            "SELECT count(*) FROM findings
             WHERE lane_id = ?1 AND check_event_id = ?2 AND status = 'open'",
            params![lane_id, round],
            |r| r.get(0),
        )?;
        f.set("review.previous_round_closed", open == 0);
    }
    Ok(f)
}
