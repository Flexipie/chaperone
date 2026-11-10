//! Daisy's handoff input and its validation, and the rows `work handoffs`
//! and `work events` print, in daisy's shapes.

use serde::{Deserialize, Serialize};

use crate::ledger::{Refusal, STATUSES};

const VERIFICATION_STATUSES: &[&str] = &["passed", "failed", "skipped"];
const SUMMARY_CHARS: usize = 4000;
const SMOKE_ENTRIES: usize = 12;
const SMOKE_WHAT_CHARS: usize = 300;
const SMOKE_SHOWS_CHARS: usize = 1000;
const SMOKE_EVIDENCE_CHARS: usize = 500;
/// The brief is read on a card; a lane that pastes its whole report into
/// it has missed the point.
const BRIEF_TEXT_CHARS: usize = 2000;
const BRIEF_ITEM_CHARS: usize = 500;
const BRIEF_ITEMS: usize = 20;
const LOOK_AT_PATH_CHARS: usize = 300;
/// Area names from the repo's pipeline profile; a profile has a handful.
const AREAS: usize = 20;
const AREA_CHARS: usize = 100;

/// Daisy's `HandoffInput`. Unknown fields are dropped, so only these are stored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HandoffInput {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    /// What the lane says it changes: areas of the repo's profile, compared
    /// with its diff by `impact --declared`. None is not declared, which an
    /// empty list would blur, so one is refused. Not checked against the
    /// profile here: the ledger reads none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub areas: Option<Vec<String>>,
    #[serde(default)]
    pub verification: Vec<Verification>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub smoke: Vec<Smoke>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<Brief>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    pub command: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Smoke {
    pub what: String,
    pub shows: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub merge_case: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub look_at: Vec<LookAt>,
    #[serde(default)]
    pub check_by_hand: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LookAt {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default)]
    pub why: String,
}

/// A handoff's `commit`, or a dossier link's: 7 to 64 hex characters.
pub(crate) fn commit_ok(c: &str) -> bool {
    (7..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit())
}

fn too_long(what: &str, s: &str, max: usize) -> Result<(), Refusal> {
    if s.chars().count() > max {
        return Err(Refusal::InvalidInput(format!(
            "{what} is longer than {max} characters"
        )));
    }
    Ok(())
}

fn invalid<T>(msg: String) -> Result<T, Refusal> {
    Err(Refusal::InvalidInput(msg))
}

impl HandoffInput {
    /// It moves the lane to ready: `ready: true`, and not a `done` handoff
    /// (which may still carry the flag after a merge).
    pub fn enters_ready(&self) -> bool {
        self.ready == Some(true) && self.status.as_deref() != Some("done")
    }

    /// Daisy's rules, in daisy's order and words.
    pub fn validate(&self) -> Result<(), Refusal> {
        let summary = self.summary.trim();
        if summary.is_empty() {
            return invalid("handoff summary is empty".into());
        }
        too_long("handoff summary", summary, SUMMARY_CHARS)?;
        if let Some(s) = &self.status
            && !STATUSES.contains(&s.as_str())
        {
            return invalid(format!(
                "handoff status must be one of {STATUSES:?}, got '{s}'"
            ));
        }
        if let Some(areas) = &self.areas {
            if areas.is_empty() {
                return invalid(
                    "handoff areas is empty; leave it out when none are declared".into(),
                );
            }
            if areas.len() > AREAS {
                return invalid(format!("handoff areas has more than {AREAS} entries"));
            }
            for a in areas {
                if a.trim().is_empty() {
                    return invalid("a handoff area is empty".into());
                }
                too_long("handoff area", a, AREA_CHARS)?;
            }
        }
        for v in &self.verification {
            if v.command.trim().is_empty() {
                return invalid("a verification entry has no command".into());
            }
            if !VERIFICATION_STATUSES.contains(&v.status.as_str()) {
                return invalid(format!(
                    "verification status must be one of {VERIFICATION_STATUSES:?}, got '{}'",
                    v.status
                ));
            }
        }
        if self.smoke.len() > SMOKE_ENTRIES {
            return invalid(format!("smoke has more than {SMOKE_ENTRIES} entries"));
        }
        for s in &self.smoke {
            if s.what.trim().is_empty() || s.shows.trim().is_empty() {
                return invalid("a smoke entry needs both what and shows".into());
            }
            too_long("smoke what", &s.what, SMOKE_WHAT_CHARS)?;
            too_long("smoke shows", &s.shows, SMOKE_SHOWS_CHARS)?;
            if let Some(e) = &s.evidence {
                too_long("smoke evidence", e, SMOKE_EVIDENCE_CHARS)?;
            }
        }
        if let Some(c) = &self.commit
            && !commit_ok(c)
        {
            return invalid(format!("commit must be 7 to 64 hex characters, got '{c}'"));
        }
        if let Some(pr) = &self.pull_request
            && !super::pr_ref_ok(pr)
            && !pr.starts_with("https://")
        {
            return invalid(format!(
                "pull_request must be owner/repo#N or an https URL, got '{pr}'"
            ));
        }
        if let Some(brief) = &self.brief {
            brief.validate()?;
        }
        Ok(())
    }
}

impl Brief {
    fn validate(&self) -> Result<(), Refusal> {
        too_long("brief goal", &self.goal, BRIEF_TEXT_CHARS)?;
        too_long("brief merge_case", &self.merge_case, BRIEF_TEXT_CHARS)?;
        for (what, list) in [
            ("decisions", &self.decisions),
            ("check_by_hand", &self.check_by_hand),
        ] {
            if list.len() > BRIEF_ITEMS {
                return invalid(format!("brief {what} has more than {BRIEF_ITEMS} entries"));
            }
            for s in list {
                too_long(&format!("brief {what}"), s, BRIEF_ITEM_CHARS)?;
            }
        }
        if self.look_at.len() > BRIEF_ITEMS {
            return invalid(format!("brief look_at has more than {BRIEF_ITEMS} entries"));
        }
        for l in &self.look_at {
            if l.path.trim().is_empty() {
                return invalid("a brief look_at entry has no path".into());
            }
            too_long("brief look_at path", &l.path, LOOK_AT_PATH_CHARS)?;
            too_long("brief look_at why", &l.why, BRIEF_ITEM_CHARS)?;
        }
        Ok(())
    }
}

/// Daisy's `Handoff` row. `id` is the id of the event that carries it;
/// `job_id` is always null.
#[derive(Clone, Debug, Serialize)]
pub struct Handoff {
    pub id: i64,
    pub work_id: Option<i64>,
    pub job_id: Option<i64>,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub tool: String,
    pub handoff: HandoffInput,
    pub created_at: String,
}

/// Daisy's `WorkEvent` row: `action` is daisy's word for the event kind,
/// `detail_json` the event's payload.
#[derive(Clone, Debug, Serialize)]
pub struct WorkEvent {
    pub id: i64,
    pub work_id: i64,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub tool: String,
    pub action: String,
    pub reason: Option<String>,
    pub detail_json: String,
    pub at: String,
}
