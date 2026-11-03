//! The one write function. Every change to the ledger is one `BEGIN
//! IMMEDIATE` transaction that checks, appends one event and folds it, so
//! the log and the tables commit together or not at all. Nothing that waits
//! runs inside it. A refusal returns before the commit, and dropping the
//! transaction rolls back whatever it touched.

use std::collections::BTreeSet;

use rusqlite::types::ValueRef;
use rusqlite::{OptionalExtension, Params, Row, Transaction, TransactionBehavior};
use serde_json::Value;

use super::event::{Actor, Applied, Event, EventKind, Mutation, Payload, Refusal};
use super::{Ledger, LedgerError, check_schema, fold};

const MIN_LEASE_SECS: i64 = 30;
const MAX_LEASE_SECS: i64 = 86_400;
const STATUSES: &[&str] = &[
    "queued",
    "claimed",
    "active",
    "blocked",
    "review",
    "done",
    "cancelled",
];

type Checked<T> = Result<Result<T, Refusal>, LedgerError>;

macro_rules! refuse {
    ($r:expr) => {
        return Ok(Err($r))
    };
}

/// Unwraps a check, returning its refusal from the enclosing function.
macro_rules! check {
    ($e:expr) => {
        match $e? {
            Ok(v) => v,
            Err(r) => return Ok(Err(r)),
        }
    };
}

impl Ledger {
    /// `at` is the caller's clock, RFC 3339 UTC with milliseconds
    /// (`2026-11-03T05:35:23.321Z`); the write never reads the wall clock.
    /// `key` is the idempotency key: a stored event with the same key is
    /// returned as it was, whatever the new payload says.
    pub fn apply(
        &self,
        actor: &Actor,
        key: Option<&str>,
        at: &str,
        mutation: Mutation,
    ) -> Result<Applied, LedgerError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        check_schema(&tx)?;
        let canonical: bool = tx.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1) IS ?1",
            [at],
            |r| r.get(0),
        )?;
        if !canonical {
            return Ok(Applied::Refused(Refusal::InvalidInput(format!(
                "at {at} is not RFC 3339 UTC with milliseconds"
            ))));
        }
        let applied = match mutation {
            Mutation::Heartbeat { lane_id } => {
                if key.is_some() {
                    return Ok(Applied::Refused(Refusal::InvalidInput(
                        "a heartbeat takes no idempotency key".into(),
                    )));
                }
                match heartbeat(&tx, actor, at, lane_id)? {
                    Ok(until) => Applied::Heartbeat {
                        lane_id,
                        lease_expires_at: until,
                    },
                    Err(r) => return Ok(Applied::Refused(r)),
                }
            }
            Mutation::Event {
                kind,
                lane_id,
                spec_id,
                payload,
            } => {
                if let Some(event) = key.map(|k| by_key(&tx, k)).transpose()?.flatten() {
                    return Ok(Applied::Replayed(event));
                }
                let mut event = Event {
                    id: 0,
                    kind,
                    lane_id,
                    spec_id,
                    actor: actor.clone(),
                    idempotency_key: key.map(String::from),
                    payload,
                    at: at.into(),
                };
                if let Err(r) = guard(&tx, &mut event)? {
                    return Ok(Applied::Refused(r));
                }
                event.id = insert(&tx, &event)?;
                fold::fold(&tx, &event)?;
                Applied::Committed(event)
            }
        };
        tx.commit()?;
        Ok(applied)
    }
}

/// Extends the lease by the lane's own `lease_secs`. Exact holder pair, not
/// a live lease: a holder back from a stall renews instead of re-claiming.
/// Another pair's lease, live or not, is held.
fn heartbeat(tx: &Transaction, actor: &Actor, at: &str, lane_id: i64) -> Checked<String> {
    let ((agent, session, secs, until), same_pair) = check!(holder(tx, actor, lane_id));
    if session.is_some() && !same_pair {
        refuse!(lease_held(agent, session, until));
    }
    let Some(secs) = secs.filter(|_| same_pair) else {
        refuse!(Refusal::InvalidInput(format!(
            "lane {lane_id} is not held by this agent and session"
        )));
    };
    let until = tx.query_row(
        "UPDATE lanes SET
             lease_expires_at = strftime('%Y-%m-%dT%H:%M:%fZ', ?1, '+' || ?2 || ' seconds'),
             last_heartbeat_at = ?1
         WHERE id = ?3 RETURNING lease_expires_at",
        rusqlite::params![at, secs, lane_id],
        |r| r.get(0),
    )?;
    Ok(Ok(until))
}

