//! Check events: `check record` (a fresh session's checks), `reviews
//! record` (a daisy review job's result, as `daisy-cli reviews get` read
//! it), and `reviews get` / `jobs get` answering for recorded jobs in
//! daisy's shapes (events.md, checks).

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ledger::{
    Actor, Applied, Checked, EventKind, Finding, Findings, Ledger, LedgerError, Mutation, Payload,
    Refusal, exists,
};
use crate::pipeline::canonical_string;

const ENTITY: &str = "work item";
const JOB: &str = "job";
/// Daisy's cut on a finding body (`REVIEW_VIEW_BODY_CHARS`).
const BODY_CHARS: usize = 600;

/// Each session check and the reasons it may be skipped for.
const CHECKS: &[(&str, &[&str])] = &[
    ("plan_check", &["measure", "investigation", "docs", "tests"]),
    ("quality_gate", &["no_code", "tests"]),
    ("merge_verdict", &[]),
    ("smoke", &["nothing_runs", "attempt_failed"]),
    ("kept_tests", &["none"]),
];

/// The check events that carry a daisy job.
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

/// What `check record` wrote, or the event a retry replayed.
#[derive(Clone, Debug, Serialize)]
pub struct Recorded {
    pub event_id: i64,
    pub work_id: i64,
    pub kind: &'static str,
    pub replayed: bool,
    pub at: String,
    pub payload: Payload,
}

fn invalid<T>(msg: String) -> Checked<T> {
    Ok(Err(Refusal::InvalidInput(msg)))
}

impl Ledger {
    /// One `check.finished` from a session check. The write refuses a
    /// session that has held the lane. The key covers the payload and the
    /// lane's newest different result for this check, so a retry replays
    /// but a result that flips back (no, yes, no) is appended.
    pub fn check_record(&self, actor: &Actor, input: &CheckInput) -> Checked<Recorded> {
        let Some((_, reasons)) = CHECKS.iter().find(|(c, _)| *c == input.check) else {
            return invalid(format!("check {} is not a session check", input.check));
        };
        let head = &input.head;
        if !(7..=40).contains(&head.len())
            || !head
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        {
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
        payload.rest.insert(
            "producer".into(),
            json!({"kind": "session", "agent": actor.agent, "session": actor.session}),
        );
        // Read before the write lock: a result that commits in between can
        // only make a retry append a duplicate.
        let latest = newest_other(self.conn(), input.work, &input.check, &payload)?;
        let canonical =
            canonical_string(&json!({"work": input.work, "payload": payload, "after": latest}));
        let key = format!("check:{:x}", Sha256::digest(canonical));
        let (applied, _) = self.apply_planned(
            actor,
            Some(&key),
            None,
            false,
            |conn, _| {
                if !lane_exists(conn, input.work)? {
                    return Ok(Err(Refusal::NotFound {
                        entity: ENTITY,
                        id: input.work,
                    }));
                }
                Ok(Ok(check_event(
                    EventKind::CheckFinished,
                    input.work,
                    payload,
                )))
            },
            |_, _, _| Ok(()),
        )?;
        let replayed = matches!(applied, Applied::Replayed(_));
        Ok(match applied {
            Applied::Committed(e) | Applied::Replayed(e) => Ok(Recorded {
                event_id: e.id,
                work_id: input.work,
                kind: e.kind.as_str(),
                replayed,
                at: e.at,
                payload: e.payload,
            }),
            Applied::Refused(r) => Err(r),
            Applied::Heartbeat { .. } => unreachable!("check_record writes an event"),
        })
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
                if !lane_exists(conn, work)? {
                    return Ok(Err(Refusal::NotFound {
                        entity: ENTITY,
                        id: work,
                    }));
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
                    let rounds: i64 = conn.query_row(
                        "SELECT count(*) FROM events WHERE lane_id = ?1
                           AND kind = 'check.finished'
                           AND json_extract(payload_json, '$.check') = 'local_review'
                           AND json_extract(payload_json, '$.outcome') = 'ran'",
                        [work],
                        |r| r.get(0),
                    )?;
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
}

/// The newest result for `check` on the lane that says something other
/// than `payload`. A retry finds the same one, so its key replays; a flip
/// back finds the result it flips from, so it appends.
fn newest_other(
    conn: &Connection,
    lane: i64,
    check: &str,
    payload: &Payload,
) -> Result<Option<i64>, LedgerError> {
    let mut stmt = conn.prepare(
        "SELECT id, payload_json FROM events WHERE lane_id = ?1 AND kind = 'check.finished'
           AND json_extract(payload_json, '$.check') = ?2 ORDER BY id DESC",
    )?;
    let mut rows = stmt.query(params![lane, check])?;
    while let Some(r) = rows.next()? {
        let stored: Payload =
            serde_json::from_str(&r.get::<_, String>(1)?).map_err(std::io::Error::other)?;
        if &stored != payload {
            return Ok(Some(r.get(0)?));
        }
    }
    Ok(None)
}

fn lane_exists(conn: &Connection, id: i64) -> Result<bool, LedgerError> {
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
