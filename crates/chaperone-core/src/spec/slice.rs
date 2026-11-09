//! `spec get --key`: one slice of a spec with its decisions and where each
//! sibling's work stands, in daisy's `SliceView` shape.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::Spec;
use super::parse::{self, Decision, SpecSection};
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
    /// Always null: Chaperone keeps no Linear mirror.
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
pub(crate) fn spec_for_key(conn: &Connection, key: &str) -> Result<Option<i64>, LedgerError> {
    Ok(conn
        .query_row(SPEC_FOR_KEY, [key.trim()], |r| r.get(0))
        .optional()?)
}

const SPEC_FOR_KEY: &str = "SELECT s.id FROM spec_keys k JOIN specs s ON s.id = k.spec_id
    WHERE k.key = ?1 ORDER BY s.updated_at DESC, s.id DESC LIMIT 1";

/// The lanes on an issue key `?1`: a Linear, daisy issue or GitHub source
/// whose id is the key, in any case (the `lanes_source` index).
pub(crate) const KEY_LANES: &str =
    "source IN ('linear', 'issue', 'github') AND source_id = ?1 COLLATE NOCASE";

fn sibling_sql(linked: &str) -> String {
    format!(
        "SELECT id, status, pr_ref FROM lanes WHERE {linked} {KEY_LANES}
          ORDER BY id DESC LIMIT 1"
    )
}

/// The section whose heading names `key`, with its decisions and the
/// spec-wide ones, in order.
pub(crate) fn section_for(body: &str, key: &str) -> Option<(SpecSection, Vec<Decision>)> {
    let section = parse::parse_spec_sections(body)
        .into_iter()
        .find(|s| s.has_key(key))?;
    let decisions = parse::parse_decisions(body)
        .into_iter()
        .filter(|d| d.slice.is_none() || d.slice == section.n)
        .collect();
    Some((section, decisions))
}

fn slice(conn: &Connection, spec: &Spec, key: &str) -> Result<Option<SliceView>, LedgerError> {
    let Some((section, decisions)) = section_for(&spec.body, key) else {
        return Ok(None);
    };
    let sections = parse::parse_spec_sections(&spec.body);
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
    // Linked first, so this spec's item wins over a newer unlinked one.
    let find = |linked: &str, args: &[&dyn rusqlite::ToSql]| {
        conn.query_row(&sibling_sql(linked), args, |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
    };
    Ok(match find("spec_id = ?2 AND", &[&key, &spec_id])? {
        Some(work) => Some(work),
        None => find("", &[&key])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Actor, MIGRATIONS};
    use crate::work::SPEC_BY_KEY;

    fn plan(conn: &Connection, sql: &str) -> String {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt.query_map(["k"], |r| r.get::<_, String>(3)).unwrap();
        rows.map(Result::unwrap).collect::<Vec<_>>().join("; ")
    }

    fn keys(conn: &Connection) -> Vec<(i64, String)> {
        let mut stmt = conn
            .prepare("SELECT spec_id, key FROM spec_keys ORDER BY spec_id, key")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn slice_spec(ledger: &Ledger, key: &str) -> Option<i64> {
        ledger
            .spec_slice(None, key, None)
            .unwrap()
            .ok()
            .map(|v| v.spec_id)
    }

    #[test]
    fn key_lookups_use_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let conn = ledger.conn();
        for (sql, index) in [
            (SPEC_FOR_KEY.to_string(), "spec_keys_key (key=?)"),
            (SPEC_BY_KEY.to_string(), "specs_key (external_key=?)"),
            (sibling_sql(""), "lanes_source (source_id=?)"),
        ] {
            let plan = plan(conn, &sql);
            assert!(plan.contains(index), "{sql}: {plan}");
            assert!(!plan.contains("SCAN"), "{sql}: {plan}");
        }
    }

    /// The fold, the migration's backfill and a rebuild agree on the keys,
    /// and `spec get --key` finds the newest-changed spec in any case.
    #[test]
    fn spec_keys_follow_the_head_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let ledger = Ledger::open_and_migrate(&path).unwrap();
        let actor = Actor {
            agent: Some("a".into()),
            session: Some("s".into()),
        };
        let at = |n: u32| format!("2026-11-07T00:00:0{n}.000Z");
        let body = |keys: &[&str]| {
            keys.iter()
                .enumerate()
                .map(|(i, k)| format!("## slice {}: s  ({k})\ndone when: x\n", i + 1))
                .collect::<String>()
        };
        let create = |parent: &str, b: String, n| {
            ledger
                .spec_create(&actor, parent, Some("t"), &b, &at(n), false)
                .unwrap()
                .unwrap()
                .id
        };
        let a = create("CORE-1", body(&["CORE-2", "o/r#3", "O/R#3"]), 1);
        let b = create("o/r#9", body(&["CORE-2"]), 2);
        assert_eq!(slice_spec(&ledger, " core-2 "), Some(b));
        assert_eq!(slice_spec(&ledger, "o/r#3"), Some(a));

        // A revise replaces the keys and makes `a` the newest-changed.
        ledger
            .spec_update(
                &actor,
                "core-1",
                &body(&["CORE-2", "CORE-4"]),
                "r",
                None,
                &at(3),
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(slice_spec(&ledger, "CORE-2"), Some(a));
        assert_eq!(slice_spec(&ledger, "CORE-4"), Some(a));
        assert_eq!(slice_spec(&ledger, "O/R#3"), None);
        let folded = keys(ledger.conn());
        assert_eq!(folded.len(), 3, "{folded:?}");

        ledger.rebuild().unwrap();
        assert_eq!(keys(ledger.conn()), folded);

        // A v2 file: migrating runs 0003 and its backfill, then 0004 and 0005.
        ledger
            .conn()
            .execute_batch(
                "DROP INDEX events_backlog; DROP INDEX events_dossier; DROP INDEX specs_key;
                 DROP INDEX lanes_source;
                 DROP TABLE spec_keys;
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        drop(ledger);
        let ledger = Ledger::open_and_migrate_with(&path, MIGRATIONS).unwrap();
        assert_eq!(keys(ledger.conn()), folded);
    }
}