/// Every check an event must pass before it is appended: ids it names
/// exist, keys it takes are free, the fields its kind folds are there,
/// `changed.from` is the current value, and a live lease held by another
/// pair is respected. Only lease kinds carry a lease or set the holder, and
/// they leave the actor holding it. Sets the ids the write assigns and the
/// lease expiry.
fn guard(tx: &Transaction, event: &mut Event) -> Checked<()> {
    use EventKind::*;
    let p = &event.payload;
    let stored = serde_json::to_string(p).map_err(std::io::Error::other)?;
    if serde_json::from_str::<Payload>(&stored).ok().as_ref() != Some(p) {
        refuse!(invalid(
            "payload does not survive a round trip; a rest key repeats a typed field"
        ));
    }
    if !event.kind.leases() && (p.lease_secs.is_some() || p.lease_expires_at.is_some()) {
        refuse!(invalid(&format!("{} carries no lease", event.kind)));
    }
    match event.kind {
        SpecCreated | SpecRevised | SpecApproved
            if event.lane_id.is_some() || !p.changed.is_empty() =>
        {
            refuse!(invalid(
                "spec events name no lane and change no lane column"
            ));
        }
        SpecCreated => {
            let (Some(key), Some(_), Some(_)) = (&p.parent_key, &p.title, &p.body) else {
                refuse!(invalid("spec.created needs parent_key, title and body"));
            };
            if event.spec_id.is_some() || p.version.is_some_and(|v| v != 1) {
                refuse!(invalid("spec.created assigns the spec id and version 1"));
            }
            if exists(tx, "SELECT 1 FROM specs WHERE external_key = ?1", [key])? {
                refuse!(Refusal::Conflict(format!("spec {key} exists")));
            }
            event.spec_id = Some(next_id(tx, "specs")?);
        }
        SpecRevised | SpecApproved => {
            let id = check!(need_id(event.spec_id, "spec"));
            let head = check!(spec_head(tx, id));
            let revised = event.kind == SpecRevised;
            if revised && p.body.is_none() {
                refuse!(invalid("spec.revised needs a body"));
            }
            let want = head + i64::from(revised);
            if p.version != Some(want) {
                refuse!(invalid(&format!("{} needs version {want}", event.kind)));
            }
        }
        LaneCreated => {
            if event.lane_id.is_some() || !p.changed.is_empty() {
                refuse!(invalid(
                    "lane.created assigns the lane id and changes no column"
                ));
            }
            if let Some(spec) = event.spec_id {
                check!(spec_head(tx, spec));
                if let Some(lane_key) = &p.lane_key
                    && exists(
                        tx,
                        "SELECT 1 FROM lanes WHERE spec_id = ?1 AND lane_key = ?2",
                        rusqlite::params![spec, lane_key],
                    )?
                {
                    refuse!(Refusal::Conflict(format!(
                        "lane {lane_key} exists on spec {spec}"
                    )));
                }
            }
            event.lane_id = Some(next_id(tx, "lanes")?);
        }
        _ => {
            if event.spec_id.is_some() {
                refuse!(invalid("only spec events and lane.created name a spec"));
            }
            let id = check!(need_id(event.lane_id, "lane"));
            check!(lane_guard(tx, event, id));
        }
    }
    Ok(Ok(()))
}

