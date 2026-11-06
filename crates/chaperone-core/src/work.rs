//! Work items: lanes in daisy's `work get` shape, and the work commands,
//! each one event through the write function, planned against the row
//! inside its transaction.

mod handoff;

use std::collections::BTreeMap;
use std::convert::Infallible;

use rusqlite::{Connection, OptionalExtension, Row, ToSql};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) use self::handoff::commit_ok;
pub use self::handoff::{Brief, Handoff, HandoffInput, LookAt, Smoke, Verification, WorkEvent};
use crate::cursor::{self, Cursor, Page};
use crate::engine;
use crate::ledger::{
    Actor, Applied, Change, Checked, Event, EventKind, Ledger, LedgerError, MAX_LEASE_SECS,
    MIN_LEASE_SECS, Mutation, Payload, Refusal, SOURCES, STATUSES, has_held, lease_held,
};
use crate::pipeline::Phase;

const ENTITY: &str = "work item";
/// The M1 gated transition (pipelines/v17.1.toml).
const HANDOFF: &str = "handoff";
const DEFAULT_LEASE_SECS: i64 = 900;
/// Daisy's `tool` on handoff and event rows.
const TOOL: &str = "chaperone";

/// The events that carry a handoff (events.md, folded rows): what `work
/// handoffs` lists and where "the latest handoff" is read.
pub(crate) const IS_HANDOFF: &str = "kind IN ('lane.handed_off', 'phase.entered')
      AND json_extract(payload_json, '$.handoff') IS NOT NULL";

/// Daisy's `WorkItem` fields, plus Chaperone's kind, pr_flow, phase and
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

/// The newest `work ready` on an item while it still overrides (a cleared
/// one is `ready: null` and reads as none). Daisy's `ReadyOverride`.
#[derive(Clone, Debug, Serialize)]
pub struct ReadyOverride {
    pub ready: bool,
    pub reason: Option<String>,
    pub by: Option<String>,
    pub at: String,
}

/// One item on the board, in daisy's `BoardItem` field names.
#[derive(Clone, Debug, Serialize)]
pub struct BoardItem {
    pub item: WorkItem,
    pub handoff: Option<Handoff>,
    pub ready_override: Option<ReadyOverride>,
    pub ready: bool,
    pub ready_reason: String,
    /// Paths this lane's newest handoff shares with another live lane's in
    /// the same repo, the first 50 by path. Handoffs only:
    /// `chaperone overlap` adds the worktrees.
    pub overlaps: Vec<Overlap>,
    pub overlaps_omitted: usize,
}

/// One path two or more live lanes in a repo touch: the other lanes, the
/// lowest 10 ids, and how many more there are.
#[derive(Clone, Debug, Serialize)]
pub struct Overlap {
    pub path: String,
    pub work_ids: Vec<i64>,
    pub work_ids_omitted: usize,
}

const BOARD_OVERLAPS: usize = 50;
/// Lanes listed per path, so lanes that share a path cost the board
/// linear work, not one row per pair.
const OVERLAP_IDS: usize = 10;

/// The head of a path's ids, lowest first and comma-separated, as the
/// board's SQL keeps it: room for at least [`OVERLAP_IDS`] + 1 (this lane
/// may be among them) of 19 digits, without a sort per row.
const IDS_CHARS: usize = 240;

/// A path's group as the board's SQL gives it: the head of its ids and its
/// lane count.
#[derive(Deserialize)]
struct SharedPath {
    path: String,
    ids: String,
    n: usize,
}

