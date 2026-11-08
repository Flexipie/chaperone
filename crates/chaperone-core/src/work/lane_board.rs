//! The overview board: every lane `work board` shows, with its gate stamps
//! and its spec, and the feed of decisions and questions between them, in
//! one read transaction (`chaperone-ui` draws it).

use std::collections::HashMap;
use std::convert::Infallible;

use rusqlite::{Transaction, TransactionBehavior};
use serde::Serialize;

use super::BoardItem;
use crate::ledger::{Ledger, LedgerError, check_schema};
use crate::pipeline::Fact;
use crate::spec::parse::parse_decisions;

/// Decisions kept per spec, and questions and answers on the board.
const FEED: i64 = 50;

#[derive(Clone, Debug, Serialize)]
pub struct LaneBoard {
    /// The ledger's `data_version` this board was read at.
    pub data_version: i64,
    /// The specs the lanes link to, by id.
    pub specs: Vec<SpecTitle>,
    /// In board order (lane key, lane position, id).
    pub lanes: Vec<BoardLane>,
    /// Questions asked and answered on the board's lanes, newest first.
    pub attention: Vec<AttentionRow>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpecTitle {
    pub id: i64,
    pub key: String,
    pub title: String,
    /// The head version's `## decisions` lines, the newest (last) first.
    pub decisions: Vec<String>,
}

/// An `attention.recorded` of type `asked` or `answered`.
#[derive(Clone, Debug, Serialize)]
pub struct AttentionRow {
    pub work_id: i64,
    pub at: String,
    /// asked | answered
    pub kind: String,
    /// orchestrator | owner: who handled it; for asked, who it went to
    pub by: String,
    /// asked only: R12's question id
    pub reason: Option<String>,
    /// The question, or the answer itself
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BoardLane {
    pub board: BoardItem,
    /// `updated_at` in seconds since the epoch, for a viewer's clock.
    pub updated_secs: i64,
    /// `lease_expires_at` the same way, so a viewer sees a lease expire
    /// while the ledger is quiet.
    pub lease_expires_secs: Option<i64>,
    /// The first `lane.claimed`'s `at` the same way: `claimed_at` restarts
    /// on every renewal and takeover; time running, as `stats` counts it,
    /// does not.
    pub claimed_secs: Option<i64>,
    /// Questions asked of the owner with no answer recorded.
    pub open_questions: i64,
    pub stamps: Stamps,
}

/// The gate facts the projection reads (events.md, facts in M1), so the
/// board shows what the gates see. None is no record.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Stamps {
    /// ran | skipped
    pub plan_check: Option<String>,
    /// ran | skipped
    pub quality_gate: Option<String>,
    /// The latest merge verdict that ran, and the head it judged.
    pub verdict: Option<String>,
    pub verdict_sha: Option<String>,
}

impl Ledger {
    /// The board, its stamps and specs as of one snapshot, its schema
    /// re-checked inside it: a newer binary may migrate while a viewer is
    /// open.
    pub fn lane_board(&self, now: &str) -> Result<LaneBoard, LedgerError> {
        let tx = Transaction::new_unchecked(self.conn(), TransactionBehavior::Deferred)?;
        check_schema(&tx)?;
        let data_version = self.data_version()?;
        let mut lanes = Vec::new();
        let Ok(()) = self.work_board_each(now, |board| {
            lanes.push(board);
            Ok::<_, Infallible>(())
        })?;
        // Questions to the owner with no answer, per open lane, on
        // events_lane: an answer is on its question's lane.
        let open_questions: HashMap<i64, i64> = tx
            .prepare(
                "SELECT a.lane_id, count(*) FROM events a
                  WHERE a.lane_id IN
                          (SELECT id FROM lanes WHERE status NOT IN ('done', 'cancelled'))
                    AND a.kind = 'attention.recorded'
                    AND json_extract(a.payload_json, '$.type') = 'asked'
                    AND json_extract(a.payload_json, '$.by') = 'owner'
                    AND NOT EXISTS (SELECT 1 FROM events b
                          WHERE b.lane_id = a.lane_id AND b.id > a.id
                            AND b.kind = 'attention.recorded'
                            AND json_extract(b.payload_json, '$.answers') = a.id)
                  GROUP BY a.lane_id",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let lanes = lanes
            .into_iter()
            .map(|board| {
                let facts = self.project(board.item.id, None)?;
                let item = &board.item;
                let (updated_secs, lease_expires_secs, claimed_secs) = tx.query_row(
                    "SELECT unixepoch(?1), unixepoch(?2),
                            (SELECT unixepoch(min(at)) FROM events
                              WHERE lane_id = ?3 AND kind = 'lane.claimed')",
                    rusqlite::params![item.updated_at, item.lease_expires_at, item.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                let open_questions = open_questions.get(&item.id).copied().unwrap_or(0);
                let text = |path: &str| match facts.get(path) {
                    Fact::Str(s) => Some(s.clone()),
                    _ => None,
                };
                let stamps = Stamps {
                    plan_check: text("plan_check.outcome"),
                    quality_gate: text("quality_gate.outcome"),
                    verdict: text("verdict.decision"),
                    verdict_sha: text("verdict.sha"),
                };
                Ok(BoardLane {
                    board,
                    updated_secs,
                    lease_expires_secs,
                    claimed_secs,
                    open_questions,
                    stamps,
                })
            })
            .collect::<Result<_, LedgerError>>()?;
        let mut stmt = tx.prepare(
            "SELECT s.id, s.external_key, s.title, v.body FROM specs s
               JOIN spec_versions v ON v.spec_id = s.id AND v.version = s.head_version
             WHERE s.id IN
               (SELECT spec_id FROM lanes WHERE status NOT IN ('done', 'cancelled'))
             ORDER BY s.id",
        )?;
        let specs = stmt
            .query_map([], |r| {
                let body: String = r.get(3)?;
                let decisions = parse_decisions(&body).into_iter().rev().take(FEED as usize);
                Ok(SpecTitle {
                    id: r.get(0)?,
                    key: r.get(1)?,
                    title: r.get(2)?,
                    decisions: decisions.map(|d| d.text).collect(),
                })
            })?
            .collect::<Result<_, _>>()?;
        // Per open lane on events_lane, kind before the payload is parsed.
        let mut stmt = tx.prepare(
            "SELECT lane_id, at, json_extract(payload_json, '$.type'),
                    json_extract(payload_json, '$.by'), json_extract(payload_json, '$.reason'),
                    json_extract(payload_json, '$.note')
               FROM events
              WHERE lane_id IN (SELECT id FROM lanes WHERE status NOT IN ('done', 'cancelled'))
                AND kind = 'attention.recorded'
                AND json_extract(payload_json, '$.type') IN ('asked', 'answered')
              ORDER BY id DESC LIMIT ?1",
        )?;
        let attention = stmt
            .query_map([FEED], |r| {
                Ok(AttentionRow {
                    work_id: r.get(0)?,
                    at: r.get(1)?,
                    kind: r.get(2)?,
                    by: r.get(3)?,
                    reason: r.get(4)?,
                    note: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(LaneBoard {
            data_version,
            specs,
            lanes,
            attention,
        })
    }
}