fn lane_guard(tx: &Transaction, event: &mut Event, lane_id: i64) -> Checked<()> {
    use EventKind::*;
    let ((agent, session, _, until), same_pair) = check!(holder(tx, &event.actor, lane_id));
    let live = until.as_deref().is_some_and(|u| u > event.at.as_str());
    let p = &mut event.payload;
    // Checks and the ready override come from other sessions, and a
    // takeover is the way past a live lease, so those kinds skip the holder
    // rule. A claim keeps it: over an expired lease its `changed.from` pins
    // the holder it replaces.
    let holder_rule = !matches!(
        event.kind,
        LaneTakenOver
            | LaneReadyOverridden
            | CheckRequested
            | CheckFinished
            | FindingUpdated
            | GateDenied
    );
    // Their exemption must not reach the row: these kinds change no column.
    if event.kind.keeps_updated_at() && !p.changed.is_empty() {
        refuse!(invalid(&format!("{} changes no lane column", event.kind)));
    }
    // A renewal names the lease it extends, so another pair's is held even
    // once it has expired.
    if session.is_some() && !same_pair && (holder_rule && live || event.kind == LaneRenewed) {
        refuse!(lease_held(agent, session, until));
    }
    if event.kind == LaneRenewed && !same_pair {
        refuse!(invalid("only the holder renews its lease"));
    }
    if event.kind.leases() {
        if event.actor.session.is_none() {
            refuse!(invalid(&format!("{} needs an agent session", event.kind)));
        }
        let secs = p.lease_secs.unwrap_or(0);
        if !(MIN_LEASE_SECS..=MAX_LEASE_SECS).contains(&secs) {
            refuse!(invalid(&format!(
                "lease_secs must be {MIN_LEASE_SECS} to {MAX_LEASE_SECS}"
            )));
        }
        // The lease is the actor's: after the write the holder is its pair.
        let pair = (
            after(tx, lane_id, p, "agent")?,
            after(tx, lane_id, p, "session")?,
        );
        let actor = &event.actor;
        if pair != (actor.agent.clone().into(), actor.session.clone().into()) {
            refuse!(invalid(&format!(
                "{} must leave this agent and session holding the lane",
                event.kind
            )));
        }
        p.lease_expires_at = Some(tx.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1, '+' || ?2 || ' seconds')",
            rusqlite::params![event.at, secs],
            |r| r.get(0),
        )?);
    }
    for (field, change) in &p.changed {
        if !fold::LANE_COLUMNS.contains(&field.as_str()) {
            refuse!(invalid(&format!("lanes has no changeable column {field}")));
        }
        if field == "status" && !change.to.as_str().is_some_and(|s| STATUSES.contains(&s)) {
            refuse!(invalid(&format!(
                "status {} is not a wire status",
                change.to
            )));
        }
        let integer = fold::INTEGER_COLUMNS.contains(&field.as_str());
        let typed = if integer {
            change.to.as_i64().is_some()
        } else {
            change.to.is_string()
        };
        if !typed && !change.to.is_null() {
            refuse!(invalid(&format!(
                "{field} must be {} or null",
                if integer { "an integer" } else { "text" }
            )));
        }
        // Only a lease sets the holder; other kinds may only clear it.
        if matches!(field.as_str(), "agent" | "session")
            && !event.kind.leases()
            && !change.to.is_null()
        {
            refuse!(invalid(&format!("{} only clears {field}", event.kind)));
        }
        let current = lane_value(tx, lane_id, field)?;
        if current != change.from {
            refuse!(Refusal::Conflict(format!(
                "{field} is {current}, not {}",
                change.from
            )));
        }
        if field == "spec_id"
            && let Some(spec) = change.to.as_i64()
        {
            check!(spec_head(tx, spec));
        }
    }
    // The (spec_id, lane_key) pair is unique; refuse here, not as a SQLite error.
    if p.changed.contains_key("spec_id") || p.changed.contains_key("lane_key") {
        let (spec, key) = (
            after(tx, lane_id, p, "spec_id")?,
            after(tx, lane_id, p, "lane_key")?,
        );
        if exists(
            tx,
            "SELECT 1 FROM lanes WHERE spec_id = ?1 AND lane_key = ?2 AND id != ?3",
            rusqlite::params![fold::sql_value(&spec), fold::sql_value(&key), lane_id],
        )? {
            refuse!(Refusal::Conflict(format!(
                "lane {key} exists on spec {spec}"
            )));
        }
    }
    match event.kind {
        CheckFinished => {
            if p.check.is_none() || p.head_sha.is_none() {
                refuse!(invalid("check.finished needs check and head_sha"));
            }
            if !matches!(p.outcome.as_deref(), Some("ran" | "skipped")) {
                refuse!(invalid("check.finished outcome is ran or skipped"));
            }
            let findings = p.finding_list();
            let keys: BTreeSet<_> = findings.iter().map(|f| &f.finding_key).collect();
            if keys.len() != findings.len() {
                refuse!(invalid("finding keys repeat"));
            }
        }
        FindingUpdated => {
            let (Some(check), Some(key)) = (p.check_event_id, &p.finding_key) else {
                refuse!(invalid(
                    "finding.updated needs check_event_id and finding_key"
                ));
            };
            if !matches!(p.status.as_deref(), Some("fixed" | "answered")) {
                refuse!(invalid("finding.updated status is fixed or answered"));
            }
            if !exists(
                tx,
                "SELECT 1 FROM findings
                 WHERE lane_id = ?1 AND check_event_id = ?2 AND finding_key = ?3",
                rusqlite::params![lane_id, check, key],
            )? {
                refuse!(Refusal::NotFound {
                    entity: "finding",
                    id: check
                });
            }
        }
        _ => {}
    }
    Ok(Ok(()))
}

