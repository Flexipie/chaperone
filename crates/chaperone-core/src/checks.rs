//! Check events: `check record` (a fresh session's checks), `check run`
//! (the harness's, measured by the binary), `reviews record` (a daisy
//! review job's result, as `daisy-cli reviews get` read it), and `reviews
//! get` / `jobs get` answering for recorded jobs in daisy's shapes
//! (events.md, checks). Also the moves of a review's findings and the
//! read of those that still block.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ledger::projection::{UNSETTLED, project};
use crate::ledger::{
    Actor, Applied, Checked, EventKind, Finding, Findings, Ledger, LedgerError, Mutation, Payload,
    Refusal, canonical_at, exists, redact_payload,
};
use crate::pipeline::{Fact, canonical_string};

const ENTITY: &str = "work item";
const JOB: &str = "job";
/// Daisy's cut on a finding body (`REVIEW_VIEW_BODY_CHARS`).
pub(crate) const BODY_CHARS: usize = 600;

/// Each session check and the reasons it may be skipped for.
pub const CHECKS: &[(&str, &[&str])] = &[
    ("plan_check", &["measure", "investigation", "docs", "tests"]),
    ("quality_gate", &["no_code", "tests"]),
    ("merge_verdict", &[]),
    ("smoke", &["nothing_runs", "attempt_failed"]),
    ("kept_tests", &["none"]),
    ("review_round", &[]),
];

/// The check events that carry a daisy job. Matches the `events_job`
/// index (migration 0002) term for term, or lookups scan every event.
const JOB_EVENTS: &str = "kind IN ('check.requested', 'check.finished')
      AND json_extract(payload_json, '$.job') = ?1";

#[derive(Clone, Debug, Default)]
pub struct CheckInput {
    pub work: i64,
    pub check: String,
    pub head: String,
    pub outcome: String,
    pub reason: Option<String>,
    pub decision: Option<String>,
    pub findings: Option<u64>,
}

/// A smoke's evidence files, as many as a handoff's smoke entries.
pub const EVIDENCE_FILES: usize = 12;

/// One evidence file a session attached to its check, in the evidence
/// store: its path in the lane's worktree, the stored text's hash and
/// what the store did to it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Attached {
    pub path: String,
    pub sha256: String,
    /// The file's size as read.
    pub bytes: u64,
    pub truncated: bool,
    pub redactions: u64,
}

/// `check run`'s input: what the harness measured on the lane's commit
/// `head`, whose tree is `tree`, and the executable that measured it.
#[derive(Clone, Debug, Default)]
pub struct RunInput {
    pub work: i64,
    pub check: String,
    pub head: String,
    pub tree: String,
    /// The sha256 of the profile the check ran by, for a check that reads
    /// one: a changed profile is a different run.
    pub profile: Option<String>,
    /// The check's own fields, as the payload carries them.
    pub fields: Map<String, Value>,
    /// A review's findings.
    pub findings: Option<Findings>,
    pub evidence_ref: Option<String>,
    pub bin: String,
    pub version: String,
}

pub const ATTENTION_TYPES: &[&str] = &["asked", "answered", "nudged", "sent_back", "revived"];
pub const ATTENTION_BY: &[&str] = &["orchestrator", "owner"];
pub const QUESTION_REASONS: &[&str] = &[
    "question.done_ambiguous",
    "question.scope",
    "question.one_way",
    "question.recorded_decision",
    "question.size",
    "question.investigation",
    "question.verdict_no",
    "question.product",
    "question.seam_reject",
    "question.overlap",
];
/// An attention note's cap, in characters: one line, not a document.
pub const NOTE_CHARS: usize = 500;

/// `attention record`'s input. `kind` is `--type`.
#[derive(Clone, Debug, Default)]
pub struct AttentionInput {
    pub work: i64,
    pub kind: String,
    pub by: String,
    pub reason: Option<String>,
    pub round: Option<u64>,
    pub answers: Option<i64>,
    pub note: Option<String>,
    pub idempotency_key: Option<String>,
}

/// `work merged`'s input: what the forge says about the merge.
#[derive(Clone, Debug, Default)]
pub struct MergedInput {
    pub work: i64,
    pub pr: String,
    pub merge_sha: String,
    pub head: String,
    pub merged_at: String,
    pub patch_id: Option<String>,
}

/// What `check record` (or `dossier note`) wrote, or the event a retry
/// replayed. A dossier note is on a key, so it has no `work_id`.
#[derive(Clone, Debug, Serialize)]
pub struct Recorded {
    pub event_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_id: Option<i64>,
    pub kind: &'static str,
    pub replayed: bool,
    pub at: String,
    pub payload: Payload,
}

/// A move of one finding, addressed as the ledger addresses it: the
/// `check.finished` that raised it and its key there.
#[derive(Clone, Debug, Default)]
pub struct FindingMove {
    pub check_event_id: i64,
    pub finding_key: String,
    pub status: String,
    /// Required to dispute or waive.
    pub reason: Option<String>,
    /// The commit that fixes it; required for `fixed`.
    pub head_sha: Option<String>,
}

/// The severities that do not block. Nothing checks the severity a review
/// writes, so the rest block, a misspelt or unknown one included: a read
/// that fails closed. `convention` may block under a stricter profile.
pub const NON_BLOCKING_SEVERITIES: &[&str] = &["nit", "improvement", "question", "convention"];

