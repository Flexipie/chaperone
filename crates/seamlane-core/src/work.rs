//! Work items: lanes in daisy's `work get` shape, and the work commands,
//! each one event through the write function, planned against the row
//! inside its transaction.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, Row, ToSql};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cursor::{self, Cursor, Page};
use crate::ledger::{
    Actor, Applied, Change, Checked, EventKind, Ledger, LedgerError, MAX_LEASE_SECS,
    MIN_LEASE_SECS, Mutation, Payload, Refusal, SOURCES, STATUSES, lease_held,
};

const ENTITY: &str = "work item";
const DEFAULT_LEASE_SECS: i64 = 900;

/// Daisy's `WorkItem` fields, plus Seamlane's kind, pr_flow, phase and
/// pipeline stamp. `repo_id` and `linear_outbox` are always null.
#[derive(Clone, Debug, Serialize)]
pub struct WorkItem {
    pub id: i64,
    pub title: String,
    pub source: String,
    pub source_id: Option<String>,
    pub source_key: Option<String>,
    pub repo_id: Option<i64>,
    pub repo_slug: Option<String>,
    pub status: String,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub claimed_at: Option<String>,
    pub lease_secs: Option<i64>,
    pub lease_expires_at: Option<String>,
    pub last_heartbeat_at: Option<String>,
    pub base_sha: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub blocker: Option<String>,
    pub next_action: Option<String>,
    pub pr_ref: Option<String>,
    pub idempotency_key: Option<String>,
    pub spec_id: Option<i64>,
    pub lane_key: Option<String>,
    pub lane_pos: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    /// none | active | expired, against the read's `now`.
    pub lease_state: String,
    pub linear_outbox: Option<Value>,
    pub kind: String,
    pub pr_flow: String,
    pub phase: Option<String>,
    pub pipeline_id: Option<String>,
    pub pipeline_hash: Option<String>,
    pub route: Option<String>,
}

/// `?1` is now. `source_key` is the source id for a tracker source, as
/// daisy shows it. The idempotency key is the lane's first event's, which
/// is its `lane.created` (one seek on `events_lane`).
const SELECT: &str = "SELECT id, title, source, source_id,
      CASE WHEN source IN ('linear', 'github', 'issue') THEN source_id END AS source_key,
      repo_slug, status, agent, session, claimed_at, lease_secs, lease_expires_at,
      last_heartbeat_at, base_sha, branch, worktree_path, blocker, next_action, pr_ref,
      (SELECT idempotency_key FROM events WHERE lane_id = lanes.id ORDER BY id LIMIT 1),
      spec_id, lane_key, lane_pos, created_at, updated_at,
      CASE WHEN lease_expires_at IS NULL THEN 'none'
           WHEN lease_expires_at > ?1 THEN 'active' ELSE 'expired' END,
      kind, pr_flow, phase, pipeline_id, pipeline_hash, route
    FROM lanes";

