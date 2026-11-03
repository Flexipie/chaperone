//! `spec get --key`: one slice of a spec with its decisions and where each
//! sibling's work stands, in daisy's `SliceView` shape.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::parse::{self, Decision, SpecSection};
use super::{SELECT, Spec};
use crate::ledger::{Checked, Ledger, LedgerError, Refusal};

/// Another slice of the same spec, with its work item when it has one.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SiblingView {
    pub n: Option<u32>,
    pub key: Option<String>,
    pub title: String,
    pub done_when: String,
    pub work_id: Option<i64>,
    pub status: Option<String>,
    pub pr_ref: Option<String>,
    /// Always null: Seamlane keeps no Linear mirror.
    pub linear_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SliceView {
    pub spec_id: i64,
    pub parent_key: String,
    pub parent_title: String,
    pub spec_status: String,
    pub spec_done: bool,
    pub version: i64,
    pub versions: i64,
    pub section: SpecSection,
    /// This slice's decisions and the spec-wide ones, in order.
    pub decisions: Vec<Decision>,
    pub siblings: Vec<SiblingView>,
}

impl Ledger {
    /// The slice whose section names `key`, from `spec` (by id or parent
    /// key) or else the newest-changed spec that has it, at `version`.
    pub fn spec_slice(
        &self,
        spec: Option<&str>,
        key: &str,
        version: Option<i64>,
    ) -> Checked<SliceView> {
        let found = match spec {
            Some(s) => s.to_string(),
            None => match spec_for_key(self.conn(), key)? {
                Some(id) => id.to_string(),
                None => {
                    return Ok(Err(Refusal::NotFoundRef {
                        entity: "spec with a slice for",
                        reference: key.trim().into(),
                    }));
                }
            },
        };
        let s = match self.spec_get(&found, version)? {
            Ok(s) => s,
            Err(r) => return Ok(Err(r)),
        };
        Ok(match slice(self.conn(), &s, key)? {
            Some(view) => Ok(view),
            None => Err(Refusal::NotFoundRef {
                entity: "slice",
                reference: format!("{} in spec {} v{}", key.trim(), s.id, s.version),
            }),
        })
    }
}

/// The newest-changed spec whose newest version has a section for `key`.
fn spec_for_key(conn: &Connection, key: &str) -> Result<Option<i64>, LedgerError> {
    let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY s.updated_at DESC, s.id DESC"))?;
    for s in stmt.query_map(rusqlite::params![None::<i64>, None::<i64>], Spec::from_row)? {
        let s = s?;
        if parse::parse_spec_sections(&s.body)
            .iter()
            .any(|sec| sec.has_key(key))
        {
            return Ok(Some(s.id));
        }
    }
    Ok(None)
}

fn slice(conn: &Connection, spec: &Spec, key: &str) -> Result<Option<SliceView>, LedgerError> {
    let sections = parse::parse_spec_sections(&spec.body);
    let Some(section) = sections.iter().find(|s| s.has_key(key)).cloned() else {
        return Ok(None);
    };
    let decisions = parse::parse_decisions(&spec.body)
        .into_iter()
        .filter(|d| d.slice.is_none() || d.slice == section.n)
        .collect();
    let mut siblings = Vec::new();
    for s in sections.iter().filter(|s| !s.has_key(key)) {
        let work = match &s.key {
            Some(k) => work_for_key(conn, spec.id, k)?,
            None => None,
        };
        let (work_id, status, pr_ref) = match work {
            Some((id, status, pr)) => (Some(id), Some(status), pr),
            None => (None, None, None),
        };
        siblings.push(SiblingView {
            n: s.n,
            key: s.key.clone(),
            title: s.title.clone(),
            done_when: s.done_when.clone(),
            work_id,
            status,
            pr_ref,
            linear_state: None,
        });
    }
    Ok(Some(SliceView {
        spec_id: spec.id,
        parent_key: spec.linear_key.clone(),
        parent_title: spec.title.clone(),
        spec_status: spec.status.clone(),
        spec_done: spec.done,
        version: spec.version,
        versions: spec.versions,
        section,
        decisions,
        siblings,
    }))
}

/// A slice's work item: the one linked to this spec first, else the newest
/// whose Linear, daisy issue or GitHub key is the slice's.
fn work_for_key(
    conn: &Connection,
    spec_id: i64,
    key: &str,
) -> Result<Option<(i64, String, Option<String>)>, LedgerError> {
    Ok(conn
        .query_row(
            "SELECT id, status, pr_ref FROM lanes
              WHERE source IN ('linear', 'issue', 'github') AND source_id = ?1 COLLATE NOCASE
              ORDER BY (spec_id IS ?2) DESC, id DESC LIMIT 1",
            rusqlite::params![key, spec_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}
