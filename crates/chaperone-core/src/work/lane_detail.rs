//! One lane's detail (`chaperone-ui` draws it): its issue key's spec
//! slice (the dossier's section), its handoffs and its reviews' findings,
//! inside the lane page's read transaction.

use rusqlite::Connection;
use serde::Serialize;

use super::{Handoff, IS_HANDOFF, handoff_at};
use crate::dossier::{SliceSection, slice_of};
use crate::ledger::{Ledger, LedgerError};

/// Rows kept per list, newest first.
pub const DETAIL_ROWS: i64 = 50;

#[derive(Debug, Serialize)]
pub struct LaneDetail {
    pub work_id: i64,
    /// The ledger's `data_version` this detail was read at.
    pub data_version: i64,
    /// None when the lane has no tracker key, or one that does not parse.
    pub slice: Option<SliceSection>,
    /// Newest first.
    pub handoffs: Vec<Handoff>,
    /// Open first, then newest check first.
    pub findings: Vec<FindingRow>,
}

#[derive(Debug, Serialize)]
pub struct FindingRow {
    pub check: String,
    pub head_sha: Option<String>,
    pub severity: String,
    /// open | fixed | answered | verified | refuted | disputed | waived | accepted
    pub status: String,
    pub path: Option<String>,
    pub title: String,
}

impl Ledger {
    /// Lane `id`'s detail inside the caller's read transaction; `key` is
    /// its tracker key (the board's `source_key`). An unknown lane has no
    /// findings.
    pub(super) fn detail_in(
        &self,
        tx: &Connection,
        id: i64,
        key: Option<&str>,
    ) -> Result<LaneDetail, LedgerError> {
        let data_version = self.data_version()?;
        let slice = match key {
            Some(k) => slice_of(tx, k)?,
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
            slice,
            handoffs,
            findings,
        })
    }
}
