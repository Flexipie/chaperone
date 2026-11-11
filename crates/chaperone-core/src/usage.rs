//! `usage report` and `usage get`: R11's `usage.reported`, one event per
//! provider message, read from the lane's transcripts (origin
//! `transcript`; the R4 adapters write the same kind later). Totals are
//! sums over those events, never a stored counter.

mod scan;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value, json};

pub use scan::Session;

use crate::checks::lane_exists;
use crate::ledger::{
    Actor, Applied, Checked, EventKind, Ledger, LedgerError, Mutation, Payload, Refusal,
};
use crate::runner::Run;

const ENTITY: &str = "work item";

/// The transcript homes to read: the Claude config dir (`~/.claude`) and
/// the Codex home (`~/.codex`). At least one; each must exist.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
}

/// What `usage report` did. `existing` messages were already on the lane;
/// `pending` ones are still being written and wait for the next report.
#[derive(Clone, Debug, Serialize)]
pub struct Reported {
    pub work_id: i64,
    pub written: usize,
    pub existing: usize,
    pub pending: usize,
    pub sessions: Vec<Session>,
}

fn invalid<T>(msg: String) -> Checked<T> {
    Ok(Err(Refusal::InvalidInput(msg)))
}

impl Ledger {
    /// Scans the transcripts that started in lane `work`'s worktree and
    /// appends one `usage.reported` per message the lane does not have,
    /// each in its own transaction, keyed `usage:<lane>:<message_id>`. A
    /// rerun appends nothing; one cut short by a crash leaves whole events,
    /// and the next run adds the rest. A transcript it cannot read refuses
    /// the report before anything is written.
    pub fn usage_report(&self, actor: &Actor, work: i64, roots: &Roots) -> Checked<Reported> {
        let worktree: Option<Option<String>> = self
            .conn()
            .query_row(
                "SELECT worktree_path FROM lanes WHERE id = ?1",
                [work],
                |r| r.get(0),
            )
            .optional()?;
        let Some(worktree) = worktree else {
            return Ok(Err(Refusal::NotFound {
                entity: ENTITY,
                id: work,
            }));
        };
        let Some(stored) = worktree else {
            return invalid(format!(
                "work item {work} has no worktree_path, so no transcript can be matched to it"
            ));
        };
        // `/` or a relative path would match every transcript or none.
        let worktree = stored.trim_end_matches('/').to_string();
        if !worktree.starts_with('/') {
            return invalid(format!(
                "work item {work} has worktree_path {stored:?}; usage needs an absolute folder below /"
            ));
        }
        if roots.claude.is_none() && roots.codex.is_none() {
            return invalid(
                "no transcript folder to read; pass --claude-dir or --codex-dir".into(),
            );
        }
        let mut found = scan::Scan::default();
        type Read = fn(&Path, &str, &mut scan::Scan) -> std::io::Result<()>;
        for (root, read) in [
            (&roots.claude, scan::claude as Read),
            (&roots.codex, scan::codex),
        ] {
            let Some(root) = root else { continue };
            if !root.is_dir() {
                return invalid(format!("{} is not a folder", root.display()));
            }
            read(root, &worktree, &mut found)?;
        }
        let have: HashSet<String> = self
            .conn()
            .prepare(
                "SELECT idempotency_key FROM events
                 WHERE lane_id = ?1 AND kind = 'usage.reported'",
            )?
            .query_map([work], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut out = Reported {
            work_id: work,
            written: 0,
            existing: 0,
            pending: found.sessions.iter().map(|s| s.pending).sum(),
            sessions: found.sessions,
        };
        for usage in found.usage {
            let key = format!("usage:{work}:{}", usage.message_id);
            if have.contains(&key) {
                out.existing += 1;
                continue;
            }
            let payload = Payload {
                source: Some(usage.source.into()),
                rest: usage.fields,
                ..Payload::default()
            };
            match self.usage_event(actor, work, &key, payload)? {
                Applied::Committed(_) => out.written += 1,
                Applied::Replayed(_) => out.existing += 1,
                Applied::Refused(r) => return Ok(Err(r)),
                Applied::Heartbeat { .. } => unreachable!("usage_report writes an event"),
            }
        }
        Ok(Ok(out))
    }

    /// One `usage.reported` (source and origin `harness`) for a session
    /// the harness ran: `labels` say what ran it, then the session's id,
    /// cost and counts, and R11's token fields (Codex's when `labels` say
    /// `cli` `codex`, else Claude's). Its message is `run:<session id>`,
    /// so a retry appends nothing. A session that left no id wrote nothing
    /// to count.
    pub fn usage_run(
        &self,
        actor: &Actor,
        work: i64,
        mut labels: Map<String, Value>,
        run: &Run,
    ) -> Checked<()> {
        let Some(session) = &run.session_id else {
            return Ok(Ok(()));
        };
        let message_id = format!("run:{session}");
        let key = format!("usage:{work}:{message_id}");
        labels.extend([
            ("message_id".into(), json!(message_id)),
            ("origin".into(), json!("harness")),
            ("session_id".into(), json!(session)),
            ("cost_usd".into(), json!(run.cost_usd)),
            ("raw".into(), scan::counts(&run.model_usage)),
        ]);
        let u = &run.model_usage;
        let tokens = if labels.get("cli") == Some(&json!("codex")) {
            scan::codex_tokens(u)
        } else {
            // Claude's `modelUsage` counts per model: summed, and unknown
            // when any model lacks the count.
            let models = u.as_object().filter(|m| !m.is_empty());
            let sum = |k: &str| {
                models?
                    .values()
                    .map(|m| m.get(k).and_then(Value::as_u64))
                    .sum()
            };
            scan::claude_tokens([
                sum("inputTokens"),
                sum("cacheReadInputTokens"),
                sum("cacheCreationInputTokens"),
                sum("outputTokens"),
            ])
        };
        labels.extend(tokens.as_object().cloned().unwrap_or_default());
        let payload = Payload {
            source: Some("harness".into()),
            rest: labels,
            ..Payload::default()
        };
        Ok(match self.usage_event(actor, work, &key, payload)? {
            Applied::Refused(r) => Err(r),
            _ => Ok(()),
        })
    }

    /// Appends `payload` as a `usage.reported` on lane `work` under `key`,
    /// or replays the one stored there.
    fn usage_event(
        &self,
        actor: &Actor,
        work: i64,
        key: &str,
        payload: Payload,
    ) -> Result<Applied, LedgerError> {
        let (applied, _) = self.apply_planned(
            actor,
            Some(key),
            None,
            false,
            |_, _| {
                Ok(Ok(Mutation::Event {
                    kind: EventKind::UsageReported,
                    lane_id: Some(work),
                    spec_id: None,
                    payload,
                }))
            },
            |_, _, _| Ok(()),
        )?;
        Ok(applied)
    }

    /// Lane `work`'s usage: one row per transcript (`cli`, session, and the
    /// subagent when it is one), and totals per `source`. `known` is false
    /// when the lane has no usage event: unknown, not zero. A sum skips the
    /// messages where that field is null; a field null on every message
    /// stays null.
    pub fn usage_get(&self, work: i64) -> Checked<Value> {
        let conn = self.conn();
        if !lane_exists(conn, work)? {
            return Ok(Err(Refusal::NotFound {
                entity: ENTITY,
                id: work,
            }));
        }
        // The totals' fields, then what names a transcript row.
        // `cost_usd` is the CLI's own figure, which only harness rows carry.
        const SUMS: [&str; 8] = [
            "uncached_input",
            "cache_read",
            "cache_write",
            "output",
            "reasoning",
            "input_total",
            "output_total",
            "cost_usd",
        ];
        const NAMES: [&str; 4] = ["source", "cli", "session_id", "agent_id"];
        let f = |k: &str| format!("json_extract(payload_json, '$.{k}')");
        let rows = |group: &[&str], extra: &[(&str, String)]| {
            let mut cols = vec![
                ("messages".to_string(), "count(*)".to_string()),
                (
                    "peak_input_total".into(),
                    format!("max({})", f("input_total")),
                ),
            ];
            cols.extend(
                SUMS.iter()
                    .map(|k| (k.to_string(), format!("sum({})", f(k)))),
            );
            cols.extend(group.iter().map(|k| (k.to_string(), f(k))));
            cols.extend(extra.iter().map(|(k, e)| (k.to_string(), e.clone())));
            let select: Vec<&str> = cols.iter().map(|(_, e)| e.as_str()).collect();
            let by: Vec<String> = group.iter().map(|k| f(k)).collect();
            let sql = format!(
                "SELECT {} FROM events WHERE lane_id = ?1 AND kind = 'usage.reported'
                 GROUP BY {} ORDER BY min(id)",
                select.join(", "),
                by.join(", ")
            );
            let mut stmt = conn.prepare(&sql)?;
            let found = stmt.query_map(params![work], |r| {
                let mut m = Map::new();
                for (i, (name, _)) in cols.iter().enumerate() {
                    m.insert(name.clone(), sql_json(r.get(i)?));
                }
                Ok(m)
            })?;
            found.collect::<Result<Vec<_>, _>>()
        };
        let sources = rows(
            &NAMES,
            &[
                ("agent_type", format!("max({})", f("agent_type"))),
                ("description", format!("max({})", f("description"))),
                ("models", format!("group_concat(DISTINCT {})", f("model"))),
            ],
        )?;
        let mut totals = Map::new();
        for mut row in rows(&NAMES[..1], &[])? {
            // Codex events recorded before #214 hold the cache in
            // uncached_input, so the sum across CLIs means nothing; rows keep
            // it per CLI.
            row.remove("uncached_input");
            let source = row.remove("source");
            let source = source.as_ref().and_then(Value::as_str).unwrap_or_default();
            totals.insert(source.into(), row.into());
        }
        Ok(Ok(json!({
            "work_id": work,
            "known": !sources.is_empty(),
            "totals": totals,
            "sources": sources,
        })))
    }
}

fn sql_json(v: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as V;
    match v {
        V::Null => Value::Null,
        V::Integer(i) => json!(i),
        V::Real(f) => json!(f),
        V::Text(t) => json!(t),
        V::Blob(_) => Value::Null,
    }
}
