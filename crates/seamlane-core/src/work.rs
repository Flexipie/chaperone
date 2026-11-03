//! Work items: lanes in daisy's `work get` shape, and `work create`, which
//! writes `lane.created` through [`Ledger::apply_with`].

use rusqlite::{Connection, OptionalExtension, Row, ToSql};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cursor::{self, Cursor, Page};
use crate::ledger::{
    Actor, Applied, Checked, EventKind, Ledger, LedgerError, Mutation, Payload, Refusal, SOURCES,
    STATUSES,
};

const ENTITY: &str = "work item";

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
    /// and is rolled back.
    pub fn work_create(
        &self,
        actor: &Actor,
        input: &CreateInput,
        at: &str,
        dry_run: bool,
    ) -> Checked<WorkItem> {
        let (payload, key) = match create_payload(input) {
            Ok(v) => v,
            Err(r) => return Ok(Err(r)),
        };
        let spec_id = match input.spec.as_deref().map(|s| resolve_spec(self.conn(), s)) {
            Some(Ok(Ok(id))) => Some(id),
            Some(Ok(Err(r))) => return Ok(Err(r)),
            Some(Err(e)) => return Err(e),
            None => None,
        };
        let mutation = Mutation::Event {
            kind: EventKind::LaneCreated,
            lane_id: None,
            spec_id,
            payload,
        };
        let (applied, item) = self.apply_with(
            actor,
            key.as_deref(),
            at,
            mutation,
            dry_run,
            |conn, applied| match applied {
                Applied::Committed(e) | Applied::Replayed(e)
                    if e.kind == EventKind::LaneCreated =>
                {
                    get_row(conn, e.lane_id.unwrap_or_default(), at)
                }
                _ => Ok(None),
            },
        )?;
        Ok(match (applied, item.flatten()) {
            (Applied::Refused(r), _) => Err(r),
            (_, Some(item)) => Ok(item),
            _ => Err(Refusal::InvalidInput(
                "that idempotency key belongs to another event".into(),
            )),
        })
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
    let trimmed = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
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
    let lane_key = trimmed(&input.lane_key);
    if let Some(lane) = &lane_key
        && (lane.chars().count() > 80 || lane.chars().any(char::is_control))
    {
        return invalid(format!(
            "lane must be at most 80 printable characters, got '{lane}'"
        ));
    }
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
