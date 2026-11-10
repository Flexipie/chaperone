//! The one write function. Every change to the ledger is one `BEGIN
//! IMMEDIATE` transaction that checks, appends one event and folds it, so
//! the log and the tables commit together or not at all. Nothing that waits
//! runs inside it. A refusal returns before the commit, and dropping the
//! transaction rolls back whatever it touched.

use std::collections::BTreeSet;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, Params, Row, Transaction, TransactionBehavior};
use serde_json::Value;

use super::event::{Actor, Applied, Event, EventKind, Mutation, Payload, Refusal};
use super::{Ledger, LedgerError, check_schema, fold, redact};
use crate::checks::{NOTE_CHARS, one_line, sha_ok};
use crate::engine;
use crate::pipeline::{REQUIRED_TRANSITIONS, Route};
use crate::work::{OWNER_UI, PAUSED_BY_OWNER, SPEC_BY_KEY};

pub(crate) const MIN_LEASE_SECS: i64 = 30;
pub(crate) const MAX_LEASE_SECS: i64 = 86_400;
pub const STATUSES: &[&str] = &[
    "queued",
    "claimed",
    "active",
    "blocked",
    "review",
    "done",
    "cancelled",
];

/// The checks a session produces (events.md, `check.finished` payloads).
pub const SESSION_CHECKS: &[&str] = &[
    "plan_check",
    "quality_gate",
    "merge_verdict",
    "smoke",
    "kept_tests",
    "review_round",
];

/// Who may make a finding move (events.md, finding statuses). The lane's
/// only move on a finding is to dispute it, so it never settles its own.
enum By {
    /// Anyone, for a `question` only (the guard checks the severity).
    Anyone,
    /// A session that has held the lane.
    Lane,
    /// A session that never has: the verifier, or the owner (the ready
    /// override's rule, cli-contract.md).
    Independent,
}

/// Every move `finding.updated` may make; a status no row leaves is final.
const FINDING_MOVES: &[(&str, &str, By)] = &[
    ("open", "fixed", By::Independent),
    ("open", "answered", By::Anyone),
    ("verified", "fixed", By::Independent),
    ("open", "verified", By::Independent),
    ("open", "refuted", By::Independent),
    ("disputed", "verified", By::Independent),
    ("disputed", "refuted", By::Independent),
    ("open", "waived", By::Independent),
    ("verified", "waived", By::Independent),
    ("disputed", "waived", By::Independent),
    ("open", "disputed", By::Lane),
    ("verified", "disputed", By::Lane),
];

pub const SOURCES: &[&str] = &["linear", "github", "note", "issue", "manual"];
pub const KINDS: &[&str] = &["code", "docs", "measure", "investigation"];
pub const PR_FLOWS: &[&str] = &["pr", "direct", "none"];
/// A spec body is a document a person reads; this is far above any real one.
pub const SPEC_BODY_MAX_BYTES: usize = 256 * 1024;

pub(crate) type Checked<T> = Result<Result<T, Refusal>, LedgerError>;

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
    /// (`2026-11-03T05:35:23.321Z`); given one, the write never reads the
    /// wall clock. `key` is the idempotency key: a stored event of the same
    /// kind with that key is returned as it was, whatever the new payload
    /// says; another kind's key is `invalid_input`.
    pub fn apply(
        &self,
        actor: &Actor,
        key: Option<&str>,
        at: &str,
        mutation: Mutation,
    ) -> Result<Applied, LedgerError> {
        Ok(self
            .apply_with(actor, key, at, mutation, false, |_, _| Ok(()))?
            .0)
    }

    /// [`Ledger::apply`], then `read` on the same transaction before it
    /// commits, so the caller sees the rows the event left. `read` runs on
    /// a committed, replayed or heartbeat write, never on a refusal, and an
    /// error from it rolls the write back. With `dry_run` nothing commits.
    pub fn apply_with<T>(
        &self,
        actor: &Actor,
        key: Option<&str>,
        at: &str,
        mutation: Mutation,
        dry_run: bool,
        read: impl FnOnce(&Connection, &Applied) -> Result<T, LedgerError>,
    ) -> Result<(Applied, Option<T>), LedgerError> {
        self.apply_planned(
            actor,
            key,
            Some(at),
            dry_run,
            |_, _| Ok(Ok(mutation)),
            |conn, applied, _| read(conn, applied),
        )
    }

    /// [`Ledger::apply_with`] for a command whose event depends on the row:
    /// `plan` builds the mutation inside the transaction, after the schema
    /// and actor checks, so what it read cannot change before the write.
    /// With `at` None the clock is read once the write lock is held and
    /// moved past the newest event's `at` ([`write_clock`]), so clock-timed
    /// events, and each lane's `updated_at`, only go up. `read` gets that
    /// `at`.
    pub(crate) fn apply_planned<T>(
        &self,
        actor: &Actor,
        key: Option<&str>,
        at: Option<&str>,
        dry_run: bool,
        plan: impl FnOnce(&Connection, &str) -> Checked<Mutation>,
        read: impl FnOnce(&Connection, &Applied, &str) -> Result<T, LedgerError>,
    ) -> Result<(Applied, Option<T>), LedgerError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        check_schema(&tx)?;
        if let Err(r) = actor.check() {
            return Ok((Applied::Refused(r), None));
        }
        let at = match at {
            Some(at) => at.to_string(),
            None => write_clock(&tx)?,
        };
        let applied = match plan(&tx, &at)? {
            Ok(mutation) => write(&tx, actor, key, &at, mutation)?,
            Err(r) => Applied::Refused(r),
        };
        if matches!(applied, Applied::Refused(_)) {
            return Ok((applied, None));
        }
        let out = read(&tx, &applied, &at)?;
        if !dry_run {
            tx.commit()?;
        }
        Ok((applied, Some(out)))
    }
}

