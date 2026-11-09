//! A repo's backlog priority (#119): the owner's order of its open issues,
//! one `backlog.prioritised` per change, the newest one standing. On a
//! repo, never a lane, like `dossier.noted`.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension};
use serde_json::json;

use crate::checks::{Recorded, recorded};
use crate::ledger::{Actor, Checked, EventKind, Ledger, LedgerError, Mutation, Payload, Refusal};
use crate::work::pr_ref_ok;

/// Keys an order holds at most: one page of daisy's mirror.
pub const ORDER_CAP: usize = 500;

/// A repo's orders. Matches the `events_backlog` index (migration 0005)
/// term for term, or lookups scan every event.
const ORDERS: &str = "kind = 'backlog.prioritised' AND json_extract(payload_json, '$.repo') = ?1";

/// The newest order on the (lowercased) repo: its event id and keys.
fn latest(conn: &Connection, repo: &str) -> Result<Option<(i64, Vec<String>)>, LedgerError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            &format!(
                "SELECT id, json_extract(payload_json, '$.order') FROM events
                  WHERE {ORDERS} ORDER BY id DESC LIMIT 1"
            ),
            [repo],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(id, order)| {
        Ok((
            id,
            serde_json::from_str(&order).map_err(std::io::Error::other)?,
        ))
    })
    .transpose()
}

impl Ledger {
    /// One `backlog.prioritised`: `repo`'s issue keys (`owner/repo#N`),
    /// first first. `after` is the id of the order the list was built
    /// from, None when there was none, so an order written since is a
    /// conflict, never lost.
    pub fn backlog_prioritise(
        &self,
        actor: &Actor,
        repo: &str,
        order: &[String],
        after: Option<i64>,
    ) -> Checked<Recorded> {
        let repo = repo.trim().to_lowercase();
        let order: Vec<String> = order.iter().map(|k| k.trim().to_lowercase()).collect();
        let here = format!("{repo}#");
        let mut seen = HashSet::new();
        if !pr_ref_ok(&format!("{repo}#1"))
            || order.len() > ORDER_CAP
            || !order
                .iter()
                .all(|k| k.starts_with(&here) && pr_ref_ok(k) && seen.insert(k))
        {
            return Ok(Err(Refusal::InvalidInput(format!(
                "an order is at most {ORDER_CAP} distinct issue keys of one owner/repo, \
                 each owner/repo#N"
            ))));
        }
        let mut payload = Payload {
            repo: Some(repo.clone()),
            ..Payload::default()
        };
        payload.rest.insert("order".into(), json!(order));
        payload.rest.insert("after".into(), json!(after));
        let (applied, _) = self.apply_planned(
            actor,
            None,
            None,
            false,
            |conn, _| {
                if latest(conn, &repo)?.map(|(id, _)| id) != after {
                    return Ok(Err(Refusal::Conflict {
                        entity: "backlog",
                        message: format!("{repo}'s order changed since it was read; reload"),
                    }));
                }
                Ok(Ok(Mutation::Event {
                    kind: EventKind::BacklogPrioritised,
                    lane_id: None,
                    spec_id: None,
                    payload,
                }))
            },
            |_, _, _| Ok(()),
        )?;
        Ok(recorded(applied, None))
    }

    /// `repo`'s standing order: the newest `backlog.prioritised`'s id and
    /// keys, or none and no keys.
    pub fn backlog_priority(&self, repo: &str) -> Result<(Option<i64>, Vec<String>), LedgerError> {
        Ok(match latest(self.conn(), &repo.to_lowercase())? {
            Some((id, order)) => (Some(id), order),
            None => (None, Vec::new()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order lookup searches the index.
    #[test]
    fn order_lookup_uses_index() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let sql = format!("EXPLAIN QUERY PLAN SELECT id FROM events WHERE {ORDERS}");
        let plan: String = ledger
            .conn()
            .query_row(&sql, ["o/r"], |r| r.get(3))
            .unwrap();
        assert!(plan.contains("USING INDEX events_backlog"), "{plan}");
    }

    /// An order built from one that is no longer the newest is refused and
    /// writes nothing; one built from the newest is written and stands.
    #[test]
    fn stale_after_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let actor = Actor {
            agent: Some("owner-ui".into()),
            session: Some("ui:1".into()),
        };
        let keys = |k: &[&str]| k.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let first = ledger
            .backlog_prioritise(&actor, "O/R", &keys(&["O/R#2", "o/r#1"]), None)
            .unwrap()
            .unwrap();
        // Another window read no order, or the first, before it was written.
        let stale = ledger.backlog_prioritise(&actor, "o/r", &keys(&["o/r#1"]), None);
        assert!(matches!(stale.unwrap(), Err(Refusal::Conflict { .. })));
        let wrong = ledger.backlog_prioritise(&actor, "o/r", &keys(&["o/r#1"]), Some(0));
        assert!(matches!(wrong.unwrap(), Err(Refusal::Conflict { .. })));
        assert_eq!(
            ledger.backlog_priority("o/r").unwrap(),
            (Some(first.event_id), keys(&["o/r#2", "o/r#1"]))
        );
        let fresh = ledger
            .backlog_prioritise(
                &actor,
                "o/r",
                &keys(&["o/r#1", "o/r#2"]),
                Some(first.event_id),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            ledger.backlog_priority("O/R").unwrap(),
            (Some(fresh.event_id), keys(&["o/r#1", "o/r#2"]))
        );
        // Another repo's key, or one twice, is no order.
        for bad in [&["o/x#1"][..], &["o/r#1", "o/r#1"]] {
            let r = ledger.backlog_prioritise(&actor, "o/r", &keys(bad), Some(fresh.event_id));
            assert!(matches!(r.unwrap(), Err(Refusal::InvalidInput(_))));
        }
        assert_eq!(ledger.backlog_priority("o/x").unwrap(), (None, Vec::new()));
    }
}
