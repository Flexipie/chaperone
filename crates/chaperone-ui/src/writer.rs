//! The owner's writes from the lane page (#109 slice 6): answer, send
//! back, open in herdr. One thread with its own read-write ledger, the
//! only one the UI opens; the board keeps its read-only handle. Every
//! write is agent `owner-ui`, session `ui:<start time>`, `--by owner`,
//! the orchestrator's path: the herdr prompt first, then the record, so a
//! lane herdr cannot reach gets nothing recorded.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use chaperone_core::checks::{AttentionInput, NOTE_CHARS};
use chaperone_core::ledger::{Actor, Ledger, LedgerError, Refusal};
use chaperone_core::spec::decide::DecideInput;

use crate::watch::Shown;

pub const AGENT: &str = "owner-ui";

/// The lane a write is for: its work item, its herdr agent and its key.
#[derive(Clone)]
pub struct Lane {
    pub work: i64,
    pub agent: String,
    pub key: String,
}

/// A provisional spec decision the owner accepts.
pub struct Confirm {
    pub spec: String,
    pub slice: Option<u32>,
    pub question: String,
    pub answer: String,
}

pub enum Act {
    /// Answers question `asks`; with none, first records `asked` (the
    /// lane's blocker, `question.done_ambiguous`). With `confirm`, the
    /// answer confirms that decision in the spec.
    Answer {
        lane: Lane,
        asks: Option<i64>,
        blocker: Option<String>,
        text: String,
        confirm: Option<Confirm>,
    },
    SendBack {
        lane: Lane,
        round: u64,
        reason: String,
    },
    Focus {
        work: i64,
        workspace: String,
    },
}

impl Act {
    fn work(&self) -> i64 {
        match self {
            Act::Answer { lane, .. } | Act::SendBack { lane, .. } => lane.work,
            Act::Focus { work, .. } => *work,
        }
    }
}

/// An act's outcome in a line, for the lane it was on.
pub struct Wrote {
    pub work: i64,
    pub said: Result<String, String>,
}

/// Starts the writer; `herdr` is the herdr CLI. Each act's outcome lands
/// in `shown.wrote`, then `changed` runs.
pub fn spawn(
    path: PathBuf,
    herdr: OsString,
    shown: Arc<Mutex<Shown>>,
    changed: impl Fn() + Send + 'static,
) -> Sender<Act> {
    let (tx, rx) = mpsc::channel::<Act>();
    thread::spawn(move || {
        let writable = || {
            let l = Ledger::open(&path).map_err(|e| e.to_string())?;
            let now = l.now().map_err(|e| e.to_string())?;
            Ok::<_, String>((l, owner(&now)))
        };
        let mut open = writable();
        for act in rx {
            let work = act.work();
            // A ledger missing at start is opened once it exists, as the
            // board's watcher does; kept once open.
            if open.is_err() {
                open = writable();
            }
            let said = match &open {
                Ok((ledger, actor)) => run(ledger, actor, &herdr, act),
                Err(e) => Err(format!("ledger not opened for writing: {e}")),
            };
            let mut s = shown.lock().unwrap();
            s.writing = false;
            s.wrote = Some(Wrote { work, said });
            drop(s);
            changed();
        }
    });
    tx
}

/// The UI's actor for a window started at `start`.
pub fn owner(start: &str) -> Actor {
    Actor {
        agent: Some(AGENT.into()),
        session: Some(format!("ui:{start}")),
    }
}

fn done<T>(c: Result<Result<T, Refusal>, LedgerError>) -> Result<T, String> {
    match c {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(r)) => Err(format!("refused: {r:?}")),
        Err(e) => Err(e.to_string()),
    }
}

