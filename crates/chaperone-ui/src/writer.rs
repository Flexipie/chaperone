//! The owner's writes from the lane page (#109 slice 6): answer, send
//! back, open in herdr, merge (#117), pause; and from a repo's Backlog
//! (slice 9): start a lane, reorder. One thread with its own read-write
//! ledger, the only one the UI opens; the board keeps its read-only
//! handle. Every write is agent `owner-ui`, session `ui:<start time>`,
//! `--by owner`, the orchestrator's path: the herdr prompt first, then the
//! record, so a lane herdr cannot reach gets nothing recorded. Merge is
//! the exception, as `/merge` does it: GitHub first, then `pr.merged`,
//! then the herdr prompt to hand off done. A start is spawn's: the lane
//! writes its own create and claim.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chaperone_core::checks::{AttentionInput, MergedInput, NOTE_CHARS};
use chaperone_core::child;
use chaperone_core::ledger::{Actor, Ledger, LedgerError, Refusal};
use chaperone_core::pipeline::sha_prefix_eq;
use chaperone_core::spec::decide::DecideInput;
use chaperone_core::work::{HandoffInput, OWNER_UI, PAUSED_BY_OWNER, UpdateInput};
use serde_json::Value;

use crate::board::short;
use crate::watch::Shown;

pub const AGENT: &str = OWNER_UI;
/// How long one gh or herdr call may take.
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
/// spawn answers once the worktree and item exist; setup and the agent
/// start in the background.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(60);
const OUT_CAP: usize = 1 << 20;
const PAUSE: &str = "Paused by the owner: stop where you are and don't change the work item; \
                     wait for the owner.";

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
    /// Squash-merges `pr` at `head`, the handoff head the owner confirmed,
    /// once the ledger and GitHub both still hold it; a PR already merged
    /// at that head is only recorded. Then the lane hands off done.
    Merge {
        lane: Lane,
        pr: String,
        head: String,
    },
    /// Blocks a claimed or active lane, "paused by owner", over its
    /// agent's lease, as of `expected_updated_at`; its herdr `agent`, when
    /// it has one, is told first.
    Pause {
        work: i64,
        key: String,
        agent: Option<String>,
        expected_updated_at: String,
    },
    /// `spawn --repo <repo_path> --no-focus <key>`, `key` an
    /// `owner/repo#N` as the board spells the repo.
    Start {
        key: String,
        repo_path: String,
    },
    /// `repo`'s backlog order, built from its order `after`.
    Prioritise {
        repo: String,
        order: Vec<String>,
        after: Option<i64>,
    },
}

/// Where an act's outcome shows: a lane's page or a repo's Backlog.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Lane(i64),
    Repo(String),
}

impl Act {
    pub fn target(&self) -> Target {
        match self {
            Act::Answer { lane, .. } | Act::SendBack { lane, .. } | Act::Merge { lane, .. } => {
                Target::Lane(lane.work)
            }
            Act::Focus { work, .. } | Act::Pause { work, .. } => Target::Lane(*work),
            Act::Start { key, .. } => {
                Target::Repo(key.rsplit_once('#').map_or(key.as_str(), |(r, _)| r).into())
            }
            Act::Prioritise { repo, .. } => Target::Repo(repo.clone()),
        }
    }
}

/// An act's outcome in a line, for where it was.
pub struct Wrote {
    pub target: Target,
    pub said: Result<String, String>,
}