/// A finding that still blocks its lane ([`Ledger::blocking_findings`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockingFinding {
    pub check_event_id: i64,
    pub finding_key: String,
    pub severity: String,
    pub status: String,
    pub title: String,
}

fn invalid<T>(msg: String) -> Checked<T> {
    Ok(Err(Refusal::InvalidInput(msg)))
}

impl Ledger {
    /// One `check.finished` from a session check. The write refuses a
    /// session that has held the lane. Keyed as `session_check` says.
    pub fn check_record(&self, actor: &Actor, input: &CheckInput) -> Checked<Recorded> {
        self.check_record_with(actor, input, &[])
    }

    /// [`Ledger::check_record`] with the evidence files a smoke attaches,
    /// already stored: the lane's claim of what it ran and saw.
    pub fn check_record_with(
        &self,
        actor: &Actor,
        input: &CheckInput,
        files: &[Attached],
    ) -> Checked<Recorded> {
        if !files.is_empty() && input.check != "smoke" {
            return invalid("--evidence goes with smoke only".into());
        }
        if files.len() > EVIDENCE_FILES {
            return invalid(format!("--evidence takes at most {EVIDENCE_FILES} files"));
        }
        let Some((_, reasons)) = CHECKS.iter().find(|(c, _)| *c == input.check) else {
            return invalid(format!("check {} is not a session check", input.check));
        };
        let head = &input.head;
        if !sha_ok(head) {
            return invalid(format!(
                "--head must be 7 to 40 lowercase hex digits, got {head}"
            ));
        }
        let skipped = input.outcome == "skipped";
        let verdict = input.check == "merge_verdict";
        if verdict && skipped {
            return invalid("merge_verdict is ran only".into());
        }
        match &input.reason {
            None if skipped => return invalid(format!("skipped needs --reason {reasons:?}")),
            Some(_) if !skipped => return invalid("--reason goes with skipped only".into()),
            Some(r) if !reasons.contains(&r.as_str()) => {
                return invalid(format!("{} skips for {reasons:?}, not {r}", input.check));
            }
            _ => {}
        }
        if verdict != input.decision.is_some()
            || !matches!(input.decision.as_deref(), None | Some("yes" | "no"))
        {
            return invalid("--decision yes|no goes with merge_verdict, and only there".into());
        }
        let counted = matches!(input.check.as_str(), "plan_check" | "quality_gate");
        if input.findings.is_some() != (counted && !skipped) {
            return invalid("--findings <n> is required for a plan_check or quality_gate that ran, and only there".into());
        }
        let mut payload = Payload {
            check: Some(input.check.clone()),
            outcome: Some(input.outcome.clone()),
            head_sha: Some(head.clone()),
            findings: input.findings.map(Findings::Count),
            ..Payload::default()
        };
        for (k, v) in [("reason", &input.reason), ("decision", &input.decision)] {
            if let Some(v) = v {
                payload.rest.insert(k.into(), json!(v));
            }
        }
        if !files.is_empty() {
            payload.rest.insert("evidence_files".into(), json!(files));
        }
        self.session_check(actor, input.work, payload)
    }

    /// The owner's waiver of lane `work`'s plan check, on the commit `head`
    /// the lane is on: one `plan_check` `skipped` with `reason` `waived` and
    /// the owner's `note`, for a lane that coded before its plan was
    /// attacked. Like `check record`, refused from any session that has
    /// held the lane; the same waiver again replays.
    pub fn plan_waive(
        &self,
        actor: &Actor,
        work: i64,
        head: &str,
        note: &str,
    ) -> Checked<Recorded> {
        if !one_line(note) {
            return invalid(format!(
                "--reason is one non-blank line of at most {NOTE_CHARS} characters"
            ));
        }
        let mut payload = Payload {
            check: Some("plan_check".into()),
            outcome: Some("skipped".into()),
            head_sha: Some(head.into()),
            ..Payload::default()
        };
        payload.rest.extend([
            ("reason".into(), json!("waived")),
            ("note".into(), json!(note)),
        ]);
        self.session_check(actor, work, payload)
    }

    /// `payload` as a session's `check.finished` on lane `work`: its
    /// producer, then a key over the payload and the lane's newest different
    /// result for the check, so a retry replays but a result that flips
    /// back (no, yes, no) is appended.
    fn session_check(&self, actor: &Actor, work: i64, mut payload: Payload) -> Checked<Recorded> {
        payload.rest.insert(
            "producer".into(),
            json!({"kind": "session", "agent": actor.agent, "session": actor.session}),
        );
        // Hashed as the write will store it, so a waiver's note that had a
        // secret redacted still replays (as `dossier note`).
        let mut stored = payload.clone();
        match redact_payload(&mut stored)? {
            Ok(0) => {}
            Ok(n) => stored.redactions = Some(n),
            Err(r) => return Ok(Err(r)),
        }
        // Read before the write lock: a result that commits in between can
        // only make a retry append a duplicate.
        let latest = newest_other(
            self.conn(),
            "SELECT id, payload_json FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
               AND json_extract(payload_json, '$.check') = ?2 ORDER BY id DESC",
            params![work, payload.check],
            &stored,
        )?;
        let canonical =
            canonical_string(&json!({"work": work, "payload": stored, "after": latest}));
        let key = format!("check:{:x}", Sha256::digest(canonical));
        self.finish_check(actor, &key, work, payload)
    }

