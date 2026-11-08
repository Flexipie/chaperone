//! One lane's detail (`chaperone-ui` draws it): its issue key's dossier
//! for the spec slice, its handoffs, its check results by head and its
//! reviews' findings, in one read transaction.

use rusqlite::{Transaction, TransactionBehavior};
use serde::Serialize;

use super::{Handoff, IS_HANDOFF, handoff_at};
use crate::dossier::{Dossier, dossier_read};
use crate::ledger::{Ledger, LedgerError, check_schema};

/// Rows kept per list, newest first.
pub const DETAIL_ROWS: i64 = 50;

#[derive(Debug, Serialize)]
pub struct LaneDetail {
    pub work_id: i64,
    /// The ledger's `data_version` this detail was read at.
    pub data_version: i64,
    /// None when the lane has no tracker key, or the ledger has nothing on it.
    pub dossier: Option<Dossier>,
    /// Newest first.
    pub handoffs: Vec<Handoff>,
    /// Newest first, one per check and head (`gate_results`).
    pub checks: Vec<CheckRow>,
    /// Open first, then newest check first.
    pub findings: Vec<FindingRow>,
}

#[derive(Debug, Serialize)]
pub struct CheckRow {
    pub check: String,
    pub head_sha: String,
    /// ran | skipped
    pub outcome: String,
    pub at: String,
}

#[derive(Debug, Serialize)]
pub struct FindingRow {
    pub check: String,
    pub head_sha: Option<String>,
    pub severity: String,
    /// open | fixed | answered
    pub status: String,
    pub path: Option<String>,
    pub title: String,
}

impl Ledger {
    /// Lane `id`'s detail; `key` is its tracker key (the board's
    /// `source_key`). An unknown lane has no checks or findings.
    pub fn lane_detail(&self, id: i64, key: Option<&str>) -> Result<LaneDetail, LedgerError> {
        let tx = Transaction::new_unchecked(self.conn(), TransactionBehavior::Deferred)?;
        check_schema(&tx)?;
        let data_version = self.data_version()?;
        // A key that does not parse or has no record is no dossier, not an
        // error: the rest of the detail still shows.
        let dossier = match key {
            Some(k) => dossier_read(&tx, k)?.ok(),
            None => None,
        };
        // By lane, not through the dossier: a lane with no tracker key has
        // handoffs too.
        let mut stmt = tx.prepare(&format!(
            "SELECT id, lane_id, actor_agent, actor_session,
                    json_extract(payload_json, '$.handoff'), at
               FROM events WHERE lane_id = ?1 AND {IS_HANDOFF} ORDER BY id DESC LIMIT ?2"
        ))?;
        let mut rows = stmt.query((id, DETAIL_ROWS))?;
        let mut handoffs = Vec::new();
        while let Some(r) = rows.next()? {
            handoffs.push(handoff_at(r, 0)?);
        }
        let mut stmt = tx.prepare(
            "SELECT gate, head_sha, status, at FROM gate_results WHERE lane_id = ?1
             ORDER BY event_id DESC LIMIT ?2",
        )?;
        let checks = stmt
            .query_map((id, DETAIL_ROWS), |r| {
                Ok(CheckRow {
                    check: r.get(0)?,
                    head_sha: r.get(1)?,
                    outcome: r.get(2)?,
                    at: r.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        // Every finished check of the lane (events_lane), then findings'
        // unique (check_event_id, finding_key): a check recorded again on
        // the same head keeps the earlier round's findings.
        let mut stmt = tx.prepare(
            "SELECT json_extract(e.payload_json, '$.check'), f.head_sha, f.severity,
                    f.status, f.path, f.title
               FROM events e JOIN findings f ON f.check_event_id = e.id
              WHERE e.lane_id = ?1 AND e.kind = 'check.finished'
              ORDER BY f.status = 'open' DESC, e.id DESC, f.id LIMIT ?2",
        )?;
        let findings = stmt
            .query_map((id, DETAIL_ROWS), |r| {
                Ok(FindingRow {
                    check: r.get(0)?,
                    head_sha: r.get(1)?,
                    severity: r.get(2)?,
                    status: r.get(3)?,
                    path: r.get(4)?,
                    title: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(LaneDetail {
            work_id: id,
            data_version,
            dossier,
            handoffs,
            checks,
            findings,
        })
    }
}