fn invalid(msg: &str) -> Refusal {
    Refusal::InvalidInput(msg.into())
}

fn need_id(id: Option<i64>, entity: &str) -> Checked<i64> {
    Ok(id.ok_or_else(|| invalid(&format!("this event needs a {entity} id"))))
}

fn spec_head(tx: &Transaction, id: i64) -> Checked<i64> {
    let head = tx
        .query_row("SELECT head_version FROM specs WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(head.ok_or(Refusal::NotFound { entity: "spec", id }))
}

fn exists(tx: &Transaction, sql: &str, args: impl Params) -> Result<bool, LedgerError> {
    Ok(tx.query_row(sql, args, |_| Ok(())).optional()?.is_some())
}

type Holder = (Option<String>, Option<String>, Option<i64>, Option<String>);

/// The lane's agent, session, lease_secs and lease_expires_at, and whether
/// `actor` is exactly that holder pair.
fn holder(tx: &Transaction, actor: &Actor, lane_id: i64) -> Checked<(Holder, bool)> {
    let lane: Option<Holder> = tx
        .query_row(
            "SELECT agent, session, lease_secs, lease_expires_at FROM lanes WHERE id = ?1",
            [lane_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some(h) = lane else {
        refuse!(Refusal::NotFound {
            entity: "lane",
            id: lane_id
        });
    };
    let same_pair = h.1.is_some() && (&h.0, &h.1) == (&actor.agent, &actor.session);
    Ok(Ok((h, same_pair)))
}

/// A held lease, `conflict` with `by` and `until` in the CLI contract.
fn lease_held(agent: Option<String>, session: Option<String>, until: Option<String>) -> Refusal {
    Refusal::LeaseHeld {
        by: format!(
            "{}/{}",
            agent.unwrap_or_default(),
            session.unwrap_or_default()
        ),
        until: until.unwrap_or_default(),
    }
}

fn lane_value(tx: &Transaction, lane_id: i64, field: &str) -> Result<Value, LedgerError> {
    Ok(tx.query_row(
        &format!("SELECT {field} FROM lanes WHERE id = ?1"),
        [lane_id],
        |r| Ok(to_json(r.get_ref(0)?)),
    )?)
}

/// A lane column once the event folds: its `changed.to`, else the row's value.
fn after(tx: &Transaction, lane_id: i64, p: &Payload, field: &str) -> Result<Value, LedgerError> {
    match p.changed.get(field) {
        Some(change) => Ok(change.to.clone()),
        None => lane_value(tx, lane_id, field),
    }
}

/// Plain ids, assigned under the write lock.
fn next_id(tx: &Transaction, table: &str) -> Result<i64, LedgerError> {
    Ok(tx.query_row(
        &format!("SELECT coalesce(max(id), 0) + 1 FROM {table}"),
        [],
        |r| r.get(0),
    )?)
}

fn to_json(v: ValueRef) -> Value {
    match v {
        ValueRef::Integer(i) => i.into(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
        _ => Value::Null,
    }
}

fn insert(tx: &Transaction, e: &Event) -> Result<i64, LedgerError> {
    let payload = serde_json::to_string(&e.payload).map_err(std::io::Error::other)?;
    Ok(tx.query_row(
        "INSERT INTO events
           (lane_id, spec_id, kind, actor_agent, actor_session, idempotency_key, payload_json, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
        rusqlite::params![
            e.lane_id,
            e.spec_id,
            e.kind.as_str(),
            e.actor.agent,
            e.actor.session,
            e.idempotency_key,
            payload,
            e.at,
        ],
        |r| r.get(0),
    )?)
}

pub(super) const EVENT_COLUMNS: &str =
    "id, kind, lane_id, spec_id, actor_agent, actor_session, idempotency_key, payload_json, at";

/// Reads an `EVENT_COLUMNS` row. A kind or payload this binary cannot read
/// is an error, never a skipped event.
pub(super) fn event_from_row(r: &Row) -> Result<Event, LedgerError> {
    let kind: String = r.get(1)?;
    let payload: String = r.get(7)?;
    Ok(Event {
        id: r.get(0)?,
        kind: kind.parse().map_err(std::io::Error::other)?,
        lane_id: r.get(2)?,
        spec_id: r.get(3)?,
        actor: Actor {
            agent: r.get(4)?,
            session: r.get(5)?,
        },
        idempotency_key: r.get(6)?,
        payload: serde_json::from_str(&payload).map_err(std::io::Error::other)?,
        at: r.get(8)?,
    })
}

fn by_key(tx: &Transaction, key: &str) -> Result<Option<Event>, LedgerError> {
    let mut stmt = tx.prepare(&format!(
        "SELECT {EVENT_COLUMNS} FROM events WHERE idempotency_key = ?1"
    ))?;
    let mut rows = stmt.query([key])?;
    rows.next()?.map(event_from_row).transpose()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ledger::{Change, Finding, Findings};

    const AT: &str = "2026-11-03T00:00:00.000Z";

    fn ev(
        kind: EventKind,
        lane_id: Option<i64>,
        spec_id: Option<i64>,
        payload: Payload,
    ) -> Mutation {
        Mutation::Event {
            kind,
            lane_id,
            spec_id,
            payload,
        }
    }

    fn change(field: &str, from: serde_json::Value, to: serde_json::Value) -> Payload {
        Payload {
            changed: [(field.to_string(), Change { from, to })].into(),
            ..Payload::default()
        }
    }

    fn finding(key: &str) -> Finding {
        Finding {
            finding_key: key.into(),
            severity: "nit".into(),
            title: "t".into(),
            body: "b".into(),
            path: None,
            rest: Default::default(),
        }
    }

    /// Each refusal branch of the guard writes no event.
    #[test]
    fn refusals_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let me = Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        };
        let spec = Payload {
            parent_key: Some("Flexipie/harness#1".into()),
            title: Some("t".into()),
            body: Some("b".into()),
            ..Payload::default()
        };
        let other = Actor {
            agent: Some("b".into()),
            session: Some("t".into()),
        };
        let no_session = Actor {
            session: None,
            ..me.clone()
        };
        let lease = Payload {
            lease_secs: Some(60),
            ..Payload::default()
        };
        // Lane 2 is on spec 1 as L, held by the other pair.
        let mut held = change("session", json!(null), json!("t"));
        held.changed
            .extend(change("agent", json!(null), json!("b")).changed);
        held.lease_secs = Some(60);
        for (who, m) in [
            (&me, ev(EventKind::SpecCreated, None, None, spec.clone())),
            (
                &me,
                ev(EventKind::LaneCreated, None, Some(1), Payload::default()),
            ),
            (
                &me,
                ev(
                    EventKind::LaneCreated,
                    None,
                    Some(1),
                    Payload {
                        lane_key: Some("L".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (&other, ev(EventKind::LaneClaimed, Some(2), None, held)),
        ] {
            assert!(matches!(
                ledger.apply(who, None, AT, m).unwrap(),
                Applied::Committed(_)
            ));
        }
        let mut dup = Payload::default();
        dup.rest.insert("status".into(), json!("x"));
        dup.status = Some("y".into());
        let cases: Vec<(&str, Option<&str>, Mutation)> = vec![
            (
                "2026-11-03 00:00:00",
                None,
                ev(EventKind::LaneUpdated, Some(1), None, Payload::default()),
            ),
            (AT, Some("k"), Mutation::Heartbeat { lane_id: 1 }),
            (AT, None, Mutation::Heartbeat { lane_id: 9 }),
            (AT, None, Mutation::Heartbeat { lane_id: 1 }),
            (AT, None, Mutation::Heartbeat { lane_id: 2 }),
            (
                AT,
                None,
                ev(EventKind::LaneReadyOverridden, Some(1), None, dup),
            ),
            (
                AT,
                None,
                ev(EventKind::SpecCreated, None, None, Payload::default()),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::SpecCreated,
                    None,
                    None,
                    Payload {
                        version: Some(2),
                        ..spec.clone()
                    },
                ),
            ),
            (AT, None, ev(EventKind::SpecCreated, None, None, spec)),
            (
                AT,
                None,
                ev(
                    EventKind::SpecApproved,
                    Some(1),
                    Some(1),
                    Payload {
                        version: Some(1),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::SpecApproved,
                    None,
                    Some(1),
                    Payload {
                        version: Some(1),
                        lease_secs: Some(60),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(EventKind::SpecRevised, None, None, Payload::default()),
            ),
            (
                AT,
                None,
                ev(EventKind::SpecApproved, None, Some(9), Payload::default()),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::SpecRevised,
                    None,
                    Some(1),
                    Payload {
                        version: Some(2),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::SpecRevised,
                    None,
                    Some(1),
                    Payload {
                        version: Some(3),
                        body: Some("b".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(EventKind::LaneCreated, Some(1), None, Payload::default()),
            ),
            (
                AT,
                None,
                ev(EventKind::LaneCreated, None, Some(9), Payload::default()),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    Payload {
                        lease_expires_at: Some(AT.into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(EventKind::LaneUpdated, Some(1), Some(1), Payload::default()),
            ),
            (
                AT,
                None,
                ev(EventKind::LaneUpdated, None, None, Payload::default()),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneRenewed,
                    Some(1),
                    None,
                    Payload {
                        lease_secs: Some(60),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(EventKind::LaneRenewed, Some(2), None, lease.clone()),
            ),
            (
                AT,
                None,
                ev(EventKind::LaneTakenOver, Some(2), None, lease.clone()),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    Payload {
                        lease_secs: Some(60),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("updated_at", json!(null), json!("x")),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("status", json!("queued"), json!("ready")),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("pid", json!(null), json!(true)),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("branch", json!(null), json!(1)),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("spec_id", json!(1), json!("x")),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("spec_id", json!(1), json!(9)),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("lane_key", json!(null), json!("L")),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::LaneUpdated,
                    Some(1),
                    None,
                    change("agent", json!(null), json!("a")),
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::CheckFinished,
                    Some(1),
                    None,
                    change("status", json!("queued"), json!("done")),
                ),
            ),
            (
                AT,
                None,
                ev(EventKind::CheckFinished, Some(1), None, Payload::default()),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::CheckFinished,
                    Some(1),
                    None,
                    Payload {
                        check: Some("smoke".into()),
                        head_sha: Some("abc1234".into()),
                        outcome: Some("passed".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::CheckFinished,
                    Some(1),
                    None,
                    Payload {
                        check: Some("smoke".into()),
                        head_sha: Some("abc1234".into()),
                        outcome: Some("ran".into()),
                        findings: Some(Findings::List(vec![finding("f"), finding("f")])),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(EventKind::FindingUpdated, Some(1), None, Payload::default()),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::FindingUpdated,
                    Some(1),
                    None,
                    Payload {
                        check_event_id: Some(1),
                        finding_key: Some("f".into()),
                        status: Some("open".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                AT,
                None,
                ev(
                    EventKind::FindingUpdated,
                    Some(1),
                    None,
                    Payload {
                        check_event_id: Some(1),
                        finding_key: Some("f".into()),
                        status: Some("fixed".into()),
                        ..Payload::default()
                    },
                ),
            ),
        ];
        let events = |l: &Ledger| -> i64 {
            l.conn()
                .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
                .unwrap()
        };
        let cases = cases
            .into_iter()
            .map(|(at, key, m)| (&me, at, key, m))
            .chain([(
                &no_session,
                AT,
                None,
                ev(EventKind::LaneClaimed, Some(1), None, lease),
            )]);
        for (i, (who, at, key, m)) in cases.enumerate() {
            let applied = ledger.apply(who, key, at, m).unwrap();
            assert!(
                matches!(applied, Applied::Refused(_)),
                "case {i}: {applied:?}"
            );
            assert_eq!(events(&ledger), 4, "case {i}");
        }
    }
}
