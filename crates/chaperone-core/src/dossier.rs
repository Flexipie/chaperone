//! The dossier of an issue key (ADR 0007): `dossier note` appends one
//! `dossier.noted` on the key, and `dossier get` reads what the ledger
//! already knows about it (the slice, lanes, handoffs, files) with the notes,
//! capped. Nothing is folded: the dossier is a read.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::checks::{Recorded, newest_other, recorded};
use crate::ledger::{
    Actor, Checked, EventKind, Ledger, LedgerError, Mutation, Payload, Refusal, exists,
    redact_payload,
};
use crate::pipeline::canonical_string;
use crate::spec::parse::{Decision, parse_decisions, parse_spec_sections};
use crate::spec::slice::{KEY_LANES, section_for, spec_for_key};
use crate::spec::{Spec, normalize_key, read as read_spec};
use crate::work::{HandoffInput, IS_HANDOFF, SPEC_BY_KEY, commit_ok, pr_ref_ok};

const LINK_KINDS: &[&str] = &["issue", "pr", "adr", "doc", "commit"];
/// The finding cap.
const NOTE_CHARS: usize = 600;
const LINKS: usize = 5;
/// Every other printed string, and a link's ref.
const TEXT_CHARS: usize = 300;
const LANES: usize = 10;
const HANDOFFS: usize = 5;
const HANDOFF_DECISIONS: usize = 5;
const SPEC_DECISIONS: usize = 20;
const FILES: usize = 100;
const NOTES: usize = 20;
/// Daisy's `context task` budget, on the answer as the CLI prints it
/// (the envelope, pretty). Bytes, so the 48,000 characters ADR 0007 names
/// hold too.
const ANSWER_BYTES: usize = 48_000;
/// What the envelope's `meta` takes, which the ledger does not build.
const META_BYTES: usize = 200;

/// A key's notes. Matches the `events_dossier` index (migration 0004) term
/// for term, or lookups scan every event.
const NOTE_EVENTS: &str = "kind = 'dossier.noted' AND json_extract(payload_json, '$.key') = ?1";

/// `dossier note`'s input. `links` are `<kind>:<ref>` as typed.
#[derive(Debug, Default)]
pub struct NoteInput {
    pub key: String,
    pub note: String,
    pub links: Vec<String>,
    pub supersedes: Option<i64>,
}

/// What was left out of a section, by list (and `strings` cut short).
pub type Omitted = BTreeMap<&'static str, usize>;

#[derive(Debug, Serialize)]
pub struct Dossier {
    pub key: String,
    pub slice: SliceSection,
    pub lanes: Section<LaneRow>,
    pub handoffs: HandoffSection,
    pub files: Section<FileRow>,
    pub notes: Section<NoteRow>,
}

#[derive(Debug, Serialize)]
pub struct Section<T> {
    pub items: Vec<T>,
    pub omitted: Omitted,
}

/// The slice whose heading names the key, and the spec whose parent the
/// key is. A key can be both.
#[derive(Debug, Serialize)]
pub struct SliceSection {
    pub in_spec: Option<InSpec>,
    pub parent_of: Option<ParentOf>,
    pub omitted: Omitted,
}