/// The clock for a write that passed none: now, or 1 ms past the newest
/// event when now is not later (two commits in one millisecond, or a wall
/// clock that stepped back). `--expected-updated-at` compares these
/// strings, so a write in between always shows. One seek by rowid.
fn write_clock(tx: &Transaction) -> Result<String, LedgerError> {
    const MS: &str = "'%Y-%m-%dT%H:%M:%fZ'";
    Ok(tx.query_row(
        &format!(
            "SELECT max(strftime({MS}, 'now'), coalesce(strftime({MS},
               (SELECT at FROM events ORDER BY id DESC LIMIT 1), '+0.001 seconds'), ''))"
        ),
        [],
        |r| r.get(0),
    )?)
}

fn write(
    tx: &Transaction,
    actor: &Actor,
    key: Option<&str>,
    at: &str,
    mutation: Mutation,
) -> Result<Applied, LedgerError> {
    if !canonical_at(tx, at)? {
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
            match heartbeat(tx, actor, at, lane_id)? {
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
            // Before the lookup: a key is stored, and compared, as it is.
            if let Some(found) = key.and_then(redact::secret) {
                return Ok(Applied::Refused(invalid(&format!(
                    "the idempotency key looks like a {found}; it is stored as it is"
                ))));
            }
            if let Some(event) = key.map(|k| by_key(tx, k)).transpose()?.flatten() {
                // Scoped to the kind, so one command's key never answers another's.
                if event.kind != kind {
                    return Ok(Applied::Refused(invalid(&format!(
                        "that idempotency key belongs to a {} event",
                        event.kind
                    ))));
                }
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
            if let Err(r) = guard(tx, &mut event)? {
                return Ok(Applied::Refused(r));
            }
            if let (EventKind::PhaseEntered, Some(lane)) = (event.kind, event.lane_id) {
                match engine::deny(tx, lane, &event)? {
                    Ok(None) => {}
                    Ok(Some(denial)) => {
                        let key = denial.idempotency_key.as_deref().unwrap_or_default();
                        match by_key(tx, key)? {
                            Some(stored) if stored.kind == EventKind::GateDenied => {
                                return Ok(Applied::Replayed(stored));
                            }
                            Some(stored) => {
                                return Ok(Applied::Refused(invalid(&format!(
                                    "the denial key {key} belongs to a {} event",
                                    stored.kind
                                ))));
                            }
                            None => {}
                        }
                        event = denial;
                    }
                    Err(r) => return Ok(Applied::Refused(r)),
                }
            }
            event.id = insert(tx, &event)?;
            fold::fold(tx, &event)?;
            Applied::Committed(event)
        }
    };
    Ok(applied)
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
    if p.redactions.is_some() {
        refuse!(invalid("redactions is set by the write"));
    }
    // Before the insert, so the log never holds a pre-redaction copy and a
    // retry replays the redacted bytes.
    let redactions = check!(redact::payload(&mut event.payload));
    if redactions > 0 {
        event.payload.redactions = Some(redactions);
    }
    let p = &event.payload;
    if !event.kind.leases() && p.lease_secs.is_some() {
        refuse!(invalid(&format!("{} carries no lease", event.kind)));
    }
    // The expiry is the write's to set, from `at` (lane_guard).
    if p.lease_expires_at.is_some() {
        refuse!(invalid("lease_expires_at is set by the write"));
    }
    // The stamp is the write's to set, on lane.claimed only (lane_guard).
    if event.kind != LaneClaimed
        && (p.pipeline_id.is_some() || p.pipeline_hash.is_some() || p.route.is_some())
    {
        refuse!(invalid(&format!(
            "{} carries no pipeline stamp",
            event.kind
        )));
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
            let (Some(key), Some(_), Some(body)) = (&p.parent_key, &p.title, &p.body) else {
                refuse!(invalid("spec.created needs parent_key, title and body"));
            };
            check!(spec_body(body));
            if event.spec_id.is_some() || p.version.is_some_and(|v| v != 1) {
                refuse!(invalid("spec.created assigns the spec id and version 1"));
            }
            let existing: Option<i64> =
                tx.query_row(SPEC_BY_KEY, [key], |r| r.get(0)).optional()?;
            if let Some(id) = existing {
                refuse!(invalid(&format!(
                    "{key} already has spec {id}; change it with `spec update {id}`"
                )));
            }
            event.spec_id = Some(next_id(tx, "specs")?);
        }
        SpecRevised | SpecApproved => {
            let id = check!(need_id(event.spec_id, "spec"));
            let head = check!(spec_head(tx, id));
            let revised = event.kind == SpecRevised;
            if revised {
                let Some(body) = &p.body else {
                    refuse!(invalid("spec.revised needs a body"));
                };
                check!(spec_body(body));
            }
            // No version: the next one. A given version is the caller's
            // optimistic check, so a stale one is a conflict.
            let want = head + i64::from(revised);
            let given = p.version;
            match given {
                None => event.payload.version = Some(want),
                Some(v) if v == want => {}
                Some(_) => refuse!(Refusal::Conflict {
                    entity: "spec",
                    message: format!("spec {id} is at version {head}; read it again"),
                }),
            }
        }
        LaneCreated => {
            if event.lane_id.is_some() || !p.changed.is_empty() {
                refuse!(invalid(
                    "lane.created assigns the lane id and changes no column"
                ));
            }
            // The route is chosen from kind and pr_flow at claim, so they are
            // never guessed later.
            let vocab =
                |v: &Option<String>, of: &[&str]| v.as_deref().is_some_and(|v| of.contains(&v));
            if p.title.is_none()
                || !vocab(&p.source, SOURCES)
                || !vocab(&p.kind, KINDS)
                || !vocab(&p.pr_flow, PR_FLOWS)
            {
                refuse!(invalid(&format!(
                    "lane.created needs a title, source {SOURCES:?}, kind {KINDS:?} and pr_flow {PR_FLOWS:?}"
                )));
            }
            if let Some(spec) = event.spec_id {
                check!(spec_head(tx, spec));
                if let Some(lane_key) = &p.lane_key
                    && exists(
                        tx,
                        "SELECT 1 FROM lanes WHERE spec_id = ?1 AND lane_key = ?2 AND lane_pos IS ?3",
                        rusqlite::params![spec, lane_key, p.lane_pos],
                    )?
                {
                    refuse!(Refusal::Conflict {
                        entity: "work item",
                        message: format!("lane {lane_key} exists on spec {spec}"),
                    });
                }
            }
            event.lane_id = Some(next_id(tx, "lanes")?);
        }
        // On a key or a repo, never a lane, so they move no lane's
        // `updated_at` and are no fact anchor (ADR 0007, #119). `dossier
        // note` and `backlog_prioritise` check the rest.
        DossierNoted | BacklogPrioritised
            if event.lane_id.is_some() || event.spec_id.is_some() || !p.changed.is_empty() =>
        {
            refuse!(invalid(&format!(
                "{} names no lane or spec and changes no column",
                event.kind
            )));
        }
        BacklogPrioritised => {
            let lower = p.repo.as_deref().is_some_and(|r| r == r.to_lowercase());
            if !lower
                || !p.rest.get("order").is_some_and(Value::is_array)
                || !p.rest.contains_key("after")
            {
                refuse!(invalid(
                    "backlog.prioritised needs a lowercased repo, an order and after"
                ));
            }
        }
        DossierNoted => {
            let lower = |v: &Value| v.as_str().is_some_and(|k| k == k.to_lowercase());
            if !p.rest.get("key").is_some_and(lower)
                || !p.rest.get("note").is_some_and(Value::is_string)
            {
                refuse!(invalid("dossier.noted needs a lowercased key and a note"));
            }
        }
        _ => {
            if event.spec_id.is_some() {
                refuse!(invalid("only spec events and lane.created name a spec"));
            }
            let id = check!(need_id(event.lane_id, "lane"));
            check!(lane_guard(tx, event, id));
            // Usage is deduped on the lane and the provider's message id.
            if event.kind == UsageReported
                && !event
                    .payload
                    .rest
                    .get("message_id")
                    .is_some_and(|v| v.is_string())
            {
                refuse!(invalid("usage.reported needs a message_id"));
            }
            if event.kind == PrMerged
                && (event.payload.head_sha.is_none()
                    || !["pr", "merge_sha", "merged_at"]
                        .iter()
                        .all(|k| event.payload.rest.get(*k).is_some_and(|v| v.is_string())))
            {
                refuse!(invalid(
                    "pr.merged needs pr, merge_sha, head_sha and merged_at"
                ));
            }
            if event.kind == AttentionRecorded
                && !["type", "by"]
                    .iter()
                    .all(|k| event.payload.rest.get(*k).is_some_and(|v| v.is_string()))
            {
                refuse!(invalid("attention.recorded needs type and by"));
            }
            // A handoff's id is its event's, so `work handoffs` and the
            // stored payload agree whoever wrote it.
            if event.payload.rest.contains_key("handoff") {
                let id = next_id(tx, "events")?;
                event.payload.rest.insert("handoff_id".into(), id.into());
            }
        }
    }
    Ok(Ok(()))
}

