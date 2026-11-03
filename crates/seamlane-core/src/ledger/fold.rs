//! Folds one event into the state tables. `apply` runs it after its checks,
//! and `rebuild` runs it over the whole log, so the tables are always a
//! function of the log. An event that does not fit the tables is an error.

use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde_json::Value;

use super::event::{Event, EventKind};
use super::write::{EVENT_COLUMNS, event_from_row};
use super::{Ledger, LedgerError, check_schema};

/// Lane columns an event may set through `changed`. The lease columns, the
/// pipeline stamp, `head_event_id` and `updated_at` are set by the fold itself.
pub(super) const LANE_COLUMNS: &[&str] = &[
    "spec_id",
    "lane_key",
    "status",
    "next_action",
    "blocker",
    "pr_ref",
    "base_sha",
    "lane_pos",
    "agent",
    "session",
    "head_sha",
    "branch",
    "worktree_path",
    "pid",
    "exit_code",
    "cli_session_id",
    "port_base",
    "port_count",
    "viewer_hint",
];

/// The `LANE_COLUMNS` that store an integer; the rest store text.
pub(super) const INTEGER_COLUMNS: &[&str] = &[
    "spec_id",
    "lane_pos",
    "pid",
    "exit_code",
    "port_base",
    "port_count",
];

pub(super) fn fold(conn: &Connection, e: &Event) -> Result<(), LedgerError> {
    use EventKind::*;
    let p = &e.payload;
    match e.kind {
        SpecCreated => {
            conn.execute(
                "INSERT INTO specs VALUES (?1, ?2, ?3, 'draft', 1, ?4, ?5)",
                params![e.spec_id, p.parent_key, p.title, e.id, e.at],
            )?;
            spec_version(conn, e, 1)?;
        }
        SpecRevised => {
            one(conn.execute(
                "UPDATE specs SET head_version = ?2, head_event_id = ?3, updated_at = ?4
                 WHERE id = ?1",
                params![e.spec_id, p.version, e.id, e.at],
            )?)?;
            spec_version(conn, e, p.version.unwrap_or_default())?;
        }
        SpecApproved => one(conn.execute(
            "UPDATE specs SET status = 'approved', head_event_id = ?2, updated_at = ?3
             WHERE id = ?1",
            params![e.spec_id, e.id, e.at],
        )?)?,
        LaneCreated => {
            conn.execute(
                "INSERT INTO lanes (id, title, source, source_id, repo_slug, kind, pr_flow,
                   next_action, spec_id, lane_key, lane_pos, status, head_event_id,
                   created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'queued', ?12, ?13, ?13)",
                params![
                    e.lane_id,
                    p.title,
                    p.source,
                    p.source_id,
                    p.repo,
                    p.kind,
                    p.pr_flow,
                    p.next_action,
                    e.spec_id,
                    p.lane_key,
                    p.lane_pos,
                    e.id,
                    e.at
                ],
            )?;
        }
        _ => fold_lane(conn, e)?,
    }
    Ok(())
}

fn fold_lane(conn: &Connection, e: &Event) -> Result<(), LedgerError> {
    let p = &e.payload;
    for (field, change) in &p.changed {
        // The write refused any other name; a log naming one is not ours.
        if !LANE_COLUMNS.contains(&field.as_str()) {
            return Err(std::io::Error::other(format!("event {} sets {field}", e.id)).into());
        }
        one(conn.execute(
            &format!("UPDATE lanes SET {field} = ?2 WHERE id = ?1"),
            params![e.lane_id, sql_value(&change.to)],
        )?)?;
    }
    // A cleared holder takes its lease with it, so no read sees a live
    // lease on an unheld lane. A lease restarts the claim and heartbeat
    // clocks, as daisy's claim does.
    let released = p.changed.get("session").is_some_and(|c| c.to.is_null());
    if e.kind.leases() || released {
        let since = e.kind.leases().then_some(&e.at);
        one(conn.execute(
            "UPDATE lanes SET lease_secs = ?2, lease_expires_at = ?3, claimed_at = ?4,
               last_heartbeat_at = ?4
             WHERE id = ?1",
            params![e.lane_id, p.lease_secs, p.lease_expires_at, since],
        )?)?;
    }
    // The write puts a stamp only on a lane's first claim.
    if let Some(hash) = &p.pipeline_hash {
        one(conn.execute(
            "UPDATE lanes SET pipeline_id = ?2, pipeline_hash = ?3, route = ?4 WHERE id = ?1",
            params![e.lane_id, p.pipeline_id, hash, p.route],
        )?)?;
    }
    one(conn.execute(
        "UPDATE lanes SET head_event_id = ?2, updated_at = iif(?4, updated_at, ?3) WHERE id = ?1",
        params![e.lane_id, e.id, e.at, e.kind.keeps_updated_at()],
    )?)?;
    match e.kind {
        EventKind::CheckFinished => {
            conn.execute(
                "INSERT INTO gate_results VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (lane_id, gate, head_sha) DO UPDATE SET
                   status = excluded.status, evidence_ref = excluded.evidence_ref,
                   event_id = excluded.event_id, at = excluded.at",
                params![
                    e.lane_id,
                    p.check,
                    p.head_sha,
                    p.outcome,
                    p.evidence_ref,
                    e.id,
                    e.at
                ],
            )?;
            for f in p.finding_list() {
                conn.execute(
                    "INSERT INTO findings (lane_id, check_event_id, finding_key, severity,
                       status, head_sha, path, title, body, event_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'open', ?5, ?6, ?7, ?8, ?2, ?9)",
                    params![
                        e.lane_id,
                        e.id,
                        f.finding_key,
                        f.severity,
                        p.head_sha,
                        f.path,
                        f.title,
                        f.body,
                        e.at
                    ],
                )?;
            }
        }
        EventKind::FindingUpdated => one(conn.execute(
            "UPDATE findings SET status = ?4, event_id = ?5, updated_at = ?6
             WHERE lane_id = ?1 AND check_event_id = ?2 AND finding_key = ?3",
            params![
                e.lane_id,
                p.check_event_id,
                p.finding_key,
                p.status,
                e.id,
                e.at
            ],
        )?)?,
        _ => {}
    }
    Ok(())
}

