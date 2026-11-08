//! The lane page's ledger data (`chaperone-ui` draws it): the lane's
//! detail, its timeline, its decisions with who made them, its review
//! rounds and the gates its next gated step requires, in one read
//! transaction. Its files are git's, not the ledger's
//! (`git::lane_files`), and its repo's median claim to merge
//! ([`Ledger::repo_median_h`]) folds every lane the repo merged in 30 days:
//! neither belongs on a read that reruns at every commit.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

use super::DETAIL_ROWS;
use super::lane_detail::LaneDetail;
use crate::ledger::{Ledger, LedgerError, check_schema};
use crate::pipeline::{Fact, Phase, Route, by_stamp};
use crate::spec::parse::parse_decision;
use crate::stats::Cohort;

#[derive(Debug, Serialize)]
pub struct LanePage {
    pub detail: LaneDetail,
    /// Newest first: every lane event but usage, lease renewals, check
    /// requests and finding updates.
    pub timeline: Vec<TimelineRow>,
    /// The spec's (this slice's and the spec-wide), the handoff brief's
    /// (newest handoff) and the answers to the lane's questions.
    pub decisions: Vec<DecisionRow>,
    /// `review.local_rounds`: the engine's count, the one the gates read.
    pub review_rounds: i64,
    /// None when this binary has no document for the lane's stamp, or the
    /// document no row for its route: unstamped, never "0 of 0 passed".
    pub gates: Option<Gates>,
}

#[derive(Debug, Serialize)]
pub struct TimelineRow {
    pub at: String,
    /// Since the lane's `claimed_at`; None before a claim.
    pub minutes: Option<i64>,
    /// The event kind (events.md).
    pub kind: String,
    /// What it says in a few words: the phase, the check and its outcome,
    /// the attention type and who handled it, the new status, the PR, the
    /// denying gate.
    pub what: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DecisionRow {
    /// The lane agent for a brief's decision, `owner` or `orchestrator`
    /// for an answer; None for a spec line, which records no who.
    pub by: Option<String>,
    /// A spec line's "agreed:" (where and when, as written).
    pub agreed: Option<String>,
    /// When it was recorded; None for a spec line.
    pub at: Option<String>,
    pub text: String,
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

#[derive(Debug, Serialize)]
pub struct GateRow {
    pub gate: String,
    pub passed: bool,
}

/// Events the timeline leaves out: the noise between the steps.
const QUIET: &str = "('usage.reported', 'lane.renewed', 'check.requested', 'finding.updated')";

impl Ledger {
    /// Lane `id`'s page as of one snapshot, its schema re-checked inside
    /// it; `key` is its tracker key as for [`Ledger::lane_detail`]. An
    /// unknown lane has an empty timeline and no gates.
    pub fn lane_page(&self, id: i64, key: Option<&str>) -> Result<LanePage, LedgerError> {
        let tx = Transaction::new_unchecked(self.conn(), TransactionBehavior::Deferred)?;
        check_schema(&tx)?;
        let detail = self.detail_in(&tx, id, key)?;
        let j = |k: &str| format!("json_extract(e.payload_json, '$.{k}')");
        let mut stmt = tx.prepare(&format!(
            "SELECT e.at, CAST(round((julianday(e.at) - julianday(l.claimed_at)) * 1440) AS INTEGER),
                    e.kind, e.actor_agent, {}, {}, {}, {}, {}, {}, {}, {}
               FROM events e JOIN lanes l ON l.id = e.lane_id
              WHERE e.lane_id = ?1 AND e.kind NOT IN {QUIET}
              ORDER BY e.id DESC LIMIT ?2",
            j("to"),
            j("check"),
            j("outcome"),
            j("type"),
            j("by"),
            j("changed.status.to"),
            j("pr"),
            j("gate"),
        ))?;
        let timeline = stmt
            .query_map((id, DETAIL_ROWS), |r| {
                let kind: String = r.get(2)?;
                let s = |i: usize| r.get::<_, Option<String>>(i);
                let pair = |a: Option<String>, b: Option<String>| match (a, b) {
                    (Some(a), Some(b)) => Some(format!("{a} {b}")),
                    (a, b) => a.or(b),
                };
                let what = match kind.as_str() {
                    "phase.entered" => s(4)?,
                    "check.finished" => pair(s(5)?, s(6)?),
                    "attention.recorded" => pair(s(7)?, s(8)?.map(|b| format!("by {b}"))),
                    "lane.updated" => s(9)?,
                    "pr.opened" | "pr.merged" => s(10)?,
                    "gate.denied" => s(11)?,
                    _ => None,
                };
                Ok(TimelineRow {
                    at: r.get(0)?,
                    minutes: r.get(1)?,
                    kind,
                    what,
                    agent: r.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;

        let mut decisions: Vec<DecisionRow> = detail
            .slice
            .iter()
            .filter_map(|s| s.in_spec.as_ref())
            .flat_map(|s| &s.decisions)
            .map(|line| DecisionRow {
                by: None,
                agreed: parse_decision(line).agreed,
                at: None,
                text: line.clone(),
            })
            .collect();
        if let Some(h) = detail.handoffs.first() {
            let brief = h.handoff.brief.iter().flat_map(|b| &b.decisions);
            decisions.extend(brief.map(|d| DecisionRow {
                by: h.agent.clone(),
                agreed: None,
                at: Some(h.created_at.clone()),
                text: d.clone(),
            }));
        }
        let mut stmt = tx.prepare(
            "SELECT json_extract(payload_json, '$.by'), at, json_extract(payload_json, '$.note')
               FROM events WHERE lane_id = ?1 AND kind = 'attention.recorded'
                AND json_extract(payload_json, '$.type') = 'answered'
              ORDER BY id DESC LIMIT ?2",
        )?;
        let answers = stmt.query_map((id, DETAIL_ROWS), |r| {
            Ok(DecisionRow {
                by: r.get(0)?,
                agreed: None,
                at: r.get(1)?,
                text: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })?;
        for a in answers {
            decisions.push(a?);
        }

        let head = detail
            .handoffs
            .first()
            .and_then(|h| h.handoff.commit.clone());
        let stamp: Option<(Option<String>, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT pipeline_id, pipeline_hash, route FROM lanes WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (mut review_rounds, mut gates) = (0, None);
        if let Some((pid, hash, route)) = stamp {
            let facts = self.project(id, head.as_deref())?;
            if let Fact::Int(n) = facts.get("review.local_rounds") {
                review_rounds = *n;
            }
            gates = (|| {
                let doc = by_stamp(pid.as_deref()?, hash.as_deref()?)?;
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
            detail,
            timeline,
            decisions,
            review_rounds,
            gates,
        })
    }

    /// `stats`'s `claim_to_merge_h_median` over `repo`'s lanes (any case)
    /// with a `pr.merged` in the last 30 days; null when none has merged.
    pub fn repo_median_h(&self, repo: &str) -> Result<Value, LedgerError> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT e.lane_id FROM events e JOIN lanes l ON l.id = e.lane_id
              WHERE e.kind = 'pr.merged' AND l.repo_slug = ?1 COLLATE NOCASE
                AND json_extract(e.payload_json, '$.merged_at')
                    >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-30 days')",
        )?;
        let ids = stmt
            .query_map([repo], |r| r.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        // The ids were just read, so none is refused.
        match self.stats(&Cohort::Work(ids))? {
            Ok(mut v) => Ok(v["cohort"]["claim_to_merge_h_median"].take()),
            Err(r) => Err(std::io::Error::other(format!("stats: {r:?}")).into()),
        }
    }
}