    /// One `check.finished` the harness produced (`check run`), `ran`, with
    /// producer `{kind: harness, bin, version}`, under [`Ledger::run_key`].
    pub fn check_run(&self, actor: &Actor, input: &RunInput) -> Checked<Recorded> {
        let key = self.run_key(input)?;
        self.finish_run(actor, input, true, &key)
    }

    /// [`Ledger::check_run`] under `key`, `skipped` when it did not `ran`
    /// (a session that gave no answer: its fields say why).
    pub(crate) fn finish_run(
        &self,
        actor: &Actor,
        input: &RunInput,
        ran: bool,
        key: &str,
    ) -> Checked<Recorded> {
        let mut payload = Payload {
            check: Some(input.check.clone()),
            outcome: Some(if ran { "ran" } else { "skipped" }.into()),
            head_sha: Some(input.head.clone()),
            evidence_ref: input.evidence_ref.clone(),
            findings: input.findings.clone(),
            rest: input.fields.clone(),
            ..Payload::default()
        };
        payload.rest.insert("tree".into(), json!(input.tree));
        if let Some(profile) = &input.profile {
            payload.rest.insert("profile".into(), json!(profile));
        }
        payload.rest.insert(
            "producer".into(),
            json!({"kind": "harness", "bin": input.bin, "version": input.version}),
        );
        self.finish_check(actor, key, input.work, payload)
    }

    /// `run:<lane>:<check>:<head>:<profile or ->:<attempt>`. The attempt is
    /// the id of the lane's newest result for this check that is not a pass
    /// on this commit and profile (0 for none): a pass replays, a failure
    /// or a review that is not complete runs again, and a run back on an earlier commit after another
    /// appends, as `check record`'s flip back does. Read before the write
    /// lock; the lane's run lock keeps two runs from reading it at once.
    pub fn run_key(&self, input: &RunInput) -> Result<String, LedgerError> {
        let attempt: i64 = self.conn().query_row(
            "SELECT coalesce(max(id), 0) FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
               AND json_extract(payload_json, '$.check') = ?2
               AND NOT (json_extract(payload_json, '$.head_sha') IS ?3
                 AND json_extract(payload_json, '$.profile') IS ?4
                 AND json_extract(payload_json, '$.result') IS NOT 'fail'
                 AND json_extract(payload_json, '$.complete') IS NOT 0)",
            params![input.work, input.check, input.head, input.profile],
            |r| r.get(0),
        )?;
        let (work, check, head) = (input.work, &input.check, &input.head);
        let profile = input.profile.as_deref().unwrap_or("-");
        Ok(format!("run:{work}:{check}:{head}:{profile}:{attempt}"))
    }