impl WorkItem {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(WorkItem {
            id: r.get(0)?,
            title: r.get(1)?,
            source: r.get(2)?,
            source_id: r.get(3)?,
            source_key: r.get(4)?,
            repo_id: None,
            repo_slug: r.get(5)?,
            status: r.get(6)?,
            agent: r.get(7)?,
            session: r.get(8)?,
            claimed_at: r.get(9)?,
            lease_secs: r.get(10)?,
            lease_expires_at: r.get(11)?,
            last_heartbeat_at: r.get(12)?,
            base_sha: r.get(13)?,
            branch: r.get(14)?,
            worktree_path: r.get(15)?,
            blocker: r.get(16)?,
            next_action: r.get(17)?,
            pr_ref: r.get(18)?,
            idempotency_key: r.get(19)?,
            spec_id: r.get(20)?,
            lane_key: r.get(21)?,
            lane_pos: r.get(22)?,
            created_at: r.get(23)?,
            updated_at: r.get(24)?,
            lease_state: r.get(25)?,
            linear_outbox: None,
            kind: r.get(26)?,
            pr_flow: r.get(27)?,
            phase: r.get(28)?,
            pipeline_id: r.get(29)?,
            pipeline_hash: r.get(30)?,
            route: r.get(31)?,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct CreateInput {
    pub title: String,
    /// linear | github | note | issue | manual (default manual).
    pub source: Option<String>,
    pub source_id: Option<String>,
    /// Stored as given.
    pub repo: Option<String>,
    pub next_action: Option<String>,
    pub idempotency_key: Option<String>,
    /// A spec id, or its parent key.
    pub spec: Option<String>,
    pub lane_key: Option<String>,
    pub lane_pos: Option<i64>,
    /// code | docs | measure | investigation.
    pub kind: String,
    /// pr | direct.
    pub pr_flow: String,
}

#[derive(Clone, Debug, Default)]
pub struct ClaimInput {
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    /// 30..=86400, default 900.
    pub lease_secs: Option<i64>,
    /// Take an item another pair holds live.
    pub takeover: bool,
}

/// What `update` may change; a field left None is kept.
#[derive(Clone, Debug, Default)]
pub struct UpdateInput {
    pub status: Option<String>,
    pub reason: Option<String>,
    pub blocker: Option<String>,
    pub next_action: Option<String>,
    pub pr_ref: Option<String>,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    /// A spec id, or its parent key.
    pub spec: Option<String>,
    pub lane_key: Option<String>,
    pub lane_pos: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct WorkFilter {
    pub status: Option<String>,
    pub agent: Option<String>,
    pub session: Option<String>,
    /// done and cancelled are hidden unless asked for.
    pub include_terminal: bool,
}

impl Ledger {
    /// Appends one `lane.created`, or with the key of an earlier create
    /// returns that item and writes nothing. With `dry_run` the write runs
    /// and is rolled back. `at` None reads the clock under the write lock.
    pub fn work_create(
        &self,
        actor: &Actor,
        input: &CreateInput,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        let (payload, key) = match create_payload(input) {
            Ok(v) => v,
            Err(r) => return Ok(Err(r)),
        };
        let (applied, item) = self.apply_planned(
            actor,
            key.as_deref(),
            at,
            dry_run,
            |conn, _| {
                let spec_id = match input.spec.as_deref().map(|s| resolve_spec(conn, s)) {
                    Some(spec) => match spec? {
                        Ok(id) => Some(id),
                        Err(r) => return Ok(Err(r)),
                    },
                    None => None,
                };
                Ok(Ok(Mutation::Event {
                    kind: EventKind::LaneCreated,
                    lane_id: None,
                    spec_id,
                    payload,
                }))
            },
            |conn, applied, at| match applied {
                Applied::Committed(e) | Applied::Replayed(e) => {
                    get_row(conn, e.lane_id.unwrap_or_default(), at)
                }
                _ => Ok(None),
            },
        )?;
        Ok(item_of(applied, item))
    }

    /// Takes, renews or takes over the lease, as daisy's `claim`: an unheld
    /// item is claimed, the holder's own pair renews, an expired lease is
    /// taken over, and a live one held by another pair is refused unless
    /// `takeover`. `queued` moves to `claimed`; other statuses are kept.
    pub fn work_claim(
        &self,
        actor: &Actor,
        id: i64,
        input: &ClaimInput,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        // Before the row is read, as daisy: a bad length is never a conflict.
        let secs = input.lease_secs.unwrap_or(DEFAULT_LEASE_SECS);
        if !(MIN_LEASE_SECS..=MAX_LEASE_SECS).contains(&secs) {
            return Ok(Err(Refusal::InvalidInput(format!(
                "lease_secs must be between {MIN_LEASE_SECS} and {MAX_LEASE_SECS}, got {secs}"
            ))));
        }
        self.write_lane(actor, id, at, dry_run, |_, item| {
            use EventKind::*;
            if is_terminal(&item.status) {
                return Ok(Err(Refusal::InvalidInput(format!(
                    "work item {id} is {}; reopen it first (update --status queued)",
                    item.status
                ))));
            }
            let mut p = Payload {
                lease_secs: Some(secs),
                ..Payload::default()
            };
            let from = || json!({ "agent": item.agent, "session": item.session });
            let kind = match &item.session {
                None => LaneClaimed,
                Some(_) if held_by(item, actor) => LaneRenewed,
                Some(_) if item.lease_state != "active" => {
                    p.rest.insert("from".into(), from());
                    p.rest.insert("reason".into(), json!("lease expired"));
                    p.rest
                        .insert("expired_at".into(), json!(item.lease_expires_at));
                    LaneTakenOver
                }
                Some(_) if input.takeover => {
                    p.rest.insert("from".into(), from());
                    p.rest.insert("reason".into(), json!("forced"));
                    p.rest
                        .insert("live_until".into(), json!(item.lease_expires_at));
                    LaneTakenOver
                }
                Some(_) => return Ok(Err(holder_refusal(item))),
            };
            if kind != LaneRenewed {
                put(&mut p.changed, "agent", &item.agent, &actor.agent);
                put(&mut p.changed, "session", &item.session, &actor.session);
            }
            if item.status == "queued" {
                put(&mut p.changed, "status", &item.status, "claimed");
            }
            for (field, from, to) in [
                ("worktree_path", &item.worktree_path, &input.worktree_path),
                ("branch", &item.branch, &input.branch),
                ("base_sha", &item.base_sha, &input.base_sha),
            ] {
                if let Some(to) = to {
                    put(&mut p.changed, field, from, to);
                }
            }
            Ok(Ok(lane_event(kind, id, p)))
        })
    }

    /// Extends the holder's lease by its own length. Writes no event.
    pub fn work_heartbeat(
        &self,
        actor: &Actor,
        id: i64,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        self.write_lane(actor, id, at, dry_run, |_, item| {
            if item.session.is_none() {
                return Ok(Err(Refusal::InvalidInput(format!(
                    "work item {id} is not claimed; claim it first"
                ))));
            }
            Ok(Ok(Mutation::Heartbeat { lane_id: id }))
        })
    }

    /// Status and field changes, as daisy's `update`: the owner check, the
    /// `--expected-updated-at` stamp, `transition_ok`, and `blocked` needs a
    /// blocker or a reason. A move to a terminal status clears the holder;
    /// a move out of one reopens.
    pub fn work_update(
        &self,
        actor: &Actor,
        id: i64,
        input: &UpdateInput,
        expected_updated_at: Option<&str>,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        use EventKind::*;
        if let Some(pr) = &input.pr_ref
            && !pr_ref_ok(pr)
        {
            return Ok(Err(Refusal::InvalidInput(format!(
                "pr_ref must look like owner/repo#123, got '{pr}'"
            ))));
        }
        let lane_key = match check_lane(&input.lane_key) {
            Ok(v) => v,
            Err(r) => return Ok(Err(r)),
        };
        self.write_lane(actor, id, at, dry_run, |conn, item| {
            if let Err(r) = check_owner(item, actor) {
                return Ok(Err(r));
            }
            let spec_id = match input.spec.as_deref().map(|s| resolve_spec(conn, s)) {
                Some(spec) => match spec? {
                    Ok(id) => Some(id),
                    Err(r) => return Ok(Err(r)),
                },
                None => None,
            };
            let mut p = Payload::default();
            let mut kind = LaneUpdated;
            if let Some(to) = &input.status {
                if !STATUSES.contains(&to.as_str()) {
                    return Ok(Err(Refusal::InvalidInput(format!(
                        "status must be one of {STATUSES:?}, got '{to}'"
                    ))));
                }
                if !transition_ok(&item.status, to) {
                    return Ok(Err(Refusal::InvalidInput(format!(
                        "cannot move work item {id} from {} to {to}",
                        item.status
                    ))));
                }
                if to == "blocked"
                    && trimmed(&input.blocker).is_none()
                    && trimmed(&input.reason).is_none()
                {
                    return Ok(Err(Refusal::InvalidInput(
                        "moving to blocked needs --blocker or --reason".into(),
                    )));
                }
                if is_terminal(&item.status) {
                    kind = LaneReopened;
                }
                if is_terminal(to) {
                    kind = if to == "cancelled" {
                        LaneCancelled
                    } else {
                        LaneUpdated
                    };
                    if item.session.is_some() {
                        put(&mut p.changed, "agent", &item.agent, Value::Null);
                        put(&mut p.changed, "session", &item.session, Value::Null);
                    }
                }
                put(&mut p.changed, "status", &item.status, to);
            }
            if expected_updated_at.is_some_and(|t| t != item.updated_at) {
                return Ok(Err(Refusal::Conflict {
                    entity: ENTITY,
                    message: format!("{ENTITY} was changed by another process; reload and retry"),
                }));
            }
            for (field, from, to) in [
                ("blocker", &item.blocker, trimmed(&input.blocker)),
                (
                    "next_action",
                    &item.next_action,
                    trimmed(&input.next_action),
                ),
                ("pr_ref", &item.pr_ref, input.pr_ref.clone()),
                ("branch", &item.branch, trimmed(&input.branch)),
                ("base_sha", &item.base_sha, trimmed(&input.base_sha)),
                ("lane_key", &item.lane_key, lane_key.clone()),
            ] {
                if let Some(to) = to {
                    put(&mut p.changed, field, from, to);
                }
            }
            if let Some(spec) = spec_id {
                put(&mut p.changed, "spec_id", item.spec_id, spec);
            }
            if let Some(pos) = input.lane_pos {
                put(&mut p.changed, "lane_pos", item.lane_pos, pos);
            }
            if let Some(reason) = &input.reason {
                p.rest.insert("reason".into(), json!(reason));
            }
            // The PR's first arrival is its own event, with the commit of the
            // latest handoff.
            if kind == LaneUpdated
                && item.pr_ref.is_none()
                && let Some(pr) = &input.pr_ref
            {
                kind = PrOpened;
                p.head_sha = conn
                    .query_row(
                        "SELECT json_extract(payload_json, '$.handoff.commit') FROM events
                         WHERE lane_id = ?1 AND json_extract(payload_json, '$.handoff') IS NOT NULL
                         ORDER BY id DESC LIMIT 1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                p.rest.insert("pr".into(), json!(pr));
            }
            Ok(Ok(lane_event(kind, id, p)))
        })
    }

    /// `update` to cancelled from any non-terminal status, clearing the holder.
    pub fn work_cancel(
        &self,
        actor: &Actor,
        id: i64,
        reason: Option<String>,
        expected_updated_at: Option<&str>,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        let input = UpdateInput {
            status: Some("cancelled".into()),
            reason,
            ..UpdateInput::default()
        };
        self.work_update(actor, id, &input, expected_updated_at, at, dry_run)
    }

    /// Gives the item up: the holder, or anyone once the lease has expired.
    /// Clears the holder; `claimed` moves back to `queued`, every other
    /// status is kept (a released `review` item is still in review).
    pub fn work_release(
        &self,
        actor: &Actor,
        id: i64,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        self.write_lane(actor, id, at, dry_run, |_, item| {
            if item.session.is_none() {
                return Ok(Err(Refusal::InvalidInput(format!(
                    "work item {id} is not claimed"
                ))));
            }
            // The write's lease guard refuses another pair's live lease.
            let mut p = Payload::default();
            put(&mut p.changed, "agent", &item.agent, Value::Null);
            put(&mut p.changed, "session", &item.session, Value::Null);
            if item.status == "claimed" {
                put(&mut p.changed, "status", &item.status, "queued");
            }
            Ok(Ok(lane_event(EventKind::LaneReleased, id, p)))
        })
    }

    /// One write on an existing item: `plan` sees the row as of `at`, read
    /// under the write lock, and the item comes back as the write left it.
    fn write_lane(
        &self,
        actor: &Actor,
        id: i64,
        at: Option<&str>,
        dry_run: bool,
        plan: impl FnOnce(&Connection, &WorkItem) -> Checked<Mutation>,
    ) -> Checked<WorkItem> {
        let (applied, item) = self.apply_planned(
            actor,
            None,
            at,
            dry_run,
            |conn, at| match get_row(conn, id, at)? {
                Some(item) => plan(conn, &item),
                None => Ok(Err(Refusal::NotFound { entity: ENTITY, id })),
            },
            |conn, _, at| get_row(conn, id, at),
        )?;
        Ok(item_of(applied, item))
    }

    /// One item, its lease state read against `now`.
    pub fn work_get(&self, id: i64, now: &str) -> Checked<WorkItem> {
        Ok(get_row(self.conn(), id, now)?.ok_or(Refusal::NotFound { entity: ENTITY, id }))
    }

    /// Newest activity first, keyset `[updated_at, id]`.
    pub fn work_list(
        &self,
        filter: &WorkFilter,
        limit: u32,
        cursor: Option<&str>,
        now: &str,
    ) -> Checked<Page<WorkItem>> {
        let checked = (|| {
            if let Some(s) = &filter.status
                && !STATUSES.contains(&s.as_str())
            {
                return Err(Refusal::InvalidInput(format!(
                    "status must be one of {STATUSES:?}, got '{s}'"
                )));
            }
            Ok((
                cursor::check_limit(limit)?,
                cursor.map(|c| Cursor::decode(c, 2)).transpose()?,
            ))
        })();
        let (limit, cursor) = match checked {
            Ok(v) => v,
            Err(r) => return Ok(Err(r)),
        };
        let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(now.to_string())];
        let mut clauses = Vec::new();
        if filter.status.is_none() && !filter.include_terminal {
            clauses.push("status NOT IN ('done', 'cancelled')".to_string());
        }
        for (column, value) in [
            ("status", &filter.status),
            ("agent", &filter.agent),
            ("session", &filter.session),
        ] {
            if let Some(v) = value {
                params.push(Box::new(v.clone()));
                clauses.push(format!("{column} = ?{}", params.len()));
            }
        }
        if let Some(Cursor(parts)) = &cursor {
            // Typed binds: an integer id compared to text sorts wrong.
            let (Some(at), Some(id)) = (parts[0].as_str(), parts[1].as_i64()) else {
                return Ok(Err(Refusal::InvalidInput("malformed cursor".into())));
            };
            let n = params.len();
            params.push(Box::new(at.to_string()));
            params.push(Box::new(id));
            clauses.push(format!(
                "(updated_at < ?{a} OR (updated_at = ?{a} AND id < ?{b}))",
                a = n + 1,
                b = n + 2
            ));
        }
        params.push(Box::new(limit + 1));
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "{SELECT} {where_sql} ORDER BY updated_at DESC, id DESC LIMIT ?{}",
            params.len()
        );
        let rows = self
            .conn()
            .prepare(&sql)?
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                WorkItem::from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Ok(cursor::from_rows(rows, limit, |w| {
            vec![json!(w.updated_at), json!(w.id)]
        })))
    }
}

fn get_row(conn: &Connection, id: i64, now: &str) -> Result<Option<WorkItem>, LedgerError> {
    Ok(conn
        .query_row(
            &format!("{SELECT} WHERE id = ?2"),
            rusqlite::params![now, id],
            WorkItem::from_row,
        )
        .optional()?)
}

/// The item a write left, or why it was refused.
fn item_of(applied: Applied, item: Option<Option<WorkItem>>) -> Result<WorkItem, Refusal> {
    match (applied, item.flatten()) {
        (Applied::Refused(r), _) => Err(r),
        (_, Some(item)) => Ok(item),
        // The read runs only after a write that named an existing lane.
        (_, None) => Err(Refusal::InvalidInput("the write left no work item".into())),
    }
}

fn lane_event(kind: EventKind, id: i64, payload: Payload) -> Mutation {
    Mutation::Event {
        kind,
        lane_id: Some(id),
        spec_id: None,
        payload,
    }
}

fn put(
    changed: &mut BTreeMap<String, Change>,
    field: &str,
    from: impl Serialize,
    to: impl Serialize,
) {
    changed.insert(
        field.into(),
        Change {
            from: json!(from),
            to: json!(to),
        },
    );
}

/// Daisy's status moves. `claim` and `release` move `queued` and `claimed`
/// themselves; nothing moves to `claimed` here.
pub(crate) fn transition_ok(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        (
            "claimed",
            "active" | "blocked" | "review" | "done" | "cancelled"
        ) | ("active", "blocked" | "review" | "done" | "cancelled")
            | ("blocked", "active" | "review" | "done" | "cancelled")
            | ("review", "active" | "done" | "cancelled")
            | ("queued", "blocked" | "done" | "cancelled")
            | ("done" | "cancelled", "queued")
    )
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "cancelled")
}