fn herdr_run(herdr: &OsString, args: &[&str]) -> Result<(), String> {
    let out = Command::new(herdr)
        .args(args)
        .output()
        .map_err(|e| format!("herdr did not start: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(format!("herdr {}: {}", args[..2].join(" "), err.trim()))
}

/// One line of at most [`NOTE_CHARS`], what an attention note takes.
fn note(s: &str) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(NOTE_CHARS).collect()
}

pub fn run(ledger: &Ledger, actor: &Actor, herdr: &OsString, act: Act) -> Result<String, String> {
    // Keyed by the act's time, as the orchestrator keys its lines: the
    // same note again is a new record, never a replay of an older one.
    let at = ledger.now().map_err(|e| e.to_string())?;
    let attention = |work, kind: &str| AttentionInput {
        work,
        kind: kind.into(),
        by: "owner".into(),
        idempotency_key: Some(at.clone()),
        ..AttentionInput::default()
    };
    match act {
        Act::Focus { workspace, .. } => {
            herdr_run(herdr, &["workspace", "focus", &workspace])?;
            Ok(format!("Focused herdr workspace {workspace}."))
        }
        Act::SendBack {
            lane,
            round,
            reason,
        } => {
            let reason = note(&reason);
            let prompt = format!(
                "Owner check (chaperone-ui), round {round}, on {}: {}. Fix it, run the \
                 /ship steps it needs (smoke, review round, handoff) and hand off again.",
                lane.key,
                reason.trim_end_matches('.')
            );
            herdr_run(herdr, &["agent", "prompt", &lane.agent, &prompt])?;
            let input = AttentionInput {
                round: Some(round),
                note: Some(reason),
                ..attention(lane.work, "sent_back")
            };
            done(ledger.attention_record(actor, &input))?;
            Ok(format!("Sent {} back, round {round}.", lane.key))
        }
        Act::Answer {
            lane,
            asks,
            blocker,
            text,
            confirm,
        } => {
            let text = note(&text);
            let prompt = match &confirm {
                Some(c) => format!(
                    "Owner (chaperone-ui): confirmed the provisional decision: {} -> {}. It is \
                     in the spec; carry on.",
                    c.question,
                    c.answer.trim_end_matches('.')
                ),
                None => format!(
                    "Owner (chaperone-ui): {}. Record it as /issue says for a resolved block \
                     (next action 'resolved: ...', and the decision in the spec), then carry on.",
                    text.trim_end_matches('.')
                ),
            };
            let decide = confirm.as_ref().map(|c| {
                let input = DecideInput {
                    slice: c.slice,
                    key: None,
                    question: c.question.clone(),
                    answer: c.answer.clone(),
                    agreed_where: Some("in chat (owner-ui)".into()),
                };
                (c.spec.as_str(), input)
            });
            // A decision the spec refuses is not told to the agent.
            if let Some((spec, input)) = &decide {
                done(ledger.spec_decide(actor, spec, input, None, true))?;
            }
            herdr_run(herdr, &["agent", "prompt", &lane.agent, &prompt])?;
            if let Some((spec, input)) = &decide {
                done(ledger.spec_decide(actor, spec, input, None, false))?;
            }
            let asks = match asks {
                Some(id) => id,
                None => {
                    // An accept asks about its decision, not the blocker.
                    let asked = match &confirm {
                        Some(c) => Some(note(&format!("confirm: {}", c.question))),
                        None => blocker.map(|b| note(&b)).filter(|b| !b.is_empty()),
                    };
                    let input = AttentionInput {
                        reason: Some("question.done_ambiguous".into()),
                        note: asked.or_else(|| Some("the lane needs you".into())),
                        ..attention(lane.work, "asked")
                    };
                    done(ledger.attention_record(actor, &input))?.event_id
                }
            };
            let input = AttentionInput {
                answers: Some(asks),
                note: Some(text),
                ..attention(lane.work, "answered")
            };
            done(ledger.attention_record(actor, &input))?;
            Ok(format!("Answered {}.", lane.key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaperone_core::work::{ClaimInput, CreateInput};

    /// An event's kind, agent, session, `by` and attention type.
    type Row = (String, String, String, Option<String>, Option<String>);

    /// Access (the test bar): every row the writer adds is agent
    /// `owner-ui`, session `ui:<start>`, attention by owner, and a herdr
    /// call that fails records nothing.
    #[test]
    fn writes_only_as_owner_ui() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let rw = Ledger::open_and_migrate(&path).unwrap();
        let lane = Actor {
            agent: Some("r-2".into()),
            session: Some("w1".into()),
        };
        let body = "# P\n\n## slice 1: one  (o/r#2)\n\n## decisions\n\n\
                    - [slice 1] Q: which way? A: left. (agreed: provisional: orchestrator, 2026-11-09)\n";
        let now = rw.now().unwrap();
        rw.spec_create(&lane, "o/r#1", None, body, &now, false)
            .unwrap()
            .unwrap();
        let create = CreateInput {
            title: "one".into(),
            repo: Some("o/r".into()),
            source: Some("github".into()),
            source_id: Some("o/r#2".into()),
            spec: Some("o/r#1".into()),
            kind: "code".into(),
            pr_flow: "pr".into(),
            ..CreateInput::default()
        };
        let work = rw
            .work_create(&lane, &create, None, false)
            .unwrap()
            .unwrap()
            .id;
        rw.work_claim(&lane, work, &ClaimInput::default(), None, false)
            .unwrap()
            .unwrap();
        let ask = AttentionInput {
            work,
            kind: "asked".into(),
            by: "owner".into(),
            reason: Some("question.scope".into()),
            note: Some("in scope?".into()),
            ..AttentionInput::default()
        };
        let asked = rw.attention_record(&lane, &ask).unwrap().unwrap().event_id;
        let db = rusqlite::Connection::open(&path).unwrap();
        let max = |db: &rusqlite::Connection| -> i64 {
            db.query_row("SELECT max(id) FROM events", [], |r| r.get(0))
                .unwrap()
        };
        let before = max(&db);

        let fake = |name: &str, body: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
            std::fs::set_permissions(&p, mode).unwrap();
            OsString::from(p)
        };
        let calls = dir.path().join("calls");
        let herdr = fake(
            "herdr",
            &format!("printf '%s|' \"$@\" >> {calls:?}; echo >> {calls:?}"),
        );
        let actor = owner(&now);
        let to = Lane {
            work,
            agent: "r-2".into(),
            key: "o/r#2".into(),
        };
        let answer = |asks, confirm| Act::Answer {
            lane: to.clone(),
            asks,
            blocker: Some("stuck on the seam".into()),
            text: "yes, in scope".into(),
            confirm,
        };
        // herdr fails: nothing recorded.
        let down = fake("down", "echo no socket >&2; exit 1");
        let err = run(&rw, &actor, &down, answer(Some(asked), None)).unwrap_err();
        assert!(err.contains("no socket"), "{err}");
        assert_eq!(max(&db), before);
        let confirm = |spec: &str| Confirm {
            spec: spec.into(),
            slice: Some(1),
            question: "which way?".into(),
            answer: "left.".into(),
        };
        // The spec refuses the decision: the agent is told nothing.
        let gone = answer(None, Some(confirm("o/r#9")));
        run(&rw, &actor, &herdr, gone).unwrap_err();
        assert!(!calls.exists());
        assert_eq!(max(&db), before);

        run(&rw, &actor, &herdr, answer(Some(asked), None)).unwrap();
        // A self-blocked lane: asked, then answered.
        run(&rw, &actor, &herdr, answer(None, None)).unwrap();
        run(&rw, &actor, &herdr, answer(None, Some(confirm("o/r#1")))).unwrap();
        let back = Act::SendBack {
            lane: to.clone(),
            round: 1,
            reason: "no smoke".into(),
        };
        run(&rw, &actor, &herdr, back).unwrap();
        let focus = Act::Focus {
            work,
            workspace: "w1".into(),
        };
        run(&rw, &actor, &herdr, focus).unwrap();

        let mut stmt = db
            .prepare(
                "SELECT kind, actor_agent, actor_session, json_extract(payload_json, '$.by'),
                        json_extract(payload_json, '$.type')
                   FROM events WHERE id > ?1 ORDER BY id",
            )
            .unwrap();
        let rows: Vec<Row> = stmt
            .query_map([before], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        // Two answers, an asked and its answer twice, the spec decision and
        // the send-back: none left out of the check below.
        assert_eq!(rows.len(), 7);
        for (kind, agent, session, by, _) in &rows {
            assert_eq!(agent, AGENT, "{kind}");
            assert_eq!(session, &format!("ui:{now}"), "{kind}");
            if kind == "attention.recorded" {
                assert_eq!(by.as_deref(), Some("owner"));
            }
        }
        // The blocker is asked for a typed answer, the decision for an accept.
        let asked: Vec<String> = db
            .prepare(
                "SELECT json_extract(payload_json, '$.note') FROM events
                  WHERE id > ?1 AND json_extract(payload_json, '$.type') = 'asked' ORDER BY id",
            )
            .unwrap()
            .query_map([before], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(asked, ["stuck on the seam", "confirm: which way?"]);
        // Focus writes no row: its herdr call only.
        let calls = std::fs::read_to_string(calls).unwrap();
        assert_eq!(calls.lines().last(), Some("workspace|focus|w1|"));
    }
}