    /// The lane's newest harness `input.check` that ran on `input.tree` by
    /// `input.profile` with `field` equal to `value` (a digest of what it
    /// was given), replayed: a check that holds for a tree whatever the
    /// commit (`check run verdict`, `plan`).
    pub(crate) fn tree_run(
        &self,
        input: &RunInput,
        field: &str,
        value: &str,
    ) -> Result<Option<Recorded>, LedgerError> {
        let row: Option<(i64, String, String)> = self
            .conn()
            .query_row(
                "SELECT id, at, payload_json FROM events
                 WHERE lane_id = ?1 AND kind = 'check.finished'
                   AND json_extract(payload_json, '$.check') = ?2
                   AND json_extract(payload_json, '$.outcome') = 'ran'
                   AND json_extract(payload_json, '$.producer.kind') = 'harness'
                   AND json_extract(payload_json, '$.tree') = ?3
                   AND json_extract(payload_json, '$.profile') IS ?4
                   AND json_extract(payload_json, ?5) = ?6
                 ORDER BY id DESC LIMIT 1",
                params![
                    input.work,
                    input.check,
                    input.tree,
                    input.profile,
                    format!("$.{field}"),
                    value
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        row.map(|(event_id, at, payload)| {
            Ok(Recorded {
                event_id,
                work_id: Some(input.work),
                kind: EventKind::CheckFinished.as_str(),
                replayed: true,
                at,
                payload: serde_json::from_str(&payload).map_err(std::io::Error::other)?,
            })
        })
        .transpose()
    }

    /// `run:<lane>:<check>:<tree>:<profile>:<digest>:<attempt>` for a
    /// [`Ledger::tree_run`] check, the attempt the id of the lane's newest
    /// result for the check (0 for none): a retry after a crash keys the
    /// same, and a run after a session that gave no answer runs again.
    pub(crate) fn tree_key(&self, input: &RunInput, digest: &str) -> Result<String, LedgerError> {
        let attempt: i64 = self.conn().query_row(
            "SELECT coalesce(max(id), 0) FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
               AND json_extract(payload_json, '$.check') = ?2",
            params![input.work, input.check],
            |r| r.get(0),
        )?;
        let (work, check, tree) = (input.work, &input.check, &input.tree);
        let profile = input.profile.as_deref().unwrap_or("-");
        Ok(format!(
            "run:{work}:{check}:{tree}:{profile}:{digest}:{attempt}"
        ))
    }

    /// The result stored under a [`Ledger::run_key`], replayed, so a check
    /// that costs minutes is looked up before it runs.
    pub fn run_replay(&self, key: &str) -> Result<Option<Recorded>, LedgerError> {
        let row: Option<(i64, i64, String, String)> = self
            .conn()
            .query_row(
                "SELECT id, lane_id, at, payload_json FROM events
                 WHERE idempotency_key = ?1 AND kind = 'check.finished'",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        row.map(|(event_id, work, at, payload)| {
            Ok(Recorded {
                event_id,
                work_id: Some(work),
                kind: EventKind::CheckFinished.as_str(),
                replayed: true,
                at,
                payload: serde_json::from_str(&payload).map_err(std::io::Error::other)?,
            })
        })
        .transpose()
    }

    /// Appends `payload` as a `check.finished` on lane `work`, or replays
    /// the event stored under `key`.
    fn finish_check(
        &self,
        actor: &Actor,
        key: &str,
        work: i64,
        payload: Payload,
    ) -> Checked<Recorded> {
        let (applied, _) = self.apply_planned(
            actor,
            Some(key),
            None,
            false,
            |conn, _| {
                if !lane_exists(conn, work)? {
                    return Ok(Err(Refusal::NotFound {
                        entity: ENTITY,
                        id: work,
                    }));
                }
                Ok(Ok(check_event(EventKind::CheckFinished, work, payload)))
            },
            |_, _, _| Ok(()),
        )?;
        Ok(recorded(applied, Some(work)))
    }

    /// One `pr.merged`: PR `input.pr`, which the lane has held, merged as
    /// `merge_sha` from head `head` at `merged_at` (the forge's time). Keyed
    /// `merged:<lane>:<pr>`, so a retry replays. Refused for a PR the lane
    /// never held and for one already merged on another lane.
    pub fn work_merged(&self, actor: &Actor, input: &MergedInput) -> Checked<Recorded> {
        let pr = input.pr.trim();
        if !crate::work::pr_ref_ok(pr) {
            return invalid(format!("--pr must be owner/repo#N, got {pr}"));
        }
        for (flag, sha) in [("--merge-sha", &input.merge_sha), ("--head", &input.head)] {
            if !sha_ok(sha) {
                return invalid(format!(
                    "{flag} must be 7 to 40 lowercase hex digits, got {sha}"
                ));
            }
        }
        if let Some(p) = &input.patch_id
            && !sha_ok(p)
        {
            return invalid(format!(
                "--patch-id must be 7 to 40 lowercase hex digits, got {p}"
            ));
        }
        let work = input.work;
        let mut payload = Payload {
            head_sha: Some(input.head.clone()),
            ..Payload::default()
        };
        for (k, v) in [
            ("pr", Some(pr)),
            ("merge_sha", Some(input.merge_sha.as_str())),
            ("merged_at", Some(input.merged_at.as_str())),
            ("patch_id", input.patch_id.as_deref()),
        ] {
            payload.rest.insert(k.into(), json!(v));
        }
        let key = format!("merged:{work}:{}", pr.to_lowercase());
        let (applied, _) = self.apply_planned(
            actor,
            Some(&key),
            None,
            false,
            |conn, _| {
                if !lane_exists(conn, work)? {
                    return Ok(Err(Refusal::NotFound {
                        entity: ENTITY,
                        id: work,
                    }));
                }
                if !canonical_at(conn, &input.merged_at)? {
                    return invalid(format!(
                        "--merged-at must be RFC 3339 UTC with milliseconds, got {}",
                        input.merged_at
                    ));
                }
                // Stored as the lane held it, so a reader matching merges to
                // the lane's PRs by plain equality finds it.
                let held: Option<String> = conn
                    .query_row(
                        "SELECT iif(kind = 'pr.opened', json_extract(payload_json, '$.pr'),
                                json_extract(payload_json, '$.changed.pr_ref.to'))
                         FROM events WHERE lane_id = ?1 AND (
                           kind = 'pr.opened' AND lower(json_extract(payload_json, '$.pr')) = ?2
                           OR lower(json_extract(payload_json, '$.changed.pr_ref.to')) = ?2)
                         LIMIT 1",
                        params![work, pr.to_lowercase()],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(held) = held else {
                    return invalid(format!("work item {work} has never held PR {pr}"));
                };
                let mut payload = payload;
                payload.rest.insert("pr".into(), json!(held));
                let elsewhere = exists(
                    conn,
                    "SELECT 1 FROM events WHERE kind = 'pr.merged' AND lane_id != ?1
                       AND lower(json_extract(payload_json, '$.pr')) = ?2",
                    params![work, pr.to_lowercase()],
                )?;
                if elsewhere {
                    return invalid(format!("PR {pr} is already merged on another work item"));
                }
                Ok(Ok(check_event(EventKind::PrMerged, work, payload)))
            },
            |_, _, _| Ok(()),
        )?;
        Ok(recorded(applied, Some(work)))
    }

    /// One `attention.recorded`: an intervention on lane `input.work` by the
    /// orchestrator or the owner. Without `--idempotency-key` the key covers
    /// the payload and the lane's newest fact event, so a retry replays; a
    /// repeat with nothing new on the lane needs its own key to append.
    pub fn attention_record(&self, actor: &Actor, input: &AttentionInput) -> Checked<Recorded> {
        let kind = input.kind.as_str();
        if !ATTENTION_TYPES.contains(&kind) || !ATTENTION_BY.contains(&input.by.as_str()) {
            return invalid(format!(
                "--type is one of {ATTENTION_TYPES:?} and --by one of {ATTENTION_BY:?}"
            ));
        }
        match &input.reason {
            None if kind == "asked" => {
                return invalid(format!("asked needs --reason, one of {QUESTION_REASONS:?}"));
            }
            Some(r) if kind != "asked" || !QUESTION_REASONS.contains(&r.as_str()) => {
                return invalid(format!(
                    "--reason goes with asked only, one of {QUESTION_REASONS:?}"
                ));
            }
            _ => {}
        }
        if (kind == "sent_back") != input.round.is_some() || input.round == Some(0) {
            return invalid("--round <k>, 1 or more, goes with sent_back, and only there".into());
        }
        let answered = kind == "answered";
        if answered != input.answers.is_some() || answered && input.note.is_none() {
            return invalid("answered needs --answers <asked event id> and --note <the answer>; --answers goes with answered only".into());
        }
        if let Some(note) = &input.note
            && !one_line(note)
        {
            return invalid(format!(
                "--note is one non-blank line of at most {NOTE_CHARS} characters"
            ));
        }
        let work = input.work;
        let mut payload = Payload::default();
        let fields = [
            ("type", Some(json!(kind))),
            ("by", Some(json!(input.by))),
            ("reason", input.reason.as_ref().map(|r| json!(r))),
            ("round", input.round.map(|r| json!(r))),
            ("answers", input.answers.map(|a| json!(a))),
            ("note", input.note.as_ref().map(|n| json!(n))),
        ];
        for (k, v) in fields {
            if let Some(v) = v {
                payload.rest.insert(k.into(), v);
            }
        }
        // Read before the write lock: a fact that commits in between can
        // only make a retry append a duplicate.
        // As `work create`: a blank key is no key, and a key has a cap.
        let caller_key = crate::work::trimmed(&input.idempotency_key);
        if caller_key.as_ref().is_some_and(|k| k.len() > 200) {
            return invalid("idempotency_key is longer than 200 characters".into());
        }
        let key = match &caller_key {
            // Scoped to the lane and type, so the same key on another lane, or
            // a question and its answer keyed alike, each append.
            Some(k) => format!("attention:{work}:{kind}:{k}"),
            None => {
                let after = crate::engine::newest_fact_event(self.conn(), work)?;
                let canonical =
                    canonical_string(&json!({"work": work, "payload": payload, "after": after}));
                format!("attention:{:x}", Sha256::digest(canonical))
            }
        };
        let (applied, _) = self.apply_planned(
            actor,
            Some(&key),
            None,
            false,
            |conn, _| {
                if !lane_exists(conn, work)? {
                    return Ok(Err(Refusal::NotFound {
                        entity: ENTITY,
                        id: work,
                    }));
                }
                if let Some(asked) = input.answers
                    && !exists(
                        conn,
                        "SELECT 1 FROM events WHERE id = ?1 AND lane_id = ?2
                           AND kind = 'attention.recorded'
                           AND json_extract(payload_json, '$.type') = 'asked'",
                        params![asked, work],
                    )?
                {
                    return invalid(format!(
                        "--answers {asked} is not a question asked on work item {work}"
                    ));
                }
                Ok(Ok(check_event(EventKind::AttentionRecorded, work, payload)))
            },
            |_, _, _| Ok(()),
        )?;
        Ok(recorded(applied, Some(work)))
    }

    /// Records daisy job `job` on lane `work` from `view`, the `data` of
    /// `daisy-cli reviews get <job>`: `check.requested` while the job is
    /// not done, else one `check.finished` (`ran` for reviewed or posted,
    /// `skipped` for failed or cancelled). Returns `reviews get`'s answer.
    pub fn review_record(
        &self,
        actor: &Actor,
        work: i64,
        job: i64,
        view: &Value,
    ) -> Checked<Value> {
        let s = |k: &str| view.get(k).and_then(Value::as_str);
        if view.get("job_id").and_then(Value::as_i64) != Some(job) {
            return invalid(format!(
                "daisy answered for job {}, not {job}",
                view["job_id"]
            ));
        }
        let (Some(done), Some(status)) = (view.get("done").and_then(Value::as_bool), s("status"))
        else {
            return invalid(format!("daisy's view of job {job} has no done or status"));
        };
        let check = if s("source") == Some("local") {
            "local_review"
        } else {
            "pr_review"
        };
        let outcome = match (done, status) {
            (false, _) => None,
            (true, "reviewed" | "posted") => Some("ran"),
            (true, "failed" | "cancelled") => Some("skipped"),
            (true, other) => return invalid(format!("job {job} is done with status {other}")),
        };
        let mut p = Payload {
            check: Some(check.into()),
            head_sha: s("head_sha").map(String::from),
            outcome: outcome.map(String::from),
            ..Payload::default()
        };
        p.rest.insert("job".into(), json!(job));
        // Scalars only: daisy's free text (summary, impact, error) is not
        // stored.
        let review: Map<String, Value> = ["status", "phase", "source", "pr_ref", "branch", "repo"]
            .iter()
            .map(|k| (k.to_string(), view.get(*k).cloned().unwrap_or(Value::Null)))
            .collect();
        p.rest.insert("review".into(), review.into());
        if outcome.is_some() {
            p.rest.insert(
                "producer".into(),
                json!({"kind": "daisy_worker", "job": job}),
            );
        }
        match outcome {
            Some("ran") => {
                p.rest.insert("score".into(), view["score"].clone());
                p.rest.insert("verdict".into(), view["verdict"].clone());
                if check == "pr_review" {
                    p.rest.insert("pr".into(), view["pr_ref"].clone());
                }
                let mut list = Vec::new();
                for (i, f) in view["findings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let text = |k: &str| f.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
                    let (Some(id), Some(severity), Some(title)) =
                        (text("id"), text("severity"), text("title"))
                    else {
                        return invalid(format!(
                            "job {job} finding {i} has no id, severity or title"
                        ));
                    };
                    let mut rest = Map::new();
                    for k in ["line", "lenses", "verified", "body_truncated"] {
                        rest.insert(k.into(), f.get(k).cloned().unwrap_or(Value::Null));
                    }
                    list.push(Finding {
                        finding_key: id.into(),
                        severity: severity.into(),
                        title: title.into(),
                        body: text("body")
                            .unwrap_or("")
                            .chars()
                            .take(BODY_CHARS)
                            .collect(),
                        path: text("path").map(String::from),
                        rest,
                    });
                }
                p.findings = Some(Findings::List(list));
            }
            Some(_) => {
                p.rest.insert("reason".into(), json!(status));
            }
            None => {}
        }
        let (kind, stage) = match outcome {
            Some(_) => (EventKind::CheckFinished, "finished"),
            None => (EventKind::CheckRequested, "requested"),
        };
        let key = format!("review:{work}:{job}:{stage}");
        let (applied, answer) = self.apply_planned(
            actor,
            Some(&key),
            None,
            false,
            |conn, _| {
                let lane: Option<[Option<String>; 3]> = conn
                    .query_row(
                        "SELECT repo_slug, branch, pr_ref FROM lanes WHERE id = ?1",
                        [work],
                        |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?]),
                    )
                    .optional()?;
                let Some([repo, branch, pr_ref]) = lane else {
                    return Ok(Err(Refusal::NotFound {
                        entity: ENTITY,
                        id: work,
                    }));
                };
                // A local job carries the branch, a PR job the pr_ref (its
                // branch is null). Owner/repo ignores case on GitHub; branches
                // do not.
                let second = if check == "local_review" {
                    ("branch", branch)
                } else {
                    ("pr_ref", pr_ref)
                };
                for (field, ours) in [("repo", repo), second] {
                    let Some(ours) = ours else { continue };
                    let theirs = s(field).unwrap_or("");
                    let same = if field == "branch" {
                        theirs == ours
                    } else {
                        theirs.eq_ignore_ascii_case(&ours)
                    };
                    if !same {
                        return invalid(format!(
                            "job {job} has {field} {}, work item {work} has {ours}",
                            view[field]
                        ));
                    }
                }
                let other: Option<i64> = conn
                    .query_row(
                        &format!("SELECT lane_id FROM events WHERE {JOB_EVENTS} AND lane_id != ?2"),
                        params![job, work],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(other) = other {
                    return invalid(format!("job {job} is recorded on work item {other}"));
                }
                if outcome == Some("ran") && check == "local_review" {
                    // The engine's count (`review_round` checks too), so the
                    // stamp is the round the gates see.
                    let rounds = match project(conn, work, None)?.get("review.local_rounds") {
                        Fact::Int(n) => *n,
                        _ => 0,
                    };
                    p.rest.insert("round".into(), json!(rounds + 1));
                }
                Ok(Ok(check_event(kind, work, p)))
            },
            |conn, _, _| review_view(conn, job),
        )?;
        Ok(match applied {
            Applied::Refused(r) => Err(r),
            _ => answer.flatten().ok_or(Refusal::NotFound {
                entity: JOB,
                id: job,
            }),
        })
    }