fn lane_guard(tx: &Transaction, event: &mut Event, lane_id: i64) -> Checked<()> {
    use EventKind::*;
    let ((agent, session, _, until), same_pair) = check!(holder(tx, &event.actor, lane_id));
    let live = until.as_deref().is_some_and(|u| u > event.at.as_str());
    // Checks and the ready override come from other sessions, and a
    // takeover is the way past a live lease, so those kinds skip the holder
    // rule, as does the owner's pause. A claim keeps it: over an expired
    // lease its `changed.from` pins the holder it replaces.
    let holder_rule =
        !(event.kind.keeps_updated_at() || event.kind == LaneTakenOver || owner_pause(event));
    let p = &mut event.payload;
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
        let secs = p.lease_secs.unwrap_or(0);
        if !(MIN_LEASE_SECS..=MAX_LEASE_SECS).contains(&secs) {
            refuse!(invalid(&format!(
                "lease_secs must be between {MIN_LEASE_SECS} and {MAX_LEASE_SECS}, got {secs}"
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
        // Every lease kind runs on the stamp it has; the first claim stamps
        // the newest pipeline and the route its kind and flow pick.
        let (id, hash, kind, flow): (Option<String>, Option<String>, String, String) = tx
            .query_row(
                "SELECT pipeline_id, pipeline_hash, kind, pr_flow FROM lanes WHERE id = ?1",
                [lane_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        if let (Some(id), Some(hash)) = (&id, &hash)
            && crate::pipeline::by_stamp(id, hash).is_none()
        {
            refuse!(Refusal::PipelineUnknown {
                pipeline_id: id.clone(),
                pipeline_hash: hash.clone(),
            });
        }
        if event.kind == LaneClaimed && hash.is_none() {
            let current = crate::pipeline::current();
            p.pipeline_id = Some(current.id.clone());
            p.pipeline_hash = Some(current.hash.clone());
            p.route = Some(Route::for_lane(&kind, &flow).as_str().into());
        } else {
            (p.pipeline_id, p.pipeline_hash, p.route) = (None, None, None);
        }
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
        // Done is gated: only the engine's event folds it (events.md).
        if field == "status" && change.to == "done" && event.kind != PhaseEntered {
            refuse!(invalid(&format!(
                "{} cannot move a lane to done",
                event.kind
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
            refuse!(Refusal::Conflict {
                entity: "work item",
                message: format!("{field} is {current}, not {}", change.from),
            });
        }
        // The harness measured the lane in this folder on this branch
        // (`check run`), so a later run must read the same ones, not ones
        // the lane points it at. One not set yet may be set once. A
        // trailing slash names the same folder; a branch is compared as it
        // is (`refs/heads/feat/` is no branch).
        let trim = field == "worktree_path";
        let folder = |v: &Value| {
            v.as_str()
                .map(|p| if trim { p.trim_end_matches('/') } else { p })
                .map(String::from)
        };
        if matches!(field.as_str(), "worktree_path" | "branch")
            && !change.from.is_null()
            && folder(&change.to) != folder(&change.from)
            && exists(
                tx,
                "SELECT 1 FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
                   AND json_extract(payload_json, '$.producer.kind') = 'harness'",
                [lane_id],
            )?
        {
            refuse!(invalid(&format!(
                "{field} is frozen: lane {lane_id} has a harness check, so it cannot change"
            )));
        }
        if field == "spec_id"
            && let Some(spec) = change.to.as_i64()
        {
            check!(spec_head(tx, spec));
        }
    }
    // A half-cleared holder would be a new holder no lease named.
    if !event.kind.leases()
        && (p.changed.contains_key("agent") || p.changed.contains_key("session"))
        && (!after(tx, lane_id, p, "agent")?.is_null()
            || !after(tx, lane_id, p, "session")?.is_null())
    {
        refuse!(invalid(&format!(
            "{} clears agent and session together",
            event.kind
        )));
    }
    // The (spec_id, lane_key, lane_pos) slot is unique; refuse here, not as
    // a SQLite error.
    if ["spec_id", "lane_key", "lane_pos"]
        .iter()
        .any(|f| p.changed.contains_key(*f))
    {
        let (spec, key, pos) = (
            after(tx, lane_id, p, "spec_id")?,
            after(tx, lane_id, p, "lane_key")?,
            after(tx, lane_id, p, "lane_pos")?,
        );
        if exists(
            tx,
            "SELECT 1 FROM lanes WHERE spec_id = ?1 AND lane_key = ?2 AND lane_pos IS ?3 AND id != ?4",
            rusqlite::params![
                fold::sql_value(&spec),
                fold::sql_value(&key),
                fold::sql_value(&pos),
                lane_id
            ],
        )? {
            refuse!(Refusal::Conflict {
                entity: "work item",
                message: format!("lane {key} exists on spec {spec}"),
            });
        }
    }
    match event.kind {
        CheckFinished => {
            // The independence rule (R10): a session that has held the lane
            // never reports its session checks, so a lane cannot stamp its
            // own handoff. Reviews are daisy's worker's (events.md).
            // Actor::check has made sure there is a session.
            let session = event.actor.session.as_deref().unwrap_or_default();
            if SESSION_CHECKS.contains(&p.check.as_deref().unwrap_or_default())
                && has_held(tx, lane_id, session)?
            {
                refuse!(invalid(&format!(
                    "session {session} has held lane {lane_id}; record checks from a fresh session"
                )));
            }
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
        // The write decides it after these checks (`engine::deny`), and
        // takes `from` from the row, never the caller.
        PhaseEntered => {
            if !p
                .transition
                .as_deref()
                .is_some_and(|t| REQUIRED_TRANSITIONS.contains(&t))
            {
                refuse!(invalid(&format!(
                    "phase.entered runs only {REQUIRED_TRANSITIONS:?}"
                )));
            }
            let from = lane_value(tx, lane_id, "phase")?;
            p.rest.insert("from".into(), from);
        }
        GateDenied => refuse!(invalid("gate.denied is written by the engine only")),
        // Ready is gated the same way: an ungated handoff never says ready.
        LaneHandedOff
            if p.rest.get("handoff").and_then(|h| h.get("ready")) == Some(&true.into()) =>
        {
            refuse!(invalid(
                "a ready handoff is gated; lane.handed_off cannot carry one"
            ));
        }
        FindingUpdated => {
            let (Some(check), Some(key)) = (p.check_event_id, &p.finding_key) else {
                refuse!(invalid(
                    "finding.updated needs check_event_id and finding_key"
                ));
            };
            let to = p.status.as_deref().unwrap_or_default();
            if !FINDING_MOVES.iter().any(|m| m.1 == to) {
                let statuses: BTreeSet<_> = FINDING_MOVES.iter().map(|m| m.1).collect();
                refuse!(invalid(&format!(
                    "finding.updated status is one of {statuses:?}"
                )));
            }
            let row: Option<(String, String)> = tx
                .query_row(
                    "SELECT status, severity FROM findings
                     WHERE lane_id = ?1 AND check_event_id = ?2 AND finding_key = ?3",
                    rusqlite::params![lane_id, check, key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((from, severity)) = row else {
                refuse!(Refusal::NotFoundRef {
                    entity: "finding",
                    reference: format!("{check}/{key}"),
                });
            };
            // A retry after a lost answer lands here: say so plainly.
            if from == to {
                refuse!(invalid(&format!("finding {check}/{key} is already {to}")));
            }
            let Some((.., by)) = FINDING_MOVES.iter().find(|m| m.0 == from && m.1 == to) else {
                refuse!(invalid(&format!(
                    "a finding cannot move from {from} to {to}"
                )));
            };
            // The independence rule again: whoever has held the lane argues
            // its findings, and someone who never has rules on them.
            let session = event.actor.session.as_deref().unwrap_or_default();
            let held = has_held(tx, lane_id, session)?;
            // The verifier `check run review` starts rules whoever started
            // the run: its authority is the binary's, and only that code
            // path sets this producer. Its moves only.
            let harness =
                p.rest.get("producer").and_then(|v| v.get("kind")) == Some(&Value::from("harness"));
            let ruled = harness && matches!(to, "verified" | "refuted");
            match by {
                By::Lane if !held => refuse!(invalid(&format!(
                    "only the lane disputes a finding; session {session} has never held lane {lane_id}"
                ))),
                By::Independent if held && !ruled => refuse!(invalid(&format!(
                    "session {session} has held lane {lane_id}; a finding goes {to} from a session that never has"
                ))),
                _ => {}
            }
            if to == "answered" && severity != "question" {
                refuse!(invalid(&format!(
                    "only a question is answered; finding {check}/{key} is a {severity}"
                )));
            }
            let reason = p.rest.get("reason").and_then(Value::as_str);
            if matches!(to, "disputed" | "waived") && reason.is_none() {
                refuse!(invalid(&format!("a finding goes {to} only with a reason")));
            }
            if reason.is_some_and(|r| !one_line(r)) {
                refuse!(invalid(&format!(
                    "reason is one non-blank line of at most {NOTE_CHARS} characters"
                )));
            }
            // The verifier confirms a fix on a commit, so it names the commit.
            if to == "fixed" && !p.head_sha.as_deref().is_some_and(sha_ok) {
                refuse!(invalid(
                    "a fixed finding needs the head_sha that fixes it, 7 to 40 lowercase hex digits"
                ));
            }
        }
        _ => {}
    }
    Ok(Ok(()))
}

/// The owner's pause (cli-contract, `work update`): `owner-ui` moving a
/// claimed or active lane to blocked, "paused by owner", and nothing
/// else. The one update another pair may write over a live lease.
fn owner_pause(event: &Event) -> bool {
    let p = &event.payload;
    let status = p.changed.get("status");
    let blocker = p.changed.get("blocker");
    event.kind == EventKind::LaneUpdated
        && event.actor.agent.as_deref() == Some(OWNER_UI)
        && p.changed.len() == 2
        && status.is_some_and(|c| {
            matches!(c.from.as_str(), Some("claimed" | "active")) && c.to == "blocked"
        })
        && blocker.is_some_and(|c| c.to == PAUSED_BY_OWNER)
        && *p
            == Payload {
                changed: p.changed.clone(),
                ..Payload::default()
            }
}

/// The session has held the lane: it wrote a `lane.claimed` or
/// `lane.taken_over` on it (events.md, `check.finished` producer).
pub(crate) fn has_held(
    conn: &Connection,
    lane_id: i64,
    session: &str,
) -> Result<bool, LedgerError> {
    exists(
        conn,
        "SELECT 1 FROM events WHERE lane_id = ?1
           AND kind IN ('lane.claimed', 'lane.taken_over') AND actor_session = ?2",
        rusqlite::params![lane_id, session],
    )
}

fn spec_body(body: &str) -> Checked<()> {
    if body.trim().is_empty() {
        refuse!(invalid("spec body is empty"));
    }
    if body.len() > SPEC_BODY_MAX_BYTES {
        refuse!(invalid(&format!(
            "spec body is {} bytes, over the {SPEC_BODY_MAX_BYTES} limit",
            body.len()
        )));
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

/// `at` is RFC 3339 UTC with milliseconds, as SQLite prints it back.
pub(crate) fn canonical_at(conn: &Connection, at: &str) -> Result<bool, LedgerError> {
    Ok(conn.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1) IS ?1",
        [at],
        |r| r.get(0),
    )?)
}

pub(crate) fn exists(conn: &Connection, sql: &str, args: impl Params) -> Result<bool, LedgerError> {
    Ok(conn.query_row(sql, args, |_| Ok(())).optional()?.is_some())
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
pub(crate) fn lease_held(
    agent: Option<String>,
    session: Option<String>,
    until: Option<String>,
) -> Refusal {
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

    fn lane() -> Payload {
        Payload {
            title: Some("t".into()),
            source: Some("manual".into()),
            kind: Some("code".into()),
            pr_flow: Some("pr".into()),
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

    /// A bad event is refused for its own reason and writes no event. Not
    /// every refusal is here: the gate, the pipeline stamp, `pr.merged`,
    /// attention, dossier and usage have their own tests.
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
        let bad_agent = Actor {
            agent: Some("Codex".into()),
            ..me.clone()
        };
        let bad_session = Actor {
            session: Some("s 1".into()),
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
            (&me, ev(EventKind::LaneCreated, None, Some(1), lane())),
            (
                &me,
                ev(
                    EventKind::LaneCreated,
                    None,
                    Some(1),
                    Payload {
                        lane_key: Some("L".into()),
                        ..lane()
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
        let cases: Vec<(&str, &str, Option<&str>, Mutation)> = vec![
            (
                "at 2026-11-03 00:00:00 is not RFC 3339 UTC with",
                "2026-11-03 00:00:00",
                None,
                ev(EventKind::LaneUpdated, Some(1), None, Payload::default()),
            ),
            (
                "a heartbeat takes no idempotency key",
                AT,
                Some("k"),
                Mutation::Heartbeat { lane_id: 1 },
            ),
            (
                "spec events name no lane and change no lane column",
                AT,
                None,
                ev(
                    EventKind::SpecApproved,
                    None,
                    Some(1),
                    Payload {
                        version: Some(1),
                        ..change("status", json!("queued"), json!("done"))
                    },
                ),
            ),
            (
                "spec events name no lane and change no lane column",
                AT,
                None,
                ev(
                    EventKind::SpecCreated,
                    Some(1),
                    None,
                    Payload {
                        parent_key: Some("Flexipie/harness#2".into()),
                        ..spec.clone()
                    },
                ),
            ),
            (
                "spec events name no lane and change no lane column",
                AT,
                None,
                ev(
                    EventKind::SpecCreated,
                    None,
                    None,
                    Payload {
                        parent_key: Some("Flexipie/harness#2".into()),
                        changed: change("status", json!("queued"), json!("done")).changed,
                        ..spec.clone()
                    },
                ),
            ),
            (
                "spec events name no lane and change no lane column",
                AT,
                None,
                ev(
                    EventKind::SpecRevised,
                    Some(1),
                    Some(1),
                    Payload {
                        version: Some(2),
                        body: Some("b".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                "spec events name no lane and change no lane column",
                AT,
                None,
                ev(
                    EventKind::SpecRevised,
                    None,
                    Some(1),
                    Payload {
                        version: Some(2),
                        body: Some("b".into()),
                        ..change("status", json!("queued"), json!("done"))
                    },
                ),
            ),
            (
                "spec.created assigns the spec id and version 1",
                AT,
                None,
                ev(
                    EventKind::SpecCreated,
                    None,
                    Some(1),
                    Payload {
                        parent_key: Some("Flexipie/harness#2".into()),
                        ..spec.clone()
                    },
                ),
            ),
            (
                "lane.created assigns the lane id and changes no column",
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    change("status", json!(null), json!("queued")),
                ),
            ),
            (
                "check.finished needs check and head_sha",
                AT,
                None,
                ev(
                    EventKind::CheckFinished,
                    Some(1),
                    None,
                    Payload {
                        check: Some("smoke".into()),
                        outcome: Some("ran".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                r#"Refused(NotFound { entity: "lane", id: 9 }"#,
                AT,
                None,
                Mutation::Heartbeat { lane_id: 9 },
            ),
            (
                "lane 1 is not held by this agent and session",
                AT,
                None,
                Mutation::Heartbeat { lane_id: 1 },
            ),
            (
                r#"Refused(LeaseHeld { by: "b/t", until:"#,
                AT,
                None,
                Mutation::Heartbeat { lane_id: 2 },
            ),
            (
                "payload does not survive a round trip",
                AT,
                None,
                ev(EventKind::LaneReadyOverridden, Some(1), None, dup),
            ),
            (
                "spec.created needs parent_key, title and body",
                AT,
                None,
                ev(EventKind::SpecCreated, None, None, Payload::default()),
            ),
            (
                "spec.created assigns the spec id and version 1",
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
            (
                "Flexipie/harness#1 already has spec 1",
                AT,
                None,
                ev(EventKind::SpecCreated, None, None, spec),
            ),
            (
                "spec events name no lane and change no lane column",
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
                "spec.approved carries no lease",
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
                "this event needs a spec id",
                AT,
                None,
                ev(EventKind::SpecRevised, None, None, Payload::default()),
            ),
            (
                r#"Refused(NotFound { entity: "spec", id: 9 }"#,
                AT,
                None,
                ev(EventKind::SpecApproved, None, Some(9), Payload::default()),
            ),
            (
                "spec.revised needs a body",
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
                r#"Refused(Conflict { entity: "spec", message: "spec 1 is at"#,
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
                "lane.created assigns the lane id and changes no column",
                AT,
                None,
                ev(EventKind::LaneCreated, Some(1), None, Payload::default()),
            ),
            (
                r#"Refused(NotFound { entity: "spec", id: 9 }"#,
                AT,
                None,
                ev(EventKind::LaneCreated, None, Some(9), lane()),
            ),
            (
                "lane.created needs a title, source",
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    Payload {
                        title: None,
                        ..lane()
                    },
                ),
            ),
            (
                "lane.created needs a title, source",
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    Payload {
                        kind: Some("chore".into()),
                        ..lane()
                    },
                ),
            ),
            (
                "lane.created needs a title, source",
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    Payload {
                        source: Some("jira".into()),
                        ..lane()
                    },
                ),
            ),
            (
                "lane.created needs a title, source",
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    Payload {
                        pr_flow: Some("merge".into()),
                        ..lane()
                    },
                ),
            ),
            (
                "lease_expires_at is set by the write",
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
                "only spec events and lane.created name a spec",
                AT,
                None,
                ev(EventKind::LaneUpdated, Some(1), Some(1), Payload::default()),
            ),
            (
                "this event needs a lane id",
                AT,
                None,
                ev(EventKind::LaneUpdated, None, None, Payload::default()),
            ),
            (
                "only the holder renews its lease",
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
                r#"Refused(LeaseHeld { by: "b/t", until:"#,
                AT,
                None,
                ev(EventKind::LaneRenewed, Some(2), None, lease.clone()),
            ),
            // Another pair's lease is held against a renewal once expired too.
            (
                r#"Refused(LeaseHeld { by: "b/t", until:"#,
                "2026-11-03T00:02:00.000Z",
                None,
                ev(EventKind::LaneRenewed, Some(2), None, lease.clone()),
            ),
            (
                "lane.taken_over must leave this agent and session",
                AT,
                None,
                ev(EventKind::LaneTakenOver, Some(2), None, lease.clone()),
            ),
            (
                "lane.updated carries no lease",
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
                "lanes has no changeable column updated_at",
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
                r#"status "ready" is not a wire status"#,
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
                "pid must be an integer or null",
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
                "branch must be text or null",
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
                "spec_id must be an integer or null",
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
                r#"Refused(NotFound { entity: "spec", id: 9 }"#,
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
                r#"Refused(Conflict { entity: "work item", message: "lane \"L\""#,
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
                "lane.updated only clears agent",
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
                "check.finished changes no lane column",
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
                "check.finished needs check and head_sha",
                AT,
                None,
                ev(EventKind::CheckFinished, Some(1), None, Payload::default()),
            ),
            (
                "check.finished needs check and head_sha",
                AT,
                None,
                ev(
                    EventKind::CheckFinished,
                    Some(1),
                    None,
                    Payload {
                        head_sha: Some("abc1234".into()),
                        outcome: Some("ran".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                "check.finished outcome is ran or skipped",
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
                "finding keys repeat",
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
                "finding.updated needs check_event_id and finding_key",
                AT,
                None,
                ev(EventKind::FindingUpdated, Some(1), None, Payload::default()),
            ),
            // The host runs only handoff, and a phase.entered needs a `to`.
            (
                "phase.entered runs only",
                AT,
                None,
                ev(
                    EventKind::PhaseEntered,
                    Some(1),
                    None,
                    Payload {
                        transition: Some("to_ready".into()),
                        to: Some("ready".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                "phase.entered needs a phase to, got ''",
                AT,
                None,
                ev(
                    EventKind::PhaseEntered,
                    Some(1),
                    None,
                    Payload {
                        transition: Some("handoff".into()),
                        ..Payload::default()
                    },
                ),
            ),
            (
                "finding.updated status is one of",
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
                r#"Refused(NotFoundRef { entity: "finding", reference: "1/f" }"#,
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
            (
                "redactions is set by the write",
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    Payload {
                        redactions: Some(1),
                        ..lane()
                    },
                ),
            ),
            (
                "lane_key holds what looks like a github_token",
                AT,
                None,
                ev(
                    EventKind::LaneCreated,
                    None,
                    None,
                    Payload {
                        lane_key: Some(format!("ghp_{}", "a".repeat(36))),
                        ..lane()
                    },
                ),
            ),
        ];
        let events = |l: &Ledger| -> i64 {
            l.conn()
                .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
                .unwrap()
        };
        // The owner pauses only a claimed or active lane over a live lease.
        let ui = Actor {
            agent: Some(OWNER_UI.into()),
            session: Some("ui:1".into()),
        };
        let mut pause = change("status", json!("queued"), json!("blocked"));
        pause
            .changed
            .extend(change("blocker", json!(null), json!(PAUSED_BY_OWNER)).changed);
        let order = |repo: &str, rest: serde_json::Value| Payload {
            repo: Some(repo.into()),
            rest: serde_json::from_value(rest).unwrap(),
            ..Payload::default()
        };
        let ok = order("o/r", json!({"order": [], "after": null}));
        let backlog = [
            (
                "backlog.prioritised names no lane or spec and changes",
                ev(EventKind::BacklogPrioritised, Some(1), None, ok.clone()),
            ),
            (
                "backlog.prioritised names no lane or spec and changes",
                ev(EventKind::BacklogPrioritised, None, Some(1), ok.clone()),
            ),
            (
                "backlog.prioritised names no lane or spec and changes",
                ev(
                    EventKind::BacklogPrioritised,
                    None,
                    None,
                    Payload {
                        changed: change("status", json!("queued"), json!("done")).changed,
                        ..ok
                    },
                ),
            ),
            (
                "backlog.prioritised needs a lowercased repo, an order",
                ev(
                    EventKind::BacklogPrioritised,
                    None,
                    None,
                    order("O/r", json!({"order": [], "after": null})),
                ),
            ),
            (
                "backlog.prioritised needs a lowercased repo, an order",
                ev(
                    EventKind::BacklogPrioritised,
                    None,
                    None,
                    order("o/r", json!({"order": {}, "after": null})),
                ),
            ),
            (
                "backlog.prioritised needs a lowercased repo, an order",
                ev(
                    EventKind::BacklogPrioritised,
                    None,
                    None,
                    order("o/r", json!({"order": []})),
                ),
            ),
        ];
        let cases = cases
            .into_iter()
            .map(|(want, at, key, m)| (want, &me, at, key, m))
            .chain(backlog.map(|(want, m)| (want, &me, AT, None, m)))
            .chain([
                (
                    r#"Refused(LeaseHeld { by: "b/t", until:"#,
                    &ui,
                    AT,
                    None,
                    ev(EventKind::LaneUpdated, Some(2), None, pause),
                ),
                (
                    "a write needs an agent and a session",
                    &no_session,
                    AT,
                    None,
                    ev(EventKind::LaneClaimed, Some(1), None, lease),
                ),
                (
                    "agent must match [a-z0-9._-]{1,40}, got 'Codex'",
                    &bad_agent,
                    AT,
                    None,
                    ev(EventKind::LaneCreated, None, None, lane()),
                ),
                (
                    "session must match [A-Za-z0-9._:-]{1,120}, got 's 1'",
                    &bad_session,
                    AT,
                    None,
                    ev(EventKind::LaneCreated, None, None, lane()),
                ),
                (
                    "lane.updated clears agent and session together",
                    &other,
                    AT,
                    None,
                    ev(
                        EventKind::LaneUpdated,
                        Some(2),
                        None,
                        change("agent", json!("b"), json!(null)),
                    ),
                ),
                (
                    "lane.updated clears agent and session together",
                    &other,
                    AT,
                    None,
                    ev(
                        EventKind::LaneUpdated,
                        Some(2),
                        None,
                        change("session", json!("t"), json!(null)),
                    ),
                ),
            ]);
        for (i, (want, who, at, key, m)) in cases.enumerate() {
            // An `invalid_input` by its message, anything else as it prints:
            // a case refused for another reason than its own, or not
            // refused, fails. A `match`, so a passing run takes every branch.
            let got = match ledger.apply(who, key, at, m).unwrap() {
                Applied::Refused(Refusal::InvalidInput(message)) => message,
                other => format!("{other:?}"),
            };
            assert!(got.starts_with(want), "case {i}: want {want}, got {got}");
            assert_eq!(events(&ledger), 4, "case {i}");
        }
    }

    /// A session id or an idempotency key shaped like a secret is refused
    /// without being echoed, and nothing is stored.
    #[test]
    fn secret_shaped_identifiers_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let token = format!("ghp_{}", "a".repeat(36));
        let me = Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        };
        // A session outside the allowed characters too: that message
        // would echo it.
        let leaky_session = Actor {
            session: Some(format!("GH_TOKEN={token}")),
            ..me.clone()
        };
        let leaky_agent = Actor {
            agent: Some(format!("ghp_{}", "a".repeat(20))),
            ..me.clone()
        };
        let create = || ev(EventKind::LaneCreated, None, None, lane());
        for (who, key, want) in [
            (&leaky_session, None, "session looks like a github_token"),
            (&leaky_agent, None, "agent looks like a github_token"),
            (&me, Some(token.as_str()), "the idempotency key looks like"),
        ] {
            // Compared as printed: a `let else` would leave a branch the
            // coverage gate counts as never taken.
            let why = format!("{:?}", ledger.apply(who, key, AT, create()).unwrap());
            let want = format!("Refused(InvalidInput(\"{want}");
            assert!(why.starts_with(&want), "{why}");
            assert!(!why.contains("ghp_"), "{why}");
        }
        let stored: i64 = ledger
            .conn()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, 0);
    }

    /// Prose is redacted before the insert: the retry replays the bytes the
    /// first write stored, and neither the log nor the row holds the secret.
    #[test]
    fn prose_is_redacted_and_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let me = Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        };
        let token = format!("ghp_{}", "a".repeat(36));
        let mut p = Payload {
            title: Some(format!("leaked {token}")),
            ..lane()
        };
        p.rest
            .insert("brief".into(), json!({"decisions": ["API_KEY=hunter2"]}));
        let applied = [p.clone(), p].map(|p| {
            ledger
                .apply(
                    &me,
                    Some("k"),
                    AT,
                    ev(EventKind::LaneCreated, None, None, p),
                )
                .unwrap()
        });
        let mut want = Payload {
            title: Some("leaked [redacted:github_token]".into()),
            redactions: Some(2),
            ..lane()
        };
        want.rest.insert(
            "brief".into(),
            json!({"decisions": ["API_KEY=[redacted:assignment]"]}),
        );
        let event = Event {
            id: 1,
            kind: EventKind::LaneCreated,
            lane_id: Some(1),
            spec_id: None,
            actor: me,
            idempotency_key: Some("k".into()),
            payload: want,
            at: AT.into(),
        };
        assert_eq!(
            applied,
            [Applied::Committed(event.clone()), Applied::Replayed(event)]
        );
        let stored: String = ledger
            .conn()
            .query_row(
                "SELECT group_concat(payload_json) || (SELECT title FROM lanes) FROM events",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stored.contains("[redacted:github_token]"), "{stored}");
        assert!(!stored.contains(&token), "{stored}");
        assert!(!stored.contains("hunter2"), "{stored}");
    }

    /// A dry run is the real write, read inside its transaction, then
    /// rolled back: the next real write gets the same lane id.
    #[test]
    fn dry_run_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let me = Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        };
        let create = || ev(EventKind::LaneCreated, None, None, lane());
        let count = |c: &Connection, table: &str| -> Result<i64, LedgerError> {
            Ok(c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
        };
        let (applied, seen) = ledger
            .apply_with(&me, Some("k"), AT, create(), true, |c, _| count(c, "lanes"))
            .unwrap();
        assert!(matches!(applied, Applied::Committed(_)), "{applied:?}");
        assert_eq!(seen, Some(1));
        for table in ["events", "lanes"] {
            assert_eq!(count(ledger.conn(), table).unwrap(), 0, "{table}");
        }
        ledger.apply(&me, Some("k"), AT, create()).unwrap();
        // A key answers only its own kind.
        let update = ev(EventKind::LaneUpdated, Some(1), None, Payload::default());
        assert!(matches!(
            ledger.apply(&me, Some("k"), AT, update).unwrap(),
            Applied::Refused(Refusal::InvalidInput(_))
        ));
        let id: i64 = ledger
            .conn()
            .query_row("SELECT id FROM lanes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(id, 1);
    }

    /// Spec keys are unique in any case, bodies are capped, a missing
    /// version is the next one and a stale one is a conflict.
    #[test]
    fn spec_keys_bodies_and_versions() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let me = Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        };
        let spec = |key: &str, body: String| Payload {
            parent_key: Some(key.into()),
            title: Some("t".into()),
            body: Some(body),
            ..Payload::default()
        };
        let revise = |version: Option<i64>, body: String| Payload {
            version,
            body: Some(body),
            ..Payload::default()
        };
        let apply = |kind, spec_id, payload| {
            ledger
                .apply(&me, None, AT, ev(kind, None, spec_id, payload))
                .unwrap()
        };
        let big = "a".repeat(SPEC_BODY_MAX_BYTES + 1);
        assert!(matches!(
            apply(
                EventKind::SpecCreated,
                None,
                spec("Flexipie/harness#1", "b".into())
            ),
            Applied::Committed(_)
        ));
        for (kind, spec_id, payload) in [
            (EventKind::SpecCreated, None, spec("PER-1", " \n".into())),
            (EventKind::SpecCreated, None, spec("PER-1", big.clone())),
            (EventKind::SpecRevised, Some(1), revise(None, big)),
        ] {
            let applied = apply(kind, spec_id, payload);
            assert!(
                matches!(applied, Applied::Refused(Refusal::InvalidInput(_))),
                "{applied:?}"
            );
        }
        let dup = apply(
            EventKind::SpecCreated,
            None,
            spec("FLEXIPIE/harness#1", "b".into()),
        );
        assert!(format!("{dup:?}").contains("already has spec 1"), "{dup:?}");
        let committed = |applied: Applied| matches!(applied, Applied::Committed(_));
        assert!(committed(apply(
            EventKind::SpecRevised,
            Some(1),
            revise(None, "b2".into())
        )));
        assert!(matches!(
            apply(
                EventKind::SpecRevised,
                Some(1),
                revise(Some(2), "b3".into())
            ),
            Applied::Refused(Refusal::Conflict { entity: "spec", .. })
        ));
        assert!(committed(apply(
            EventKind::SpecApproved,
            Some(1),
            Payload::default()
        )));
        let versions: Vec<(String, Option<i64>)> = ledger
            .conn()
            .prepare("SELECT kind, json_extract(payload_json, '$.version') FROM events ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let want = [
            ("spec.created", None),
            ("spec.revised", Some(2)),
            ("spec.approved", Some(2)),
        ];
        assert_eq!(versions, want.map(|(k, v)| (k.to_string(), v)));
    }

    /// The accepting side of guard branches the property ops never take:
    /// a spec approval, and a lane leaving its spec.
    #[test]
    fn approval_and_spec_clear_commit() {
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
        let approve = Payload {
            version: Some(1),
            ..Payload::default()
        };
        for m in [
            ev(EventKind::SpecCreated, None, None, spec),
            ev(EventKind::LaneCreated, None, Some(1), lane()),
            ev(EventKind::SpecApproved, None, Some(1), approve),
            ev(
                EventKind::LaneUpdated,
                Some(1),
                None,
                change("spec_id", json!(1), json!(null)),
            ),
        ] {
            let applied = ledger.apply(&me, None, AT, m).unwrap();
            assert!(matches!(applied, Applied::Committed(_)), "{applied:?}");
        }
        let (status, spec_id): (String, Option<i64>) = ledger
            .conn()
            .query_row(
                "SELECT specs.status, lanes.spec_id FROM specs, lanes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((status.as_str(), spec_id), ("approved", None));
    }
}