impl SharedPath {
    /// The listed ids; at the cut, the last one may be partial and is dropped.
    fn ids(&self) -> Vec<i64> {
        let mut ids: Vec<&str> = self.ids.split(',').collect();
        if self.ids.len() >= IDS_CHARS {
            ids.pop();
        }
        ids.iter().filter_map(|i| i.parse().ok()).collect()
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
            // Done is gated, on the commit of the latest handoff.
            if input.status.as_deref() == Some("done") {
                let head = latest_commit(conn, id)?;
                return Ok(Ok(engine::enter(id, HANDOFF, Phase::Done, head, "done", p)));
            }
            // The PR's first arrival is its own event, with the commit of the
            // latest handoff.
            if kind == LaneUpdated
                && item.pr_ref.is_none()
                && let Some(pr) = &input.pr_ref
            {
                kind = PrOpened;
                p.head_sha = latest_commit(conn, id)?;
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

    /// Records a handoff, as daisy's `record_handoff`: validated, owner
    /// checked, its `status` moved by `transition_ok` (a terminal one clears
    /// the holder), and its first blocker, `next_action`, PR and commit
    /// applied to the item. One `lane.handed_off`, or for a ready or done
    /// handoff the gated `phase.entered` (or `gate.denied`).
    pub fn work_handoff(
        &self,
        actor: &Actor,
        id: i64,
        input: &HandoffInput,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<Handoff> {
        if let Err(r) = input.validate() {
            return Ok(Err(r));
        }
        let stored = serde_json::to_value(input).map_err(std::io::Error::other)?;
        let (applied, _) = self.apply_planned(
            actor,
            None,
            at,
            dry_run,
            |conn, at| {
                let Some(item) = get_row(conn, id, at)? else {
                    return Ok(Err(Refusal::NotFound { entity: ENTITY, id }));
                };
                if let Err(r) = check_owner(&item, actor) {
                    return Ok(Err(r));
                }
                let mut p = Payload::default();
                if let Some(to) = input.status.as_ref().filter(|s| **s != item.status) {
                    if !transition_ok(&item.status, to) {
                        return Ok(Err(Refusal::InvalidInput(format!(
                            "handoff cannot move work item {id} from {} to {to}",
                            item.status
                        ))));
                    }
                    if is_terminal(to) && item.session.is_some() {
                        put(&mut p.changed, "agent", &item.agent, Value::Null);
                        put(&mut p.changed, "session", &item.session, Value::Null);
                    }
                    put(&mut p.changed, "status", &item.status, to);
                }
                for (field, from, to) in [
                    ("blocker", &item.blocker, input.blockers.first()),
                    ("next_action", &item.next_action, input.next_action.as_ref()),
                    (
                        "pr_ref",
                        &item.pr_ref,
                        input.pull_request.as_ref().filter(|p| pr_ref_ok(p)),
                    ),
                    ("base_sha", &item.base_sha, input.commit.as_ref()),
                ] {
                    if let Some(to) = to {
                        put(&mut p.changed, field, from, to);
                    }
                }
                p.rest.insert("handoff".into(), stored);
                // Ready and done are gated (cli-contract.md, commands and
                // transitions); a needs-you or progress handoff is not.
                let done = input.status.as_deref() == Some("done");
                if done || input.ready == Some(true) {
                    let to = if done { Phase::Done } else { Phase::Ready };
                    let status = input.status.as_deref().unwrap_or(&item.status);
                    let head = input.commit.clone();
                    return Ok(Ok(engine::enter(id, HANDOFF, to, head, status, p)));
                }
                Ok(Ok(lane_event(EventKind::LaneHandedOff, id, p)))
            },
            |_, _, _| Ok(()),
        )?;
        match applied {
            Applied::Committed(e) | Applied::Replayed(e) if e.kind == EventKind::GateDenied => {
                Ok(Err(denied(e)))
            }
            Applied::Committed(e) => Ok(Ok(Handoff {
                id: e.id,
                work_id: e.lane_id,
                job_id: None,
                agent: e.actor.agent,
                session: e.actor.session,
                tool: TOOL.into(),
                // The stored copy, redacted (R13), not the input.
                handoff: serde_json::from_value(e.payload.rest["handoff"].clone())
                    .map_err(std::io::Error::other)?,
                created_at: e.at,
            })),
            Applied::Refused(r) => Ok(Err(r)),
            // No key and no heartbeat, so nothing else comes back.
            other => Err(std::io::Error::other(format!("handoff write returned {other:?}")).into()),
        }
    }

    /// The owner's override of ready: `Some(true)` ready, `Some(false)` not,
    /// `None` back to the handoffs. One `lane.ready_overridden`, which
    /// leaves `updated_at` alone. Refused from any session that has held
    /// the lane (cli-contract.md).
    pub fn work_ready(
        &self,
        actor: &Actor,
        id: i64,
        ready: Option<bool>,
        reason: Option<String>,
        at: Option<&str>,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        self.write_lane(actor, id, at, dry_run, |conn, _| {
            let Some(session) = actor.session.as_deref() else {
                return Ok(Err(Refusal::InvalidInput(
                    "work ready needs a session".into(),
                )));
            };
            if has_held(conn, id, session)? {
                return Ok(Err(Refusal::InvalidInput(format!(
                    "work ready is the owner's override; session {session} has held work item {id}"
                ))));
            }
            let mut p = Payload::default();
            p.rest.insert("ready".into(), json!(ready));
            p.rest.insert("reason".into(), json!(trimmed(&reason)));
            Ok(Ok(lane_event(EventKind::LaneReadyOverridden, id, p)))
        })
    }

    /// Every handoff on an item, oldest first. An unknown item has none.
    pub fn work_handoffs(&self, id: i64) -> Result<Vec<Handoff>, LedgerError> {
        let mut stmt = self.conn().prepare(&format!(
            "SELECT id, lane_id, actor_agent, actor_session,
                    json_extract(payload_json, '$.handoff'), at
             FROM events WHERE lane_id = ?1 AND {IS_HANDOFF} ORDER BY id"
        ))?;
        let mut rows = stmt.query([id])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(handoff_at(r, 0)?);
        }
        Ok(out)
    }

    /// Daisy's board: every open item with its newest handoff, its override
    /// and the ready rule's answer, one at a time in board order so a caller
    /// can write each out without holding the board. One statement, so one
    /// snapshot. Stops at `f`'s first error.
    pub fn work_board_each<E>(
        &self,
        now: &str,
        f: impl FnMut(BoardItem) -> Result<(), E>,
    ) -> Result<Result<(), E>, LedgerError> {
        self.board_rows(now, None, f)
    }

    /// One item as the board shows it, without `overlaps` (they need every
    /// open lane; `work ready` reads one item), or None when it is not on
    /// the board (unknown, done or cancelled).
    pub fn work_board_item(&self, id: i64, now: &str) -> Result<Option<BoardItem>, LedgerError> {
        let mut out = None;
        let Ok(()) = self.board_rows(now, Some(id), |b| {
            out = Some(b);
            Ok::<_, Infallible>(())
        })?;
        Ok(out)
    }

    fn board_rows<E>(
        &self,
        now: &str,
        id: Option<i64>,
        mut f: impl FnMut(BoardItem) -> Result<(), E>,
    ) -> Result<Result<(), E>, LedgerError> {
        let one = if id.is_some() { "AND id = ?2" } else { "" };
        // `shared` groups every open lane's newest handoff paths. It holds
        // only paths two lanes share; each row then matches its own handoff
        // against it, so nothing per lane is held. One item skips it, and
        // SQLite never computes a CTE nothing reads.
        let live_path = norm_path("f.value", "live.wt");
        let own_path = norm_path("f.value", "l.worktree_path");
        let overlaps = match id {
            Some(_) => "NULL".to_string(),
            None => format!(
                "(SELECT json_group_array(
                          json_object('path', s.path, 'ids', s.ids, 'n', s.n))
                 -- CROSS JOIN keeps the handoff outside: inside, it is parsed
                 -- again for every shared path.
                 FROM json_each(h.payload_json, '$.handoff.changed_files') f
                 CROSS JOIN shared s ON s.repo = lower(l.repo_slug) AND s.path = {own_path}
                 WHERE f.type = 'text')"
            ),
        };
        let mut stmt = self.conn().prepare(&format!(
            "WITH live AS (
               SELECT id, lower(repo_slug) AS repo, worktree_path AS wt,
                      (SELECT id FROM events WHERE lane_id = lanes.id AND {IS_HANDOFF}
                       ORDER BY id DESC LIMIT 1) AS h
               FROM lanes WHERE status NOT IN ('done', 'cancelled') AND repo_slug IS NOT NULL),
             files AS (
               SELECT DISTINCT live.id, live.repo, {live_path} AS path
               FROM live JOIN events e ON e.id = live.h,
                    json_each(e.payload_json, '$.handoff.changed_files') f
               WHERE f.type = 'text' AND {live_path} <> ''),
             shared AS MATERIALIZED (
               SELECT repo, path, count(*) AS n,
                      substr(group_concat(id, ',' ORDER BY id), 1, {IDS_CHARS}) AS ids
               FROM files GROUP BY repo, path HAVING n > 1)
             SELECT l.*, h.id, h.lane_id, h.actor_agent, h.actor_session,
                    json_extract(h.payload_json, '$.handoff'), h.at,
                    json_extract(o.payload_json, '$.ready'),
                    json_extract(o.payload_json, '$.reason'), o.actor_agent, o.at,
                    {overlaps}
             FROM ({SELECT} WHERE status NOT IN ('done', 'cancelled') {one}) AS l
             LEFT JOIN events h ON h.id = (SELECT id FROM events
               WHERE lane_id = l.id AND {IS_HANDOFF} ORDER BY id DESC LIMIT 1)
             LEFT JOIN events o ON o.id = (SELECT id FROM events
               WHERE lane_id = l.id AND kind = 'lane.ready_overridden' ORDER BY id DESC LIMIT 1)
             ORDER BY l.lane_key IS NULL, l.lane_key, l.lane_pos IS NULL, l.lane_pos, l.id"
        ))?;
        let mut rows = match id {
            Some(id) => stmt.query(rusqlite::params![now, id])?,
            None => stmt.query([now])?,
        };
        while let Some(r) = rows.next()? {
            let item = WorkItem::from_row(r)?;
            let handoff = match r.get::<_, Option<i64>>(32)? {
                Some(_) => Some(handoff_at(r, 32)?),
                None => None,
            };
            let ready_override = match r.get::<_, Option<bool>>(38)? {
                Some(ready) => Some(ReadyOverride {
                    ready,
                    reason: r.get(39)?,
                    by: r.get(40)?,
                    at: r.get(41)?,
                }),
                None => None,
            };
            let (ready, ready_reason) =
                ready_rule(&item, handoff.as_ref(), ready_override.as_ref());
            let mut shared: Vec<SharedPath> = match r.get::<_, Option<String>>(42)? {
                Some(raw) => serde_json::from_str(&raw).map_err(std::io::Error::other)?,
                None => Vec::new(),
            };
            shared.sort_unstable_by(|a, b| a.path.cmp(&b.path));
            shared.dedup_by(|a, b| a.path == b.path);
            let overlaps_omitted = shared.len().saturating_sub(BOARD_OVERLAPS);
            shared.truncate(BOARD_OVERLAPS);
            let overlaps = shared
                .into_iter()
                .map(|s| {
                    let mut work_ids: Vec<i64> =
                        s.ids().into_iter().filter(|&i| i != item.id).collect();
                    work_ids.truncate(OVERLAP_IDS);
                    Overlap {
                        path: s.path,
                        work_ids_omitted: (s.n - 1).saturating_sub(work_ids.len()),
                        work_ids,
                    }
                })
                .collect();
            let item = BoardItem {
                item,
                handoff,
                ready_override,
                ready,
                ready_reason,
                overlaps,
                overlaps_omitted,
            };
            if let Err(e) = f(item) {
                return Ok(Err(e));
            }
        }
        Ok(Ok(()))
    }

    /// The item's lane actions, oldest first, with daisy's action words.
    /// Engine and check events are not lane actions and are left out. An
    /// unknown item has none.
    pub fn work_events(&self, id: i64) -> Result<Vec<WorkEvent>, LedgerError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, lane_id, actor_agent, actor_session, kind,
                    json_extract(payload_json, '$.reason'), payload_json, at,
                    json_extract(payload_json, '$.handoff') IS NOT NULL
             FROM events WHERE lane_id = ?1 AND kind != 'usage.reported' ORDER BY id",
        )?;
        let mut rows = stmt.query([id])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let kind: String = r.get(4)?;
            let kind: EventKind = kind.parse().map_err(std::io::Error::other)?;
            let Some(action) = action(kind, r.get(8)?) else {
                continue;
            };
            out.push(WorkEvent {
                id: r.get(0)?,
                work_id: r.get(1)?,
                agent: r.get(2)?,
                session: r.get(3)?,
                tool: TOOL.into(),
                action: action.into(),
                reason: r.get(5)?,
                detail_json: r.get(6)?,
                at: r.get(7)?,
            });
        }
        Ok(out)
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