    /// Daisy's `ReviewView` for a recorded job. Free text is null.
    pub fn review_get(&self, job: i64) -> Checked<Value> {
        Ok(review_view(self.conn(), job)?.ok_or(Refusal::NotFound {
            entity: JOB,
            id: job,
        }))
    }

    /// Daisy's `Job` for a recorded review job. What daisy alone knows
    /// (cost, tokens, its job dir) is null.
    pub fn job_get(&self, job: i64) -> Checked<Value> {
        let Some(view) = review_view(self.conn(), job)? else {
            return Ok(Err(Refusal::NotFound {
                entity: JOB,
                id: job,
            }));
        };
        let (work, started, finished): (i64, String, Option<String>) = self.conn().query_row(
            &format!(
                "SELECT lane_id, min(at), max(iif(kind = 'check.finished', at, NULL))
                 FROM events WHERE {JOB_EVENTS}"
            ),
            [job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let issue_key = match (&view["pr_ref"], &view["branch"]) {
            (Value::String(pr), _) => json!(pr),
            (_, Value::String(branch)) => json!(format!("local:{branch}")),
            _ => Value::Null,
        };
        let mut out = Map::new();
        let only_daisy = "base_sha blocking_questions_json cache_read_tokens cache_write_tokens \
            cost_usd gates_pass job_dir linear_failures_json mode model_phases not_before \
            not_ready_because open_blockers phase_started_at plan_kind progress_json queue_pos \
            ready repo_id request_json round scope_pass stranded tokens_in tokens_out turns";
        for k in only_daisy.split_whitespace() {
            out.insert(k.into(), Value::Null);
        }
        for (k, from) in [
            ("branch", "branch"),
            ("error", "error"),
            ("head_sha", "head_sha"),
            ("phase", "phase"),
            ("pr_ref", "pr_ref"),
            ("repo_slug", "repo"),
            ("review_score", "score"),
            ("review_verdict", "verdict"),
            ("source", "source"),
            ("status", "status"),
            ("worktree_path", "worktree_path"),
        ] {
            out.insert(k.into(), view[from].clone());
        }
        out.extend([
            ("id".into(), json!(job)),
            ("issue_key".into(), issue_key),
            ("kind".into(), json!("review")),
            ("started_at".into(), json!(started)),
            ("finished_at".into(), json!(finished)),
            ("work_id".into(), json!(work)),
        ]);
        Ok(Ok(out.into()))
    }

    /// One `finding.updated` on the finding's own lane. The write checks
    /// the move and who may make it (events.md, finding statuses). No key:
    /// a retry is the same move again, which the write refuses.
    pub fn finding_update(
        &self,
        actor: &Actor,
        input: &FindingMove,
        dry_run: bool,
    ) -> Checked<Recorded> {
        self.finding_move(actor, input, None, dry_run)
    }

    /// [`Ledger::finding_update`], or with `producer` the harness's ruling
    /// (`check run review`'s verifier), which only that code path sets.
    pub(crate) fn finding_move(
        &self,
        actor: &Actor,
        input: &FindingMove,
        producer: Option<&Value>,
        dry_run: bool,
    ) -> Checked<Recorded> {
        let mut payload = Payload {
            check_event_id: Some(input.check_event_id),
            finding_key: Some(input.finding_key.clone()),
            status: Some(input.status.clone()),
            head_sha: input.head_sha.clone(),
            ..Payload::default()
        };
        if let Some(reason) = &input.reason {
            payload.rest.insert("reason".into(), json!(reason));
        }
        if let Some(producer) = producer {
            payload.rest.insert("producer".into(), producer.clone());
        }
        let mut lane = None;
        let (applied, _) = self.apply_planned(
            actor,
            None,
            None,
            dry_run,
            |conn, _| {
                lane = conn
                    .query_row(
                        "SELECT lane_id FROM findings WHERE check_event_id = ?1 AND finding_key = ?2",
                        params![input.check_event_id, input.finding_key],
                        |r| r.get(0),
                    )
                    .optional()?;
                Ok(lane
                    .map(|id| check_event(EventKind::FindingUpdated, id, payload))
                    .ok_or_else(|| Refusal::NotFoundRef {
                        entity: "finding",
                        reference: format!("{}/{}", input.check_event_id, input.finding_key),
                    }))
            },
            |_, _, _| Ok(()),
        )?;
        Ok(recorded(applied, lane))
    }

    /// Lane `id`'s findings that still block, oldest first, from every
    /// round: unsettled (open, verified or disputed) and of a severity not
    /// in [`NON_BLOCKING_SEVERITIES`], compared exactly.
    pub fn blocking_findings(&self, id: i64) -> Result<Vec<BlockingFinding>, LedgerError> {
        let quiet: Vec<_> = NON_BLOCKING_SEVERITIES
            .iter()
            .map(|s| format!("'{s}'"))
            .collect();
        let mut stmt = self.conn().prepare(&format!(
            "SELECT check_event_id, finding_key, severity, status, title FROM findings
              WHERE lane_id = ?1 AND {UNSETTLED} AND severity NOT IN ({})
              ORDER BY id",
            quiet.join(", ")
        ))?;
        let rows = stmt.query_map([id], |r| {
            Ok(BlockingFinding {
                check_event_id: r.get(0)?,
                finding_key: r.get(1)?,
                severity: r.get(2)?,
                status: r.get(3)?,
                title: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The newest of `sql`'s events (`id, payload_json`, newest first) whose
/// stored payload is not `payload`. A retry finds the same one, so its key
/// replays (unless another session recorded the same since); a flip back
/// (no, yes, no) finds the event it flips from, so it appends.
pub(crate) fn newest_other(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
    payload: &Payload,
) -> Result<Option<i64>, LedgerError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(args)?;
    while let Some(r) = rows.next()? {
        let stored: Payload =
            serde_json::from_str(&r.get::<_, String>(1)?).map_err(std::io::Error::other)?;
        if &stored != payload {
            return Ok(Some(r.get(0)?));
        }
    }
    Ok(None)
}

/// What a one-event record command answers: the event it wrote or the one
/// a retry replayed, else the refusal.
pub(crate) fn recorded(applied: Applied, work_id: Option<i64>) -> Result<Recorded, Refusal> {
    let replayed = matches!(applied, Applied::Replayed(_));
    match applied {
        Applied::Committed(e) | Applied::Replayed(e) => Ok(Recorded {
            event_id: e.id,
            work_id,
            kind: e.kind.as_str(),
            replayed,
            at: e.at,
            payload: e.payload,
        }),
        Applied::Refused(r) => Err(r),
        Applied::Heartbeat { .. } => unreachable!("a record command writes an event"),
    }
}

/// A commit: 7 to 40 lowercase hex digits.
/// A note a person reads in a list: one non-blank line of at most
/// `NOTE_CHARS` characters (`attention record --note`, a finding's reason).
pub(crate) fn one_line(note: &str) -> bool {
    !note.trim().is_empty()
        && !note
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        && note.chars().count() <= NOTE_CHARS
}

pub(crate) fn sha_ok(sha: &str) -> bool {
    (7..=40).contains(&sha.len())
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

pub(crate) fn lane_exists(conn: &Connection, id: i64) -> Result<bool, LedgerError> {
    exists(conn, "SELECT 1 FROM lanes WHERE id = ?1", [id])
}

fn check_event(kind: EventKind, lane: i64, payload: Payload) -> Mutation {
    Mutation::Event {
        kind,
        lane_id: Some(lane),
        spec_id: None,
        payload,
    }
}

/// `reviews get` from the job's finish, or its request while it has none.
/// Findings come from the payload, each one's outcome from its row.
fn review_view(conn: &Connection, job: i64) -> Result<Option<Value>, LedgerError> {
    let row: Option<(i64, String, String)> = conn
        .query_row(
            &format!(
                "SELECT id, kind, payload_json FROM events WHERE {JOB_EVENTS}
                 ORDER BY kind = 'check.finished' DESC, id DESC LIMIT 1"
            ),
            [job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((event_id, kind, raw)) = row else {
        return Ok(None);
    };
    let p: Payload = serde_json::from_str(&raw).map_err(std::io::Error::other)?;
    let review = p.rest.get("review").cloned().unwrap_or(Value::Null);
    let mut findings = Vec::new();
    for f in p.finding_list() {
        let status: String = conn.query_row(
            "SELECT status FROM findings WHERE check_event_id = ?1 AND finding_key = ?2",
            params![event_id, f.finding_key],
            |r| r.get(0),
        )?;
        let mut v = json!({
            "id": f.finding_key, "severity": f.severity, "title": f.title, "path": f.path,
            "body": f.body, "outcome": (status != "open").then_some(status),
        });
        v.as_object_mut()
            .unwrap_or(&mut Map::new())
            .extend(f.rest.clone());
        findings.push(v);
    }
    let get = |k: &str| p.rest.get(k).cloned().unwrap_or(Value::Null);
    let done = kind == "check.finished";
    Ok(Some(json!({
        "job_id": job,
        "source": review["source"],
        "status": review["status"],
        "phase": review["phase"],
        "done": done,
        "repo": review["repo"],
        "pr_ref": review["pr_ref"],
        "branch": review["branch"],
        "head_sha": p.head_sha,
        "queue_ahead": done.then_some(0),
        "not_before": null,
        "score": get("score"),
        "verdict": get("verdict"),
        "verdict_reason": null,
        "summary": null,
        "impact": null,
        "intent_note": null,
        "findings": findings,
        "report_path": null,
        "worktree_path": null,
        "error": null,
        "error_summary": null,
        "deferred": null,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_lookup_uses_index() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let plan: String = ledger
            .conn()
            .query_row(
                &format!("EXPLAIN QUERY PLAN SELECT id FROM events WHERE {JOB_EVENTS}"),
                [7],
                |r| r.get(3),
            )
            .unwrap();
        assert!(plan.contains("USING INDEX events_job"), "{plan}");
    }

    /// The plan check's waiver is the owner's: refused from the lane's own
    /// session, written from another as a skipped `plan_check` with reason
    /// `waived` and the note redacted, which the plan check's facts read;
    /// the same again replays, and a note that is not one line is refused.
    #[test]
    fn a_plan_waiver_is_the_owners() {
        use crate::pipeline::Fact;
        use crate::work::{ClaimInput, CreateInput};
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_and_migrate(&dir.path().join("ledger.db")).unwrap();
        let actor = |s: &str| Actor {
            agent: Some("a".into()),
            session: Some(s.into()),
        };
        let input = CreateInput {
            title: "t".into(),
            source: Some("manual".into()),
            kind: "code".into(),
            pr_flow: "pr".into(),
            ..CreateInput::default()
        };
        ledger
            .work_create(&actor("lane"), &input, None, false)
            .unwrap()
            .unwrap();
        let claim = ClaimInput::default();
        ledger
            .work_claim(&actor("lane"), 1, &claim, None, false)
            .unwrap()
            .unwrap();
        let waive = |s: &str, note: &str| ledger.plan_waive(&actor(s), 1, "abc1234", note).unwrap();
        let refused = |s: &str, note: &str| match waive(s, note) {
            Err(Refusal::InvalidInput(why)) => why,
            other => format!("{other:?}"),
        };
        let said = refused("lane", "coded first");
        assert!(said.starts_with("session lane has held lane 1"), "{said}");
        let token = format!("ghp_{}", "a".repeat(20));
        let note = format!("a hotfix, {token}");
        let first = waive("owner", &note).unwrap();
        let p = &first.payload;
        let got = (p.check.as_deref(), p.outcome.as_deref(), &p.rest["reason"]);
        assert_eq!(got, (Some("plan_check"), Some("skipped"), &json!("waived")));
        assert_eq!(p.rest["note"], "a hotfix, [redacted:github_token]");
        assert_eq!(p.rest["producer"]["session"], "owner");
        let again = waive("owner", &note).unwrap();
        assert_eq!((again.replayed, again.event_id), (true, first.event_id));
        assert!(refused("owner", "a\nb").starts_with("--reason is one non-blank line"));
        let facts = ledger.project(1, None).unwrap();
        assert_eq!(facts.get("plan_check.reason"), &Fact::Str("waived".into()));
    }
}