/// A `changed.to` as the lane row stores it: text, an integer or null.
pub(super) fn sql_value(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        v => v.as_i64().map_or(
            rusqlite::types::Value::Null,
            rusqlite::types::Value::Integer,
        ),
    }
}

fn spec_version(conn: &Connection, e: &Event, version: i64) -> Result<(), LedgerError> {
    conn.execute(
        "INSERT INTO spec_versions VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            e.spec_id,
            version,
            e.payload.body,
            e.actor.agent,
            e.id,
            e.at
        ],
    )?;
    Ok(())
}

fn one(changes: usize) -> Result<(), LedgerError> {
    if changes != 1 {
        return Err(std::io::Error::other(format!("fold changed {changes} rows, not 1")).into());
    }
    Ok(())
}

impl Ledger {
    /// Empties the state tables and folds the log back into them, in one
    /// transaction. `lease_expires_at` and `last_heartbeat_at` are outside
    /// the fold (heartbeats write no event), so their live values are kept.
    pub fn rebuild(&self) -> Result<(), LedgerError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        check_schema(&tx)?;
        let leases: Vec<(i64, Option<String>, Option<String>)> = tx
            .prepare("SELECT id, lease_expires_at, last_heartbeat_at FROM lanes")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        tx.execute_batch(
            "DELETE FROM specs; DELETE FROM spec_versions; DELETE FROM lanes;
             DELETE FROM gate_results; DELETE FROM findings; DELETE FROM messages;",
        )?;
        {
            let mut stmt =
                tx.prepare(&format!("SELECT {EVENT_COLUMNS} FROM events ORDER BY id"))?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                fold(&tx, &event_from_row(row)?)?;
            }
        }
        for (id, until, beat) in leases {
            tx.execute(
                "UPDATE lanes SET lease_expires_at = ?2, last_heartbeat_at = ?3 WHERE id = ?1",
                params![id, until, beat],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log this binary did not write (a lane column the write refuses to
    /// set, a lane no event created) fails the rebuild, and the tables stay
    /// as they were: the seeded `blocked` is not what a refold would write.
    #[test]
    fn rebuild_refuses_a_foreign_log() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let conn = ledger.conn();
        conn.execute_batch(
            "INSERT INTO events (lane_id, kind, payload_json, at)
             VALUES (NULL, 'lane.created',
                     '{\"title\":\"t\",\"source\":\"manual\",\"kind\":\"code\",\"pr_flow\":\"pr\"}',
                     '2026-11-03T00:00:00.000Z');
             INSERT INTO lanes (id, title, source, kind, pr_flow, status, head_event_id,
                                created_at, updated_at)
             VALUES (1, 't', 'manual', 'code', 'pr', 'blocked', 1,
                     '2026-11-03T00:00:00.000Z', '2026-11-03T00:00:00.000Z');",
        )
        .unwrap();
        for (lane, payload) in [
            (1, r#"{"changed":{"lease_secs":{"from":null,"to":60}}}"#),
            (
                9,
                r#"{"changed":{"status":{"from":"queued","to":"active"}}}"#,
            ),
        ] {
            conn.execute(
                "INSERT INTO events (id, lane_id, kind, payload_json, at)
                 VALUES (2, ?1, 'lane.updated', ?2, '2026-11-03T00:00:00.000Z')",
                params![lane, payload],
            )
            .unwrap();
            assert!(ledger.rebuild().is_err(), "{payload}");
            let status: String = conn
                .query_row("SELECT status FROM lanes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(status, "blocked", "{payload}");
            conn.execute("DELETE FROM events WHERE id = 2", []).unwrap();
        }
    }
}