/// A handoff event read from columns `base..base + 6` (id, lane, agent,
/// session, handoff JSON, at).
fn handoff_at(r: &Row, base: usize) -> Result<Handoff, LedgerError> {
    let raw: String = r.get(base + 4)?;
    Ok(Handoff {
        id: r.get(base)?,
        work_id: r.get(base + 1)?,
        job_id: None,
        agent: r.get(base + 2)?,
        session: r.get(base + 3)?,
        tool: TOOL.into(),
        handoff: serde_json::from_str(&raw).map_err(std::io::Error::other)?,
        created_at: r.get(base + 5)?,
    })
}

/// SQL for a handoff path as git names it: less a leading `./` or the
/// lane's worktree (`chaperone overlap` does the same).
fn norm_path(value: &str, wt: &str) -> String {
    format!(
        "CASE WHEN {value} LIKE './%' THEN substr({value}, 3)
              WHEN {wt} IS NOT NULL AND substr({value}, 1, length({wt}) + 1) = {wt} || '/'
              THEN substr({value}, length({wt}) + 2)
              ELSE {value} END"
    )
}

/// Daisy's ready rule, as far as the ledger knows it: the owner's override
/// wins; else the item is in `review` and its newest handoff says ready (a
/// ready handoff only exists past the `handoff` gate). Daisy also wants a
/// review on the PR head and no open thread from a person; the ledger sees
/// neither, so callers check them (cli-contract.md).
fn ready_rule(
    item: &WorkItem,
    handoff: Option<&Handoff>,
    over: Option<&ReadyOverride>,
) -> (bool, String) {
    if let Some(o) = over {
        let by = o.by.as_deref().unwrap_or("someone");
        let why = o
            .reason
            .as_deref()
            .map(|r| format!(": {r}"))
            .unwrap_or_default();
        let verdict = if o.ready { "ready" } else { "not ready" };
        return (o.ready, format!("marked {verdict} by {by}{why}"));
    }
    if item.status != "review" {
        return (false, format!("status is {}, not review", item.status));
    }
    if handoff.and_then(|h| h.handoff.ready) != Some(true) {
        return (false, "the lane has not said ready".into());
    }
    (true, "the lane says ready".into())
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
        (Applied::Committed(e) | Applied::Replayed(e), _) if e.kind == EventKind::GateDenied => {
            Err(denied(e))
        }
        (_, Some(item)) => Ok(item),
        // The read runs only after a write that named an existing lane.
        (_, None) => Err(Refusal::InvalidInput("the write left no work item".into())),
    }
}