/// Starts the writer; `herdr` and `gh` are the CLIs, `spawn` the lane
/// starter. Each act's outcome lands in `shown.wrote`, then `changed` runs.
pub fn spawn(
    path: PathBuf,
    herdr: OsString,
    gh: OsString,
    spawn: OsString,
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
            let target = act.target();
            // A ledger missing at start is opened once it exists, as the
            // board's watcher does; kept once open.
            if open.is_err() {
                open = writable();
            }
            let said = match &open {
                Ok((ledger, actor)) => run(ledger, actor, &herdr, &gh, &spawn, act),
                Err(e) => Err(format!("ledger not opened for writing: {e}")),
            };
            let mut s = shown.lock().unwrap();
            s.writing = false;
            s.wrote = Some(Wrote { target, said });
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

/// The `name` CLI at `cmd` with `args`: its stdout, else its stderr. Run
/// from the temp dir, so gh never reads or changes a checkout (`-R`
/// names the repo). A call past [`CLI_TIMEOUT`] fails.
fn cli(name: &str, cmd: &OsString, args: &[&str]) -> Result<String, String> {
    cli_within(name, cmd, args, CLI_TIMEOUT, "")
}

/// [`cli`] with its own time limit; `late` ends the timeout's message.
fn cli_within(
    name: &str,
    cmd: &OsString,
    args: &[&str],
    limit: Duration,
    late: &str,
) -> Result<String, String> {
    let call = format!("{name} {}", args[..2].join(" "));
    let deadline = Instant::now() + limit;
    let mut command = Command::new(cmd);
    command.args(args).current_dir(std::env::temp_dir());
    let out = child::run(&mut command, deadline, OUT_CAP).map_err(|e| match e {
        child::Failed::Spawn(e) => format!("{name} did not start: {e}"),
        child::Failed::TimedOut => format!("{call}: no answer in {}s{late}", limit.as_secs()),
        child::Failed::Wait(e) => format!("{call}: {e}"),
    })?;
    if out.cut {
        return Err(format!("{call}: answer past {OUT_CAP} bytes"));
    }
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.out).into_owned());
    }
    let err = String::from_utf8_lossy(&out.err);
    Err(format!("{call}: {}", err.trim()))
}

/// One line of at most [`NOTE_CHARS`], what an attention note takes.
fn note(s: &str) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(NOTE_CHARS).collect()
}

pub fn run(
    ledger: &Ledger,
    actor: &Actor,
    herdr: &OsString,
    gh: &OsString,
    spawn: &OsString,
    act: Act,
) -> Result<String, String> {
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
            cli("herdr", herdr, &["workspace", "focus", &workspace])?;
            Ok(format!("Focused herdr workspace {workspace}."))
        }
        Act::Pause {
            work,
            key,
            agent,
            expected_updated_at,
        } => {
            let input = UpdateInput {
                status: Some("blocked".into()),
                blocker: Some(PAUSED_BY_OWNER.into()),
                ..UpdateInput::default()
            };
            let stamp = Some(expected_updated_at.as_str());
            // A dry run first, so a pause the ledger would refuse never tells
            // the agent to stop.
            done(ledger.work_update(actor, work, &input, stamp, None, true))?;
            if let Some(agent) = &agent {
                let late = "; the agent may have been told to stop, but the pause is not \
                            recorded: press Pause again";
                let args = ["agent", "prompt", agent, PAUSE];
                cli_within("herdr", herdr, &args, CLI_TIMEOUT, late)?;
            }
            done(ledger.work_update(actor, work, &input, stamp, None, false)).map_err(|e| {
                match &agent {
                    Some(agent) => format!(
                        "Told {agent} to stop, but the pause was not recorded: {e}. Press Pause \
                         again."
                    ),
                    None => e,
                }
            })?;
            Ok(format!("Paused {key}. Answer it to resume."))
        }
        Act::Start { key, repo_path } => start(spawn, &key, &repo_path),
        Act::Prioritise { repo, order, after } => {
            done(ledger.backlog_prioritise(actor, &repo, &order, after))?;
            Ok(format!("Saved {repo}'s order."))
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
            cli("herdr", herdr, &["agent", "prompt", &lane.agent, &prompt])?;
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
            cli("herdr", herdr, &["agent", "prompt", &lane.agent, &prompt])?;
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
        Act::Merge { lane, pr, head } => merge(ledger, actor, herdr, gh, lane, &pr, &head),
    }
}