#[derive(Debug, Serialize)]
pub struct InSpec {
    pub spec_id: i64,
    pub parent_key: String,
    pub parent_title: String,
    pub version: i64,
    pub n: Option<u32>,
    pub title: String,
    /// This slice's decisions and the spec-wide ones, each its line.
    pub decisions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ParentOf {
    pub spec_id: i64,
    pub title: String,
    pub version: i64,
    pub slices: usize,
    pub decisions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LaneRow {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub branch: Option<String>,
    pub pr_ref: Option<String>,
    pub agent: Option<String>,
    pub pipeline_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Each lane's handoffs, newest lane and newest handoff first, and the
/// brief's decisions from each lane's newest handoff.
#[derive(Debug, Serialize)]
pub struct HandoffSection {
    pub items: Vec<HandoffRow>,
    pub decisions: Vec<HandoffDecision>,
    pub omitted: Omitted,
}

#[derive(Debug, Serialize)]
pub struct HandoffRow {
    pub work_id: i64,
    pub at: String,
    pub status: Option<String>,
    pub ready: Option<bool>,
    pub next_action: Option<String>,
    pub commit: Option<String>,
    pub pull_request: Option<String>,
    pub summary: String,
}

#[derive(Debug, Serialize)]
pub struct HandoffDecision {
    pub work_id: i64,
    pub decision: String,
}

#[derive(Debug, Serialize)]
pub struct FileRow {
    pub path: String,
    pub work_id: i64,
    pub commit: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NoteRow {
    pub id: i64,
    pub at: String,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<i64>,
}

fn invalid<T>(msg: String) -> Checked<T> {
    Ok(Err(Refusal::InvalidInput(msg)))
}

/// `normalize_key`'s form; notes store and match it lowercased.
fn key_of(raw: &str) -> Result<String, Refusal> {
    normalize_key(raw).ok_or_else(|| {
        Refusal::InvalidInput(format!(
            "key must be owner/repo#N or TEAM-123, got '{}'",
            raw.trim()
        ))
    })
}

/// A link's ref as stored, if it is an identifier of its kind.
fn link_ref(kind: &str, r: &str) -> Option<String> {
    if r.chars().count() > TEXT_CHARS {
        return None;
    }
    match kind {
        "issue" => normalize_key(r),
        "pr" => pr_ref_ok(r).then(|| r.to_string()),
        "commit" => commit_ok(r).then(|| r.to_string()),
        "adr" | "doc" => (!r.is_empty()
            && !r.starts_with('/')
            && !r.split('/').any(|p| p == "..")
            && !r.chars().any(char::is_control))
        .then(|| r.to_string()),
        _ => None,
    }
}

impl Ledger {
    /// One `dossier.noted` on a key. The idempotency key covers the actor,
    /// the payload as stored (redacted) and the key's newest note that says
    /// something else, so a retry replays, a restored note (A, B, A) appends,
    /// and another session's identical note appends.
    pub fn dossier_note(&self, actor: &Actor, input: &NoteInput) -> Checked<Recorded> {
        let key = match key_of(&input.key) {
            Ok(k) => k.to_lowercase(),
            Err(r) => return Ok(Err(r)),
        };
        let note = &input.note;
        if note.trim().is_empty() || note.chars().count() > NOTE_CHARS {
            return invalid(format!(
                "--note is non-blank and at most {NOTE_CHARS} characters"
            ));
        }
        if input.links.len() > LINKS {
            return invalid(format!("at most {LINKS} --link"));
        }
        let mut links = Vec::new();
        for l in &input.links {
            let stored = l
                .split_once(':')
                .and_then(|(k, r)| Some((k, link_ref(k, r)?)));
            let Some((kind, r)) = stored else {
                return invalid(format!(
                    "--link is <kind>:<ref>, kind one of {LINK_KINDS:?} and a ref of that type \
                     (a tracker key, owner/repo#N, a repo-relative path, a commit), got '{l}'"
                ));
            };
            links.push(json!({"kind": kind, "ref": r}));
        }
        let mut payload = Payload::default();
        payload.rest.insert("key".into(), json!(key));
        payload.rest.insert("note".into(), json!(note));
        if !links.is_empty() {
            payload.rest.insert("links".into(), links.into());
        }
        if let Some(id) = input.supersedes {
            payload.rest.insert("supersedes".into(), id.into());
        }
        // Hashed as the write will store it, so a note that had a secret
        // redacted still replays.
        let mut stored = payload.clone();
        match redact_payload(&mut stored)? {
            Ok(0) => {}
            Ok(n) => stored.redactions = Some(n),
            Err(r) => return Ok(Err(r)),
        }
        // Read before the write lock: a note that commits in between can
        // only make a retry append, never lose a write.
        let after = newest_other(
            self.conn(),
            &format!("SELECT id, payload_json FROM events WHERE {NOTE_EVENTS} ORDER BY id DESC"),
            [&key],
            &stored,
        )?;
        let canonical = canonical_string(&json!({
            "key": key, "agent": actor.agent, "session": actor.session,
            "payload": stored, "after": after,
        }));
        let idempotency_key = format!("dossier:{:x}", Sha256::digest(canonical));
        let (applied, _) = self.apply_planned(
            actor,
            Some(&idempotency_key),
            None,
            false,
            |conn, _| {
                if let Some(id) = input.supersedes
                    && !exists(
                        conn,
                        &format!("SELECT 1 FROM events WHERE {NOTE_EVENTS} AND id = ?2"),
                        params![key, id],
                    )?
                {
                    return invalid(format!("--supersedes {id} is not a note on {key}"));
                }
                Ok(Ok(Mutation::Event {
                    kind: EventKind::DossierNoted,
                    lane_id: None,
                    spec_id: None,
                    payload,
                }))
            },
            |_, _, _| Ok(()),
        )?;
        Ok(recorded(applied, None))
    }

    /// Everything the ledger knows about `key`, in one read transaction,
    /// capped (ADR 0007 "Small"). `not_found` when it has no slice, spec,
    /// lane or note.
    pub fn dossier_get(&self, key: &str) -> Checked<Dossier> {
        let key = match key_of(key) {
            Ok(k) => k,
            Err(r) => return Ok(Err(r)),
        };
        let tx = Transaction::new_unchecked(self.conn(), TransactionBehavior::Deferred)?;
        let slice = slice_section(&tx, &key)?;
        let lanes = lanes_section(&tx, &key)?;
        let (handoffs, files) = handoff_sections(&tx, &lanes.items)?;
        let notes = notes_section(&tx, &key.to_lowercase())?;
        // Before the budget cut: a key with any lane keeps its newest in the
        // caps, and its newest note is never superseded.
        if slice.in_spec.is_none()
            && slice.parent_of.is_none()
            && lanes.items.is_empty()
            && notes.items.is_empty()
        {
            return Ok(Err(Refusal::NotFoundRef {
                entity: "dossier for",
                reference: key,
            }));
        }
        let mut d = Dossier {
            key,
            slice,
            lanes,
            handoffs,
            files,
            notes,
        };
        fit(&mut d)?;
        Ok(Ok(d))
    }
}

/// `s` cut at `max` characters, counting the cut in `omitted.strings`.
fn cut(s: String, max: usize, omitted: &mut Omitted) -> String {
    if s.chars().count() <= max {
        return s;
    }
    *omitted.entry("strings").or_default() += 1;
    s.chars().take(max).collect()
}

fn cut_opt(s: Option<String>, omitted: &mut Omitted) -> Option<String> {
    s.map(|s| cut(s, TEXT_CHARS, omitted))
}

/// The newest `SPEC_DECISIONS` (the last ones), counting the rest under `name`.
fn decision_lines(mut d: Vec<Decision>, name: &'static str, omitted: &mut Omitted) -> Vec<String> {
    if d.len() > SPEC_DECISIONS {
        *omitted.entry(name).or_default() += d.len() - SPEC_DECISIONS;
        d.drain(..d.len() - SPEC_DECISIONS);
    }
    d.into_iter()
        .map(|d| cut(d.text, TEXT_CHARS, omitted))
        .collect()
}

fn slice_section(conn: &Connection, key: &str) -> Result<SliceSection, LedgerError> {
    let mut omitted = Omitted::new();
    let spec = |id: Option<i64>| -> Result<Option<Spec>, LedgerError> {
        id.map_or(Ok(None), |id| read_spec(conn, id, None))
    };
    let mut in_spec = None;
    if let Some(s) = spec(spec_for_key(conn, key)?)?
        && let Some((section, decisions)) = section_for(&s.body, key)
    {
        in_spec = Some(InSpec {
            spec_id: s.id,
            parent_key: s.linear_key,
            parent_title: cut(s.title, TEXT_CHARS, &mut omitted),
            version: s.version,
            n: section.n,
            title: cut(section.title, TEXT_CHARS, &mut omitted),
            decisions: decision_lines(decisions, "slice_decisions", &mut omitted),
        });
    }
    let parent = conn
        .query_row(SPEC_BY_KEY, [key], |r| r.get(0))
        .optional()?;
    let parent_of = spec(parent)?.map(|s| ParentOf {
        spec_id: s.id,
        slices: parse_spec_sections(&s.body).len(),
        decisions: decision_lines(parse_decisions(&s.body), "spec_decisions", &mut omitted),
        title: cut(s.title, TEXT_CHARS, &mut omitted),
        version: s.version,
    });
    Ok(SliceSection {
        in_spec,
        parent_of,
        omitted,
    })
}

/// The newest lanes on the key.
fn lanes_section(conn: &Connection, key: &str) -> Result<Section<LaneRow>, LedgerError> {
    let mut omitted = Omitted::new();
    let mut items = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT id, title, status, branch, pr_ref, agent, pipeline_id, created_at, updated_at
           FROM lanes WHERE {KEY_LANES} ORDER BY id DESC"
    ))?;
    let mut rows = stmt.query([key])?;
    while let Some(r) = rows.next()? {
        if items.len() == LANES {
            *omitted.entry("lanes").or_default() += 1;
            continue;
        }
        items.push(LaneRow {
            id: r.get(0)?,
            title: cut(r.get(1)?, TEXT_CHARS, &mut omitted),
            status: r.get(2)?,
            branch: cut_opt(r.get(3)?, &mut omitted),
            pr_ref: cut_opt(r.get(4)?, &mut omitted),
            agent: r.get(5)?,
            pipeline_id: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
        });
    }
    Ok(Section { items, omitted })
}

fn handoff_sections(
    conn: &Connection,
    lanes: &[LaneRow],
) -> Result<(HandoffSection, Section<FileRow>), LedgerError> {
    let mut h = HandoffSection {
        items: Vec::new(),
        decisions: Vec::new(),
        omitted: Omitted::new(),
    };
    let mut files = Section {
        items: Vec::new(),
        omitted: Omitted::new(),
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT at, json_extract(payload_json, '$.handoff') FROM events
          WHERE lane_id = ?1 AND {IS_HANDOFF} ORDER BY id DESC"
    ))?;
    for lane in lanes {
        let mut rows = stmt.query([lane.id])?;
        let mut n = 0;
        while let Some(r) = rows.next()? {
            n += 1;
            if n > HANDOFFS {
                *h.omitted.entry("handoffs").or_default() += 1;
                continue;
            }
            let raw: String = r.get(1)?;
            let ho: HandoffInput = serde_json::from_str(&raw).map_err(std::io::Error::other)?;
            if n == 1 {
                let decisions = ho.brief.as_ref().map(|b| b.decisions.clone());
                let decisions = decisions.unwrap_or_default();
                let skip = decisions.len().saturating_sub(HANDOFF_DECISIONS);
                if skip > 0 {
                    *h.omitted.entry("decisions").or_default() += skip;
                }
                for d in decisions.into_iter().take(HANDOFF_DECISIONS) {
                    let decision = cut(d, TEXT_CHARS, &mut h.omitted);
                    h.decisions.push(HandoffDecision {
                        work_id: lane.id,
                        decision,
                    });
                }
                for path in &ho.changed_files {
                    if files.items.len() == FILES {
                        *files.omitted.entry("files").or_default() += 1;
                        continue;
                    }
                    files.items.push(FileRow {
                        path: cut(path.clone(), TEXT_CHARS, &mut files.omitted),
                        work_id: lane.id,
                        commit: ho.commit.clone(),
                    });
                }
            }
            h.items.push(HandoffRow {
                work_id: lane.id,
                at: r.get(0)?,
                status: ho.status,
                ready: ho.ready,
                next_action: cut_opt(ho.next_action, &mut h.omitted),
                commit: ho.commit,
                pull_request: cut_opt(ho.pull_request, &mut h.omitted),
                summary: cut(ho.summary, TEXT_CHARS, &mut h.omitted),
            });
        }
    }
    Ok((h, files))
}

/// The newest notes on the (lowercased) key that no later note supersedes.
fn notes_section(conn: &Connection, key: &str) -> Result<Section<NoteRow>, LedgerError> {
    let mut omitted = Omitted::new();
    let mut items = Vec::new();
    let mut superseded = HashSet::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT id, actor_agent, actor_session, payload_json, at FROM events
          WHERE {NOTE_EVENTS} ORDER BY id DESC"
    ))?;
    let mut rows = stmt.query([key])?;
    // Newest first, and a note only supersedes an older one, so each
    // superseding note is read before the note it replaces.
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let mut p: Payload =
            serde_json::from_str(&r.get::<_, String>(3)?).map_err(std::io::Error::other)?;
        let supersedes = p.rest.get("supersedes").and_then(Value::as_i64);
        superseded.extend(supersedes);
        if superseded.contains(&id) {
            continue;
        }
        if items.len() == NOTES {
            *omitted.entry("notes").or_default() += 1;
            continue;
        }
        let note = match p.rest.remove("note") {
            Some(Value::String(s)) => s,
            _ => String::new(),
        };
        items.push(NoteRow {
            id,
            agent: r.get(1)?,
            session: r.get(2)?,
            at: r.get(4)?,
            note: cut(note, NOTE_CHARS, &mut omitted),
            links: p.rest.remove("links"),
            supersedes,
        });
    }
    Ok(Section { items, omitted })
}