/// A gated command's refusal, from the `gate.denied` it appended or replayed.
fn denied(e: Event) -> Refusal {
    let p = e.payload;
    Refusal::GateDenied {
        transition: p.transition.unwrap_or_default(),
        gate: p.gate.unwrap_or_default(),
        fact_paths: p.fact_paths.unwrap_or_default(),
    }
}

/// The commit of the lane's latest handoff, the head a `work update`
/// names (events.md, folded rows).
fn latest_commit(conn: &Connection, id: i64) -> Result<Option<String>, LedgerError> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT json_extract(payload_json, '$.handoff.commit') FROM events
                 WHERE lane_id = ?1 AND {IS_HANDOFF} ORDER BY id DESC LIMIT 1"
            ),
            [id],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
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

/// Daisy's action word for a lane event; None for events that are not lane
/// actions. No catch-all, so a new kind has to be placed here.
fn action(kind: EventKind, carries_handoff: bool) -> Option<&'static str> {
    use EventKind::*;
    Some(match kind {
        LaneCreated => "create",
        LaneClaimed => "claim",
        LaneRenewed => "renew",
        LaneTakenOver => "takeover",
        LaneUpdated | PrOpened => "update",
        LaneReopened => "reopen",
        LaneCancelled => "cancel",
        LaneReleased => "release",
        LaneHandedOff => "handoff",
        LaneReadyOverridden => "ready_override",
        PhaseEntered if carries_handoff => "handoff",
        PhaseEntered => "update",
        SpecCreated | SpecRevised | SpecApproved | GateDenied | CheckRequested | CheckFinished
        | FindingUpdated | UsageReported | PrMerged | AttentionRecorded | DossierNoted => {
            return None;
        }
    })
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

