//! The lane page's ledger data: the lane's newest handoff and the gates
//! its next gated step requires, in one read transaction (the hook's lane
//! context and `merge` read it).

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;

use super::{Handoff, IS_HANDOFF, handoff_at};
use crate::ledger::{Ledger, LedgerError, check_schema};
use crate::pipeline::{Phase, Route, by_id};

/// The `pr.merged` events of lane `?1`'s current PR: a lane that took
/// another PR after one merged can still merge it.
pub(crate) const CURRENT_PR_MERGED: &str = "FROM events
  WHERE lane_id = ?1 AND kind = 'pr.merged'
    AND lower(json_extract(payload_json, '$.pr'))
        = (SELECT lower(pr_ref) FROM lanes WHERE id = ?1)";

#[derive(Debug, Serialize)]
pub struct LanePage {
    /// The newest handoff, None before the first.
    pub handoff: Option<Handoff>,
    /// None when this binary has no document for the lane's stamp, or the
    /// document no row for its route: unstamped, never "0 of 0 passed".
    pub gates: Option<Gates>,
    /// The merge sha of the newest `pr.merged` of the lane's current PR, None
    /// before it merges.
    pub merged: Option<String>,
}

/// The `handoff` transition to ready (to done on a route without one)
/// on the lane's own document.
#[derive(Debug, Serialize)]
pub struct Gates {
    pub to: Phase,
    /// The head the facts were read for: the newest handoff's commit. A
    /// head gate holds for it, not for a newer PR head.
    pub head: Option<String>,
    pub gates: Vec<GateRow>,
}

impl Gates {
    /// Gate `id` is listed and passed.
    pub fn passed(&self, id: &str) -> bool {
        self.gates.iter().any(|r| r.gate == id && r.passed)
    }
}

#[derive(Debug, Serialize)]
pub struct GateRow {
    pub gate: String,
    pub passed: bool,
}

impl Ledger {
    /// Lane `id`'s page as of one snapshot, its schema re-checked inside
    /// it. An unknown lane has no handoff and no gates.
    pub fn lane_page(&self, id: i64) -> Result<LanePage, LedgerError> {
        let tx = Transaction::new_unchecked(self.conn(), TransactionBehavior::Deferred)?;
        check_schema(&tx)?;
        let handoff = tx
            .query_row(
                &format!(
                    "SELECT id, lane_id, actor_agent, actor_session,
                            json_extract(payload_json, '$.handoff'), at
                       FROM events WHERE lane_id = ?1 AND {IS_HANDOFF} ORDER BY id DESC LIMIT 1"
                ),
                [id],
                |r| Ok(handoff_at(r, 0)),
            )
            .optional()?
            .transpose()?;
        let merged = tx
            .query_row(
                &format!(
                    "SELECT json_extract(payload_json, '$.merge_sha') {CURRENT_PR_MERGED}
                      ORDER BY id DESC LIMIT 1"
                ),
                [id],
                |r| r.get(0),
            )
            .optional()?;

        let head = handoff.as_ref().and_then(|h| h.handoff.commit.clone());
        let stamp: Option<(Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT pipeline_id, route FROM lanes WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let mut gates = None;
        if let Some((pid, route)) = stamp {
            let facts = self.project(id, head.as_deref())?;
            gates = (|| {
                let doc = by_id(pid.as_deref()?)?;
                let route: Route = serde_json::from_value(route?.into()).ok()?;
                let to = match route {
                    Route::CodePr | Route::Direct => Phase::Ready,
                    Route::NoPr | Route::Investigation => Phase::Done,
                };
                let gates = doc.gates("handoff", route, to, &facts).ok()?;
                Some(Gates {
                    to,
                    head: head.clone(),
                    gates: gates
                        .into_iter()
                        .map(|(gate, passed)| GateRow { gate, passed })
                        .collect(),
                })
            })();
        }
        Ok(LanePage {
            handoff,
            gates,
            merged,
        })
    }
}