fn json_len(v: &impl Serialize) -> Result<usize, LedgerError> {
    Ok(serde_json::to_string_pretty(v)
        .map_err(std::io::Error::other)?
        .len())
}

/// The answer as the CLI prints it, `meta` aside.
#[derive(Serialize)]
struct Printed<'a> {
    ok: bool,
    data: &'a Dossier,
}

/// The printed size of the answer, its envelope included.
fn printed_len(d: &Dossier) -> Result<usize, LedgerError> {
    Ok(json_len(&Printed { ok: true, data: d })? + META_BYTES)
}

/// Cuts the answer to `ANSWER_BYTES`. Ledger facts fill first, so the
/// last items go first: notes, files, handoff decisions, handoffs, lanes.
/// The slice is bounded by its own caps and never cut.
fn fit(d: &mut Dossier) -> Result<(), LedgerError> {
    loop {
        let mut size = printed_len(d)?;
        if size <= ANSWER_BYTES {
            return Ok(());
        }
        // Estimates the size down item by item, then measures again: an
        // item takes more indented in place, and the omitted counts it adds
        // take a few bytes.
        while size > ANSWER_BYTES {
            let dropped = if let Some(x) = d.notes.items.pop() {
                count(&mut d.notes.omitted, "notes", &x)?
            } else if let Some(x) = d.files.items.pop() {
                count(&mut d.files.omitted, "files", &x)?
            } else if let Some(x) = d.handoffs.decisions.pop() {
                count(&mut d.handoffs.omitted, "decisions", &x)?
            } else if let Some(x) = d.handoffs.items.pop() {
                count(&mut d.handoffs.omitted, "handoffs", &x)?
            } else if let Some(x) = d.lanes.items.pop() {
                count(&mut d.lanes.omitted, "lanes", &x)?
            } else {
                return Ok(());
            };
            size = size.saturating_sub(dropped);
        }
    }
}

/// Counts a dropped item and returns its bytes with its comma and line.
fn count(
    omitted: &mut Omitted,
    name: &'static str,
    item: &impl Serialize,
) -> Result<usize, LedgerError> {
    *omitted.entry(name).or_default() += 1;
    Ok(json_len(item)? + 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::MIGRATIONS;

    /// A v3 file migrates to v4, and the note lookup searches the index.
    #[test]
    fn note_lookup_uses_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        drop(Ledger::open_and_migrate_with(&path, &MIGRATIONS[..3]).unwrap());
        let ledger = Ledger::open_and_migrate(&path).unwrap();
        let sql = format!("EXPLAIN QUERY PLAN SELECT id FROM events WHERE {NOTE_EVENTS}");
        let plan: String = ledger
            .conn()
            .query_row(&sql, ["o/r#1"], |r| r.get(3))
            .unwrap();
        assert!(plan.contains("USING INDEX events_dossier"), "{plan}");
    }
}
