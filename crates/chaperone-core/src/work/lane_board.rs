//! The overview board: every lane `work board` shows, with its gate stamps
//! and its spec, in one read transaction (`chaperone-ui` draws it).

use std::convert::Infallible;

use rusqlite::{Transaction, TransactionBehavior};
use serde::Serialize;

use super::BoardItem;
use crate::ledger::{Ledger, LedgerError, check_schema};
use crate::pipeline::Fact;

#[derive(Clone, Debug, Serialize)]
pub struct LaneBoard {
    /// The ledger's `data_version` this board was read at.
    pub data_version: i64,
    /// The specs the lanes link to, by id.
    pub specs: Vec<SpecTitle>,
    /// In board order (lane key, lane position, id).
    pub lanes: Vec<BoardLane>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpecTitle {
    pub id: i64,
    pub key: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BoardLane {
    pub board: BoardItem,
    /// `updated_at` in seconds since the epoch, for a viewer's clock.
    pub updated_secs: i64,
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
        let lanes = lanes
            .into_iter()
            .map(|board| {
                let facts = self.project(board.item.id, None)?;
                let updated_secs =
                    tx.query_row("SELECT unixepoch(?1)", [&board.item.updated_at], |r| {
                        r.get(0)
                    })?;
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
                    stamps,
                })
            })
            .collect::<Result<_, LedgerError>>()?;
        let mut stmt = tx.prepare(
            "SELECT id, external_key, title FROM specs WHERE id IN
               (SELECT spec_id FROM lanes WHERE status NOT IN ('done', 'cancelled'))
             ORDER BY id",
        )?;
        let specs = stmt
            .query_map([], |r| {
                Ok(SpecTitle {
                    id: r.get(0)?,
                    key: r.get(1)?,
                    title: r.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(LaneBoard {
            data_version,
            specs,
            lanes,
        })
    }
}
