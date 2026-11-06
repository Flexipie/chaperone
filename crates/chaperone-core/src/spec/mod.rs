//! Specs: the dispatch spec of one parent issue (a Linear key, a daisy issue
//! or a GitHub issue), versioned in the ledger, in daisy's `spec` shapes.
//! Every write is one event through `Ledger::apply_planned`: `spec.created`
//! at version 1, `spec.revised` appends a version, `spec.approved` marks it
//! agreed. Nothing edits a version in place.

pub mod decide;
pub mod parse;
pub mod slice;

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::ledger::{
    Actor, Applied, Checked, EventKind, Ledger, LedgerError, Mutation, Payload, Refusal,
};
use crate::work::{issue_key_ok, pr_ref_ok, resolve_spec};

/// The spec at one version, with what it derives. `linear_key` is daisy's
/// name for the parent key, whatever tracker it is from.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Spec {
    pub id: i64,
    pub linear_key: String,
    pub title: String,
    /// draft | approved
    pub status: String,
    /// It has work items and every one is done or cancelled.
    pub done: bool,
    /// Work items linked by `spec_id`.
    pub items: i64,
    pub version: i64,
    /// How many versions exist (`version` is lower with `--version N`).
    pub versions: i64,
    pub body: String,
    pub author: Option<String>,
    pub reason: Option<String>,
    pub version_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SpecSummary {
    pub id: i64,
    pub linear_key: String,
    pub title: String,
    pub status: String,
    pub done: bool,
    pub items: i64,
    pub version: i64,
    pub slices: usize,
    pub updated_at: String,
}

/// `?1` is the spec id, `?2` the version (null: the newest). Version 1's
/// reason is daisy's "created"; later ones are on their `spec.revised`.
const SELECT: &str = "SELECT s.id, s.external_key, s.title, s.status, v.version, s.head_version,
      v.body, v.author,
      COALESCE(json_extract(e.payload_json, '$.reason'),
               CASE v.version WHEN 1 THEN 'created' END),
      v.at, (SELECT at FROM spec_versions WHERE spec_id = s.id AND version = 1), s.updated_at,
      (SELECT COUNT(*) FROM lanes WHERE spec_id = s.id),
      (SELECT COUNT(*) FROM lanes WHERE spec_id = s.id AND status IN ('done', 'cancelled'))
    FROM specs s
    JOIN spec_versions v ON v.spec_id = s.id AND v.version = COALESCE(?2, s.head_version)
    LEFT JOIN events e ON e.id = v.event_id";

impl Spec {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let (items, terminal): (i64, i64) = (r.get(12)?, r.get(13)?);
        Ok(Spec {
            id: r.get(0)?,
            linear_key: r.get(1)?,
            title: r.get(2)?,
            status: r.get(3)?,
            done: items > 0 && items == terminal,
            items,
            version: r.get(4)?,
            versions: r.get(5)?,
            body: r.get(6)?,
            author: r.get(7)?,
            reason: r.get(8)?,
            version_at: r.get(9)?,
            created_at: r.get(10)?,
            updated_at: r.get(11)?,
        })
    }
}

impl Ledger {
    /// A new spec for a parent, at version 1, `draft`. One per parent in any
    /// case: a second create is refused and names the spec to update. A
    /// GitHub parent is stored as written; there is no mirror to ask.
    pub fn spec_create(
        &self,
        actor: &Actor,
        parent: &str,
        title: Option<&str>,
        body: &str,
        at: &str,
        dry_run: bool,
    ) -> Checked<Spec> {
        let Some(key) = normalize_key(parent) else {
            return Ok(Err(Refusal::InvalidInput(format!(
                "--parent must be a Linear key (CORE-1234), a daisy issue (PER-12) or a \
                 GitHub issue (owner/repo#12), got '{parent}'"
            ))));
        };
        let title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .or_else(|| body_title(body))
            .unwrap_or_else(|| key.clone());
        let payload = Payload {
            parent_key: Some(key),
            title: Some(title),
            version: Some(1),
            body: Some(body.into()),
            ..Payload::default()
        };
        self.spec_write(actor, at, dry_run, EventKind::SpecCreated, None, payload)
    }

    /// A spec by id or parent key, at `version` (None: the newest).
    pub fn spec_get(&self, spec: &str, version: Option<i64>) -> Checked<Spec> {
        let id = match resolve_spec(self.conn(), spec)? {
            Ok(id) => id,
            Err(r) => return Ok(Err(r)),
        };
        Ok(
            read(self.conn(), id, version)?.ok_or_else(|| Refusal::NotFoundRef {
                entity: "spec version",
                reference: format!("{id} v{}", version.unwrap_or_default()),
            }),
        )
    }