/// Runs spawn and reads its one JSON line: `{status, work_ids, ...}`.
/// Only `started` started an agent: `focused` and `reopened` found the
/// worktree there, and another agent holding the item, or no line, fails.
fn start(spawn: &OsString, key: &str, repo_path: &str) -> Result<String, String> {
    let args = ["--repo", repo_path, "--no-focus", key];
    let late = "; the lane may still appear, check the board before starting it again";
    let out = cli_within("spawn", spawn, &args, SPAWN_TIMEOUT, late)?;
    let line: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    let ids = line["work_ids"].as_array().map(|ids| {
        let ids: Vec<String> = ids.iter().map(Value::to_string).collect();
        ids.join(", ")
    });
    match (line["status"].as_str(), ids) {
        (Some("claimed_elsewhere"), _) => Err(format!(
            "spawn {key}: another agent holds its work item; nothing started"
        )),
        (Some("started"), Some(ids)) => Ok(format!("Started {key}: started, work {ids}.")),
        (Some(status @ ("focused" | "reopened")), _) => Ok(format!(
            "spawn found {key}'s worktree already there ({status}); no agent was started: open \
             it in herdr."
        )),
        _ => Err(format!("spawn {key}: no status line in its answer")),
    }
}

fn merge(
    ledger: &Ledger,
    actor: &Actor,
    herdr: &OsString,
    gh: &OsString,
    lane: Lane,
    pr: &str,
    head: &str,
) -> Result<String, String> {
    let read = |e: LedgerError| e.to_string();
    let now = ledger.now().map_err(read)?;
    // What the confirm showed, read again now: a YES verdict on the
    // handoff head the owner confirmed, on the PR the lane holds.
    let page = ledger.lane_page(lane.work, Some(&lane.key)).map_err(read)?;
    let gates = page.gates.as_ref();
    let passed = |id: &str| gates.is_some_and(|g| g.passed(id));
    let handed = gates.and_then(|g| g.head.as_deref()).unwrap_or_default();
    let item = done(ledger.work_get(lane.work, &now))?;
    let board = ledger.work_board_item(lane.work, &now).map_err(read)?;
    let not = |why: String| Err(format!("Not merged: {why}."));
    if !passed("verdict_is_yes") {
        return not("the verdict is not YES".into());
    }
    if !passed("verdict_on_head") {
        return not(format!(
            "the verdict is not on the handoff head {}",
            short(handed)
        ));
    }
    if !sha_prefix_eq(head, handed, 7..=40, true) {
        return not(format!(
            "the lane handed off {} since you confirmed {}",
            short(handed),
            short(head)
        ));
    }
    if item.pr_ref.as_deref() != Some(pr) {
        return not(format!(
            "the lane's PR is now {}",
            item.pr_ref.as_deref().unwrap_or("none")
        ));
    }
    // A recorded merge is finished, whatever the lane says since.
    if page.merged.is_none() && !board.is_some_and(|b| b.ready) {
        return not("the lane is not ready".into());
    }
    let Some((repo, n)) = pr.split_once('#') else {
        return not(format!("{pr} is not owner/repo#N"));
    };
    let view = || -> Result<Value, String> {
        let fields = "state,headRefOid,mergeCommit,mergedAt";
        let out = cli("gh", gh, &["pr", "view", n, "-R", repo, "--json", fields])?;
        serde_json::from_str(&out).map_err(|e| format!("gh pr view: {e}"))
    };
    let mut v = view()?;
    let oid = v["headRefOid"].as_str().unwrap_or_default().to_string();
    if !sha_prefix_eq(head, &oid, 7..=40, false) {
        return not(format!(
            "the PR head moved to {}; the verdict is on {}",
            short(&oid),
            short(head)
        ));
    }
    // Never --admin or --auto; GitHub refuses a head that moved since.
    match v["state"].as_str().unwrap_or_default() {
        // Finish skipped the confirm and the ready check: it only records.
        "OPEN" if page.merged.is_some() => {
            return not(format!("{pr} is recorded as merged but GitHub has it open"));
        }
        "OPEN" => {
            let squash = ["--squash", "--delete-branch", "--match-head-commit", &oid];
            let merge = cli(
                "gh",
                gh,
                &[["pr", "merge", n, "-R", repo].as_slice(), &squash].concat(),
            );
            // gh can fail after GitHub merged (say, deleting the branch).
            v = match (view(), &merge) {
                (Ok(v), _) => v,
                (Err(e), Ok(_)) => {
                    return Err(format!("Merged on GitHub; {e} (Merge again records it)"));
                }
                (Err(_), Err(e)) => return Err(e.clone()),
            };
            if v["state"] != "MERGED" || v["headRefOid"] != oid.as_str() {
                merge?;
                return Err(format!("Not merged yet: GitHub has {pr} queued or open."));
            }
        }
        // A merge cut short before it was recorded: record it now.
        "MERGED" => {}
        state => return not(format!("{pr} is {state} on GitHub")),
    }
    let sha = v["mergeCommit"]["oid"].as_str().unwrap_or_default();
    let at = v["mergedAt"].as_str().unwrap_or_default();
    let input = MergedInput {
        work: lane.work,
        pr: pr.into(),
        merge_sha: sha.into(),
        head: oid,
        // GitHub gives seconds; the ledger takes milliseconds (cli-contract.md).
        merged_at: format!("{}.000Z", at.trim_end_matches('Z')),
        patch_id: None,
    };
    let merged = format!("Merged {pr} as {}", short(sha));
    done(ledger.work_merged(actor, &input)).map_err(|e| {
        format!("{merged} on GitHub; recording failed: {e} (Merge again records it).")
    })?;
    // Only the lane's agent hands off done while its lease is live;
    // owner-ui does once it is not.
    let now = ledger.now().map_err(read)?;
    let item = done(ledger.work_get(lane.work, &now))?;
    if item.status == "done" {
        return Ok(format!("{merged}."));
    }
    if item.lease_state == "active" {
        let prompt = format!(
            "Owner (chaperone-ui): merged {pr} as {sha} (work merged recorded). Hand off {} \
             done the /ship way.",
            lane.key
        );
        return Ok(
            match cli("herdr", herdr, &["agent", "prompt", &lane.agent, &prompt]) {
                Ok(_) => format!("{merged}; asked the lane to hand off done."),
                Err(e) => format!("{merged}; the lane hands off done ({e})."),
            },
        );
    }
    let newest = page.detail.handoffs.first().map(|h| h.handoff.clone());
    let input = HandoffInput {
        summary: format!("{merged} from chaperone-ui."),
        status: Some("done".into()),
        ready: Some(false),
        next_action: Some(format!("done: merged as {sha}")),
        commit: Some(handed.into()),
        ..newest.unwrap_or_default()
    };
    done(ledger.work_handoff(actor, lane.work, &input, None, None, false))
        .map_err(|e| format!("{merged}; the done handoff failed: {e}"))?;
    Ok(format!("{merged}; handed off done."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaperone_core::checks::CheckInput;
    use chaperone_core::work::{ClaimInput, CreateInput};

    /// An event's kind, agent, session, `by` and attention type.
    type Row = (String, String, String, Option<String>, Option<String>);

    /// Access (the test bar): every row the writer adds is agent
    /// `owner-ui`, session `ui:<start>`, attention by owner, a herdr call
    /// that fails records nothing, and a start is spawn's alone. A merge
    /// (the one-way door) runs only on a YES verdict on the head the owner
    /// confirmed, which GitHub still has.
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
        rw.spec_create(&lane, "o/r#1", None, body, None, false)
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
        // Merge lanes: a verdict on `verdict`, handed off at `commit` with
        // PR `o/r#<n>`, marked ready by the owner.
        let (head, older) = ("abc1234", "bbb2222");
        let ready_lane = |n: u32, decision: &str, verdict: &str, commit: &str| {
            let a = Actor {
                agent: Some(format!("m-{n}")),
                session: Some(format!("m{n}")),
            };
            let create = CreateInput {
                title: format!("merge {n}"),
                repo: Some("o/r".into()),
                source: Some("manual".into()),
                kind: "code".into(),
                pr_flow: "pr".into(),
                ..CreateInput::default()
            };
            let id = rw
                .work_create(&a, &create, None, false)
                .unwrap()
                .unwrap()
                .id;
            rw.work_claim(&a, id, &ClaimInput::default(), None, false)
                .unwrap()
                .unwrap();
            let verdict = CheckInput {
                work: id,
                check: "merge_verdict".into(),
                head: verdict.into(),
                outcome: "ran".into(),
                reason: None,
                decision: Some(decision.into()),
                findings: None,
            };
            // Checks and the override come from a session that never held it.
            let by = Actor {
                agent: Some("owner".into()),
                session: Some("cli:1".into()),
            };
            rw.check_record(&by, &verdict).unwrap().unwrap();
            let handoff = HandoffInput {
                summary: "ready".into(),
                commit: Some(commit.into()),
                pull_request: Some(format!("o/r#{n}")),
                ..HandoffInput::default()
            };
            rw.work_handoff(&a, id, &handoff, None, None, false)
                .unwrap()
                .unwrap();
            rw.work_ready(&by, id, Some(true), None, None, false)
                .unwrap()
                .unwrap();
            id
        };
        let no = ready_lane(5, "no", head, head);
        let old = ready_lane(6, "yes", older, head);
        let yes = ready_lane(7, "yes", head, head);
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
        // One fake for herdr, gh and spawn (each new script costs a scan
        // on macOS): it logs its argv, as gh answers `pr view` from
        // view.json, which a merge turns MERGED, and as spawn its line.
        let (calls, view, merged) = (
            dir.path().join("calls"),
            dir.path().join("view.json"),
            dir.path().join("merged.json"),
        );
        let herdr = fake(
            "herdr",
            &format!(
                "printf '%s|' \"$@\" >> {calls:?}; echo >> {calls:?}\n\
                 case \"$2\" in view) cat {view:?};; merge) cp {merged:?} {view:?};; esac\n\
                 case \"$1\" in --repo) echo '{{\"status\":\"started\",\"work_ids\":[7]}}';; esac"
            ),
        );
        let (gh, spawn) = (herdr.clone(), herdr.clone());
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
        let err = run(&rw, &actor, &down, &gh, &spawn, answer(Some(asked), None)).unwrap_err();
        assert!(err.contains("no socket"), "{err}");
        let updated_at = rw.work_get(work, &now).unwrap().unwrap().updated_at;
        let pause = || Act::Pause {
            work,
            key: "o/r#2".into(),
            agent: Some("r-2".into()),
            expected_updated_at: updated_at.clone(),
        };
        run(&rw, &actor, &down, &gh, &spawn, pause()).unwrap_err();
        assert_eq!(max(&db), before);
        let confirm = |spec: &str| Confirm {
            spec: spec.into(),
            slice: Some(1),
            question: "which way?".into(),
            answer: "left.".into(),
        };
        // The spec refuses the decision: the agent is told nothing.
        let gone = answer(None, Some(confirm("o/r#9")));
        run(&rw, &actor, &herdr, &gh, &spawn, gone).unwrap_err();
        assert!(!calls.exists());
        assert_eq!(max(&db), before);

        let run = |act| run(&rw, &actor, &herdr, &gh, &spawn, act);
        run(answer(Some(asked), None)).unwrap();
        // A self-blocked lane: asked, then answered.
        run(answer(None, None)).unwrap();
        run(answer(None, Some(confirm("o/r#1")))).unwrap();
        let back = Act::SendBack {
            lane: to.clone(),
            round: 1,
            reason: "no smoke".into(),
        };
        run(back).unwrap();

        let full = format!("{head}{}", "d".repeat(33));
        let pr_state = |path: &std::path::Path, state: &str, oid: &str| {
            let json = format!(
                r#"{{"state":"{state}","headRefOid":"{oid}","mergeCommit":{{"oid":"{}"}},"mergedAt":"2026-11-09T10:00:00Z"}}"#,
                "e".repeat(40)
            );
            std::fs::write(path, json).unwrap();
        };
        let merge = |work, n: u32, head: &str| Act::Merge {
            lane: Lane {
                work,
                agent: format!("m-{n}"),
                key: format!("o/r#{n}"),
            },
            pr: format!("o/r#{n}"),
            head: head.into(),
        };
        let gh_log = || {
            let log = std::fs::read_to_string(&calls).unwrap();
            let gh = log.lines().filter(|l| l.starts_with("pr|"));
            gh.map(|l| format!("{l}\n")).collect::<String>()
        };
        let mark = max(&db);
        for (act, why) in [
            (merge(no, 5, head), "the verdict is not YES"),
            (merge(old, 6, head), "not on the handoff head"),
            (merge(yes, 7, older), "since you confirmed"),
        ] {
            let err = run(act).unwrap_err();
            assert!(err.contains(why), "{err}");
        }
        assert_eq!(gh_log(), "");
        pr_state(&view, "OPEN", &format!("{older}{}", "d".repeat(33)));
        let err = run(merge(yes, 7, head)).unwrap_err();
        assert!(err.contains("the PR head moved"), "{err}");
        assert!(!gh_log().contains("pr|merge|"));
        assert_eq!(max(&db), mark);
        // YES on the current head: the exact merge, then one pr.merged.
        pr_state(&view, "OPEN", &full);
        pr_state(&merged, "MERGED", &full);
        run(merge(yes, 7, head)).unwrap();
        let argv =
            format!("pr|merge|7|-R|o/r|--squash|--delete-branch|--match-head-commit|{full}|");
        let merges = |log: String| log.lines().filter(|l| l.starts_with("pr|merge|")).count();
        assert_eq!(gh_log().lines().filter(|l| *l == argv).count(), 1);
        // Already merged at that head: recorded once, never merged again.
        run(merge(yes, 7, head)).unwrap();
        assert_eq!(merges(gh_log()), 1);
        let n: i64 = db
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'pr.merged' AND lane_id = ?1",
                [yes],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        // Recorded as merged but GitHub has it open: Finish never merges.
        pr_state(&view, "OPEN", &full);
        pr_state(&merged, "OPEN", &full);
        let err = run(merge(yes, 7, head)).unwrap_err();
        assert!(err.contains("recorded as merged"), "{err}");
        assert_eq!(merges(gh_log()), 1);
        // Over the lane's live lease.
        run(pause()).unwrap();
        let order = Act::Prioritise {
            repo: "O/R".into(),
            order: vec!["o/r#3".into(), "O/R#4".into()],
            after: None,
        };
        run(order).unwrap();
        let start = Act::Start {
            key: "o/r#3".into(),
            repo_path: "/src/r".into(),
        };
        assert_eq!(run(start).unwrap(), "Started o/r#3: started, work 7.");
        let focus = Act::Focus {
            work,
            workspace: "w1".into(),
        };
        run(focus).unwrap();

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
        // Two answers, an asked and its answer twice, the spec decision,
        // the send-back, the merge, the pause and the order: none left out
        // of the check below.
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[7].0, "pr.merged");
        assert_eq!(rows[8].0, "lane.updated");
        assert_eq!(rows[9].0, "backlog.prioritised");
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
        // Start and focus write no row: their calls only.
        let calls = std::fs::read_to_string(calls).unwrap();
        let last: Vec<&str> = calls.lines().rev().take(3).collect();
        assert_eq!(
            last,
            [
                "workspace|focus|w1|",
                "--repo|/src/r|--no-focus|o/r#3|",
                &format!("agent|prompt|r-2|{PAUSE}|"),
            ]
        );
    }
}