pub(crate) fn trimmed(s: &Option<String>) -> Option<String> {
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

/// A spec by its parent key in any case, through `specs_key`.
pub(crate) const SPEC_BY_KEY: &str = "SELECT id FROM specs WHERE external_key = ?1 COLLATE NOCASE";

/// A spec by id, or by its parent key in any case.
pub(crate) fn resolve_spec(conn: &Connection, spec: &str) -> Checked<i64> {
    let spec = spec.trim();
    Ok(match spec.parse::<i64>() {
        Ok(id) => conn
            .query_row("SELECT id FROM specs WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .ok_or(Refusal::NotFound { entity: "spec", id }),
        Err(_) => conn
            .query_row(SPEC_BY_KEY, [spec], |r| r.get(0))
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
pub(crate) fn issue_key_ok(key: &str) -> bool {
    key.split_once('-').is_some_and(|(team, num)| {
        (2..=10).contains(&team.len())
            && team.chars().all(|c| c.is_ascii_uppercase())
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
    })
}

/// `owner/name#digits`.
pub(crate) fn pr_ref_ok(pr_ref: &str) -> bool {
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

    /// A refused handoff writes nothing; a status moves by `transition_ok`
    /// and a terminal one clears the holder and its lease; ready is
    /// refused from a session that held the lane and leaves `updated_at`.
    #[test]
    fn handoff_and_ready_rules() {
        let (_dir, ledger) = ledger();
        let (a, owner) = (pair("a", "s1"), pair("o", "o1"));
        let id = create(&ledger, None, None).unwrap().id;
        ledger
            .work_claim(&a, id, &ClaimInput::default(), Some(T0), false)
            .unwrap()
            .unwrap();
        let handoff = |summary: String, status: &str, ready: Option<bool>| {
            let input = HandoffInput {
                summary,
                status: Some(status.into()),
                ready,
                ..HandoffInput::default()
            };
            ledger
                .work_handoff(&a, id, &input, Some(T1), false)
                .unwrap()
        };
        for refused in [
            handoff("x".repeat(4001), "active", None),
            handoff("s".into(), "queued", None),
        ] {
            assert!(
                matches!(refused, Err(Refusal::InvalidInput(_))),
                "{refused:?}"
            );
        }
        assert_eq!(kinds(&ledger).len(), 2);
        // A needs-you handoff is not gated (ready and done: tests/handoff_gate.rs).
        assert_eq!(handoff("s".into(), "blocked", None).unwrap().id, 3);
        assert_eq!(ledger.work_get(id, T1).unwrap().unwrap().status, "blocked");
        assert!(matches!(
            ledger
                .work_ready(&a, id, Some(true), None, Some(T1), false)
                .unwrap(),
            Err(Refusal::InvalidInput(_))
        ));
        let before = ledger.work_get(id, T1).unwrap().unwrap().updated_at;
        let item = ledger
            .work_ready(&owner, id, Some(true), None, Some(T1), false)
            .unwrap()
            .unwrap();
        assert_eq!(item.updated_at, before);
    }

    /// The first claim stamps the newest pipeline and the route the lane's
    /// kind and flow pick, over any stamp the caller passed. A release and
    /// re-claim keeps it, no other kind may carry one, and a lane stamped
    /// with a pipeline this binary cannot load refuses claim, renewal and
    /// takeover and writes nothing.
    #[test]
    fn stamp_is_written_once_and_an_unknown_one_refuses() {
        let (_dir, ledger) = ledger();
        let (a, b) = (pair("a", "s1"), pair("b", "s2"));
        for kind in ["investigation", "code"] {
            let input = CreateInput {
                title: "t".into(),
                kind: kind.into(),
                pr_flow: "direct".into(),
                ..CreateInput::default()
            };
            ledger
                .work_create(&a, &input, Some(T0), false)
                .unwrap()
                .unwrap();
        }
        let claim = |who: &Actor, takeover: bool| {
            let input = ClaimInput {
                takeover,
                ..ClaimInput::default()
            };
            ledger.work_claim(who, 1, &input, Some(T0), false).unwrap()
        };
        let stamp = |id: i64| -> (String, String, String) {
            ledger
                .conn()
                .query_row(
                    "SELECT pipeline_id, pipeline_hash, route FROM lanes WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        };
        let stamped = |id: i64| -> i64 {
            ledger
                .conn()
                .query_row(
                    "SELECT count(*) FROM events WHERE lane_id = ?1
                     AND json_extract(payload_json, '$.pipeline_hash') IS NOT NULL",
                    [id],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let newest = crate::pipeline::current();
        let want = |route: &str| (newest.id.clone(), newest.hash.clone(), route.to_string());

        let item = claim(&a, false).unwrap();
        assert_eq!(
            (item.pipeline_id, item.pipeline_hash, item.route),
            (
                Some(newest.id.clone()),
                Some(newest.hash.clone()),
                Some("investigation".into())
            )
        );
        ledger
            .work_release(&a, 1, Some(T0), false)
            .unwrap()
            .unwrap();
        claim(&a, false).unwrap();
        assert_eq!((stamp(1), stamped(1)), (want("investigation"), 1));

        // A forged stamp: replaced on a first claim, refused on any other kind.
        let mut held = Payload {
            lease_secs: Some(60),
            pipeline_hash: Some("forged".into()),
            ..Payload::default()
        };
        put(&mut held.changed, "agent", &None::<String>, "a");
        put(&mut held.changed, "session", &None::<String>, "s1");
        let forged = [
            Payload {
                pipeline_id: Some("v18".into()),
                ..Payload::default()
            },
            Payload {
                pipeline_hash: Some("forged".into()),
                ..Payload::default()
            },
            Payload {
                route: Some("direct".into()),
                ..Payload::default()
            },
        ];
        assert!(matches!(
            ledger
                .apply(&a, None, T0, lane_event(EventKind::LaneClaimed, 2, held))
                .unwrap(),
            Applied::Committed(_)
        ));
        assert_eq!(stamp(2), want("direct"));
        for p in forged {
            assert_eq!(
                ledger
                    .apply(&a, None, T0, lane_event(EventKind::LaneUpdated, 2, p))
                    .unwrap(),
                Applied::Refused(Refusal::InvalidInput(
                    "lane.updated carries no pipeline stamp".into()
                ))
            );
        }
        ledger.rebuild().unwrap();
        assert_eq!(
            (stamp(1), stamp(2)),
            (want("investigation"), want("direct"))
        );

        // A newer binary's stamp.
        ledger
            .conn()
            .execute(
                "UPDATE lanes SET pipeline_hash = 'v18hash' WHERE id = 1",
                [],
            )
            .unwrap();
        let events = || kinds(&ledger).len();
        let before = events();
        let unknown = Err(Refusal::PipelineUnknown {
            pipeline_id: "v17.1".into(),
            pipeline_hash: "v18hash".into(),
        });
        assert_eq!(claim(&a, false).map(|_| ()), unknown, "renew");
        assert_eq!(claim(&b, true).map(|_| ()), unknown, "takeover");
        ledger
            .work_release(&a, 1, Some(T0), false)
            .unwrap()
            .unwrap();
        assert_eq!(claim(&b, false).map(|_| ()), unknown, "claim");
        assert_eq!(events(), before + 1);
    }
}