    /// Every spec at its newest version, most recently changed first.
    pub fn spec_list(&self) -> Result<Vec<SpecSummary>, LedgerError> {
        let mut stmt = self
            .conn()
            .prepare(&format!("{SELECT} ORDER BY s.updated_at DESC, s.id DESC"))?;
        let rows = stmt.query_map(rusqlite::params![None::<i64>, None::<i64>], Spec::from_row)?;
        rows.map(|r| {
            let s = r?;
            Ok(SpecSummary {
                slices: parse::parse_spec_sections(&s.body).len(),
                id: s.id,
                linear_key: s.linear_key,
                title: s.title,
                status: s.status,
                done: s.done,
                items: s.items,
                version: s.version,
                updated_at: s.updated_at,
            })
        })
        .collect()
    }

    /// A new version with `body`. `expected_version` is the optimistic
    /// check: a writer that read v3 gets a conflict, not a silent overwrite
    /// of a v4 someone else wrote. The title follows the body's `# ` heading.
    #[allow(clippy::too_many_arguments)]
    pub fn spec_update(
        &self,
        actor: &Actor,
        spec: &str,
        body: &str,
        reason: &str,
        expected_version: Option<i64>,
        at: &str,
        dry_run: bool,
    ) -> Checked<Spec> {
        let id = match resolve_spec(self.conn(), spec)? {
            Ok(id) => id,
            Err(r) => return Ok(Err(r)),
        };
        let reason = reason.trim();
        if reason.is_empty() {
            return Ok(Err(Refusal::InvalidInput(
                "a spec update needs a --reason".into(),
            )));
        }
        let mut payload = Payload {
            title: body_title(body),
            version: expected_version.map(|v| v.saturating_add(1)),
            body: Some(body.into()),
            ..Payload::default()
        };
        payload.rest.insert("reason".into(), reason.into());
        self.spec_write(
            actor,
            at,
            dry_run,
            EventKind::SpecRevised,
            Some(id),
            payload,
        )
    }

    /// draft -> approved. An approved spec is returned as it is, unwritten.
    pub fn spec_approve(
        &self,
        actor: &Actor,
        spec: &str,
        at: &str,
        dry_run: bool,
    ) -> Checked<Spec> {
        let current = match self.spec_get(spec, None)? {
            Ok(s) if s.status == "approved" => return Ok(Ok(s)),
            Ok(s) => s,
            Err(r) => return Ok(Err(r)),
        };
        self.spec_write(
            actor,
            at,
            dry_run,
            EventKind::SpecApproved,
            Some(current.id),
            Payload::default(),
        )
    }

    /// One spec event, and the spec it leaves, read in the same transaction.
    fn spec_write(
        &self,
        actor: &Actor,
        at: &str,
        dry_run: bool,
        kind: EventKind,
        spec_id: Option<i64>,
        payload: Payload,
    ) -> Checked<Spec> {
        self.spec_write_planned(actor, Some(at), dry_run, |_, _| {
            Ok(Ok(Mutation::Event {
                kind,
                lane_id: None,
                spec_id,
                payload,
            }))
        })
    }

    /// [`Ledger::spec_write`] for an event built from the rows inside the
    /// write transaction.
    fn spec_write_planned(
        &self,
        actor: &Actor,
        at: Option<&str>,
        dry_run: bool,
        plan: impl FnOnce(&Connection, &str) -> Checked<Mutation>,
    ) -> Checked<Spec> {
        let (applied, spec) = self.apply_planned(
            actor,
            None,
            at,
            dry_run,
            plan,
            |conn, applied, _| match applied {
                Applied::Committed(e) => read(conn, e.spec_id.unwrap_or_default(), None),
                _ => Ok(None),
            },
        )?;
        match (applied, spec.flatten()) {
            (Applied::Refused(r), _) => Ok(Err(r)),
            (_, Some(spec)) => Ok(Ok(spec)),
            (_, None) => Err(std::io::Error::other("a spec write left no spec row").into()),
        }
    }
}

pub(crate) fn read(
    conn: &Connection,
    id: i64,
    version: Option<i64>,
) -> Result<Option<Spec>, LedgerError> {
    Ok(conn
        .query_row(
            &format!("{SELECT} WHERE s.id = ?1"),
            rusqlite::params![id, version],
            Spec::from_row,
        )
        .optional()?)
}

/// A parent key as daisy stores it: a Linear or daisy issue key uppercased,
/// a GitHub issue (`owner/repo#40`) as written. None for anything else.
pub fn normalize_key(raw: &str) -> Option<String> {
    let t = raw.trim();
    if pr_ref_ok(t) {
        return Some(t.to_string());
    }
    let up = t.to_uppercase();
    issue_key_ok(&up).then_some(up)
}

/// The body's first `# ` heading.
pub fn body_title(body: &str) -> Option<String> {
    body.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}
