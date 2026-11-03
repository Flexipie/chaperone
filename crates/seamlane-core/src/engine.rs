//! The engine (R7 "Engine contract"). A gated command names a transition
//! and writes `phase.entered`; the write decides it, on the lane's stamped
//! pipeline and the facts projected from its events, inside the command's
//! one transaction. When a gate does not hold, the event appended is a
//! `gate.denied` instead, which moves nothing.

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

use crate::ledger::projection::project;
use crate::ledger::{Checked, Event, EventKind, Mutation, Payload, Refusal};
use crate::pipeline::{self, Denied, Phase, Route};

/// A gated command's event: `phase.entered` for `transition` to `to` on
/// `head`, carrying the command's `payload` (its `changed`, and a
/// handoff's `handoff`) and `status`, the status it asked for.
pub(crate) fn enter(
    lane_id: i64,
    transition: &str,
    to: Phase,
    head: Option<String>,
    status: &str,
    payload: Payload,
) -> Mutation {
    Mutation::Event {
        kind: EventKind::PhaseEntered,
        lane_id: Some(lane_id),
        spec_id: None,
        payload: Payload {
            transition: Some(transition.into()),
            to: json!(to).as_str().map(String::from),
            status: Some(status.into()),
            head_sha: head,
            ..payload
        },
    }
}

/// The `gate.denied` that replaces `entered`, a guarded `phase.entered`
/// on lane `lane_id`, when its transition does not hold; None when it does.
/// A lane with no stamp is denied as `stamp_ok`, never passed on the newest
/// pipeline; a stamp this binary cannot load is refused (`pipeline_unknown`).
pub(crate) fn deny(conn: &Connection, lane_id: i64, entered: &Event) -> Checked<Option<Event>> {
    let p = &entered.payload;
    // The guard has refused any transition but the host's.
    let transition = p.transition.clone().unwrap_or_default();
    let to = p.to.clone().unwrap_or_default();
    let Ok(to) = serde_json::from_value::<Phase>(Value::String(to.clone())) else {
        return Ok(Err(Refusal::InvalidInput(format!(
            "phase.entered needs a phase to, got '{to}'"
        ))));
    };
    let (id, hash, route): (Option<String>, Option<String>, Option<String>) = conn.query_row(
        "SELECT pipeline_id, pipeline_hash, route FROM lanes WHERE id = ?1",
        [lane_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let decided = match (id, hash, route) {
        (Some(id), Some(hash), Some(route)) => {
            let Some(doc) = pipeline::by_stamp(&id, &hash) else {
                return Ok(Err(Refusal::PipelineUnknown {
                    pipeline_id: id,
                    pipeline_hash: hash,
                }));
            };
            let route: Route =
                serde_json::from_value(Value::String(route)).map_err(std::io::Error::other)?;
            let facts = project(conn, lane_id, p.head_sha.as_deref())?;
            doc.evaluate(&transition, route, to, &facts)
        }
        _ => Err(Denied {
            gate: "stamp_ok".into(),
            fact_paths: vec!["lane.pipeline_hash".into()],
        }),
    };
    let Err(Denied { gate, fact_paths }) = decided else {
        return Ok(Ok(None));
    };
    // A retry with no new event on the lane replays this denial instead of
    // appending another (events.md, write rules).
    let latest: Option<i64> = conn
        .query_row(
            "SELECT id FROM events WHERE lane_id = ?1 AND kind != 'gate.denied'
             ORDER BY id DESC LIMIT 1",
            [lane_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(Ok(Some(Event {
        kind: EventKind::GateDenied,
        idempotency_key: Some(format!(
            "deny:{lane_id}:{transition}:{gate}:{}",
            latest.unwrap_or(0)
        )),
        payload: Payload {
            transition: Some(transition),
            gate: Some(gate),
            fact_paths: Some(fact_paths),
            head_sha: p.head_sha.clone(),
            ..Payload::default()
        },
        ..entered.clone()
    })))
}