/// The exact (agent, session) pair holds the item.
fn held_by(item: &WorkItem, actor: &Actor) -> bool {
    item.session.is_some() && (&item.agent, &item.session) == (&actor.agent, &actor.session)
}

fn holder_refusal(item: &WorkItem) -> Refusal {
    lease_held(
        item.agent.clone(),
        item.session.clone(),
        item.lease_expires_at.clone(),
    )
}

/// Daisy's owner rule for every change but claim and heartbeat: a live
/// lease held by another pair refuses everyone else.
pub(crate) fn check_owner(item: &WorkItem, actor: &Actor) -> Result<(), Refusal> {
    if item.lease_state == "active" && !held_by(item, actor) {
        return Err(holder_refusal(item));
    }
    Ok(())
}

fn trimmed(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// A lane name: trimmed, at most 80 printable characters.
fn check_lane(lane: &Option<String>) -> Result<Option<String>, Refusal> {
    let lane = trimmed(lane);
    if let Some(lane) = &lane
        && (lane.chars().count() > 80 || lane.chars().any(char::is_control))
    {
        return Err(Refusal::InvalidInput(format!(
            "lane must be at most 80 printable characters, got '{lane}'"
        )));
    }
    Ok(lane)
}

/// A spec by id, or by its parent key in any case.
fn resolve_spec(conn: &Connection, spec: &str) -> Checked<i64> {
    let spec = spec.trim();
    Ok(match spec.parse::<i64>() {
        Ok(id) => conn
            .query_row("SELECT id FROM specs WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .ok_or(Refusal::NotFound { entity: "spec", id }),
        Err(_) => conn
            .query_row(
                "SELECT id FROM specs WHERE external_key = ?1 COLLATE NOCASE",
                [spec],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Refusal::NotFoundRef {
                entity: "spec",
                reference: spec.into(),
            }),
    })
}

/// Daisy's create rules: trimmed title, a known source with a source id in
/// its shape, a Linear key uppercased, lane and key length caps.
fn create_payload(input: &CreateInput) -> Result<(Payload, Option<String>), Refusal> {
    let invalid = |m: String| Err(Refusal::InvalidInput(m));
    let title = input.title.trim();
    if title.is_empty() {
        return invalid("title is empty".into());
    }
    let source = input.source.as_deref().unwrap_or("manual");
    if !SOURCES.contains(&source) {
        return invalid(format!("source must be one of {SOURCES:?}, got '{source}'"));
    }
    let source_id = trimmed(&input.source_id).map(|s| {
        if source == "linear" {
            s.to_uppercase()
        } else {
            s
        }
    });
    match source_id.as_deref() {
        None if source == "manual" => {}
        None => return invalid(format!("source '{source}' needs a source_id")),
        Some(id) => {
            let ok = match source {
                "linear" => issue_key_ok(id),
                "github" => pr_ref_ok(id),
                "note" => id.parse::<i64>().is_ok(),
                _ => id.chars().count() <= 200,
            };
            if !ok {
                return invalid(format!("source_id '{id}' is not a valid {source} id"));
            }
        }
    }
    let lane_key = check_lane(&input.lane_key)?;
    let key = trimmed(&input.idempotency_key);
    if key.as_ref().is_some_and(|k| k.len() > 200) {
        return invalid("idempotency_key is longer than 200 characters".into());
    }
    let payload = Payload {
        title: Some(title.into()),
        source: Some(source.into()),
        source_id,
        repo: input.repo.clone(),
        next_action: trimmed(&input.next_action),
        lane_key,
        lane_pos: input.lane_pos,
        kind: Some(input.kind.clone()),
        pr_flow: Some(input.pr_flow.clone()),
        ..Payload::default()
    };
    Ok((payload, key))
}

/// `TEAM-123`: 2 to 10 uppercase letters, a dash, digits.
fn issue_key_ok(key: &str) -> bool {
    key.split_once('-').is_some_and(|(team, num)| {
        (2..=10).contains(&team.len())
            && team.chars().all(|c| c.is_ascii_uppercase())
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
    })
}

/// `owner/name#digits`.
fn pr_ref_ok(pr_ref: &str) -> bool {
    let part_ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    pr_ref.split_once('#').is_some_and(|(slug, num)| {
        slug.split_once('/')
            .is_some_and(|(owner, name)| part_ok(owner) && part_ok(name))
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: &str = "2026-11-03T00:00:00.000Z";
    const T1: &str = "2026-11-03T00:01:00.000Z";

    fn pair(agent: &str, session: &str) -> Actor {
        Actor {
            agent: Some(agent.into()),
            session: Some(session.into()),
        }
    }

    fn ledger() -> (tempfile::TempDir, Ledger) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        (dir, ledger)
    }

    fn create(ledger: &Ledger, lane: Option<&str>, pos: Option<i64>) -> Result<WorkItem, Refusal> {
        let input = CreateInput {
            title: "t".into(),
            spec: lane.map(|_| "1".into()),
            lane_key: lane.map(String::from),
            lane_pos: pos,
            kind: "code".into(),
            pr_flow: "pr".into(),
            ..CreateInput::default()
        };
        ledger
            .work_create(&pair("a", "s"), &input, Some(T0), false)
            .unwrap()
    }

    fn kinds(ledger: &Ledger) -> Vec<String> {
        ledger
            .conn()
            .prepare("SELECT kind FROM events WHERE lane_id IS NOT NULL ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// Daisy's lease rules: claim, renew, refuse a live lease, take over an
    /// expired one or with --takeover, heartbeat only by the holder, and
    /// release back to queued.
    #[test]
    fn lease_rules() {
        let (_dir, ledger) = ledger();
        let (a, b) = (pair("a", "s1"), pair("b", "s2"));
        let id = create(&ledger, None, None).unwrap().id;
        let claim = |who: &Actor, at: &str, takeover: bool| {
            let input = ClaimInput {
                lease_secs: Some(30),
                takeover,
                ..ClaimInput::default()
            };
            ledger.work_claim(who, id, &input, Some(at), false).unwrap()
        };
        let item = claim(&a, T0, false).unwrap();
        assert_eq!(
            (item.status.as_str(), item.claimed_at.as_deref()),
            ("claimed", Some(T0))
        );
        let held = Refusal::LeaseHeld {
            by: "a/s1".into(),
            until: "2026-11-03T00:00:30.000Z".into(),
        };
        assert_eq!(claim(&b, T0, false).unwrap_err(), held);
        assert_eq!(
            ledger
                .work_heartbeat(&b, id, Some(T0), false)
                .unwrap()
                .unwrap_err(),
            held
        );
        claim(&a, T0, false).unwrap();
        claim(&b, T1, false).unwrap();
        let item = claim(&a, T1, true).unwrap();
        assert_eq!(item.agent.as_deref(), Some("a"));
        let item = ledger
            .work_release(&a, id, Some(T1), false)
            .unwrap()
            .unwrap();
        assert_eq!(
            (item.status.as_str(), item.session, item.claimed_at),
            ("queued", None, None)
        );
        assert_eq!(
            kinds(&ledger),
            [
                "lane.created",
                "lane.claimed",
                "lane.renewed",
                "lane.taken_over",
                "lane.taken_over",
                "lane.released"
            ]
        );
    }

    /// The update stamp and the owner check refuse and write nothing; a
    /// fresh stamp writes.
    #[test]
    fn update_rules() {
        let (_dir, ledger) = ledger();
        let (a, b) = (pair("a", "s1"), pair("b", "s2"));
        let item = create(&ledger, None, None).unwrap();
        ledger
            .work_claim(&a, item.id, &ClaimInput::default(), Some(T0), false)
            .unwrap()
            .unwrap();
        let update = |who: &Actor, status: &str, stamp: &str| {
            let input = UpdateInput {
                status: Some(status.into()),
                ..UpdateInput::default()
            };
            ledger
                .work_update(who, item.id, &input, Some(stamp), Some(T0), false)
                .unwrap()
        };
        assert!(matches!(
            update(&a, "active", T1),
            Err(Refusal::Conflict { .. })
        ));
        assert!(matches!(
            update(&b, "active", T0),
            Err(Refusal::LeaseHeld { .. })
        ));
        assert_eq!(kinds(&ledger).len(), 2);
        // No clock passed: the write reads it under its lock.
        let input = UpdateInput {
            status: Some("active".into()),
            ..UpdateInput::default()
        };
        let item = ledger
            .work_update(&a, item.id, &input, Some(T0), None, false)
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "active");
        assert!(item.updated_at.as_str() > T1, "{}", item.updated_at);
    }

    /// A stack's layers share a lane at different positions; a taken slot,
    /// by create or by update, is a conflict.
    #[test]
    fn lane_slots_are_unique() {
        let (_dir, ledger) = ledger();
        let spec = Payload {
            parent_key: Some("Flexipie/harness#1".into()),
            title: Some("t".into()),
            body: Some("b".into()),
            ..Payload::default()
        };
        ledger
            .apply(
                &pair("a", "s"),
                None,
                T0,
                Mutation::Event {
                    kind: EventKind::SpecCreated,
                    lane_id: None,
                    spec_id: None,
                    payload: spec,
                },
            )
            .unwrap();
        create(&ledger, Some("L"), Some(1)).unwrap();
        let second = create(&ledger, Some("L"), Some(2)).unwrap();
        create(&ledger, Some("L"), None).unwrap();
        for refused in [
            create(&ledger, Some("L"), Some(2)),
            create(&ledger, Some("L"), None),
            ledger
                .work_update(
                    &pair("a", "s"),
                    second.id,
                    &UpdateInput {
                        lane_pos: Some(1),
                        ..UpdateInput::default()
                    },
                    None,
                    Some(T0),
                    false,
                )
                .unwrap(),
        ] {
            assert!(
                matches!(refused, Err(Refusal::Conflict { .. })),
                "{refused:?}"
            );
        }
    }
}
