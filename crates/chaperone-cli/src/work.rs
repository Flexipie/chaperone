//! `chaperone work ...`: runs a parsed command against the ledger and
//! returns an `Outcome`; rendering is `output`'s.

use std::ffi::OsStr;
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::time::{Duration, Instant};

use chaperone_core::checks::{MergedInput, Recorded, never_held};
use chaperone_core::child;
use chaperone_core::cursor::Page;
use chaperone_core::ledger::{Actor, Ledger, Refusal};
use chaperone_core::work::{
    ClaimInput, CreateInput, HandoffInput, UpdateInput, WorkFilter, WorkItem,
};
use chaperone_core::{git, merge, work};
use serde::Serialize;
use serde_json::{Value, json};

use crate::checks::refuse;
use crate::cli::{ActorArgs, Format, WorkCmd};
use crate::output::{self, CliError, Meta, Outcome};

/// How long `work merged` waits for `gh pr view`.
const FORGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `work handoff` waits for `git rev-parse` in the worktree.
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(
    ledger: &Ledger,
    cmd: WorkCmd,
    meta: &mut Meta,
    format: Format,
) -> Result<Outcome, CliError> {
    let mut dry = |dry_run: bool| {
        if dry_run {
            meta.dry_run = Some(true);
        }
        dry_run
    };
    match cmd {
        WorkCmd::Create {
            title,
            source,
            source_id,
            repo,
            next_action,
            idempotency_key,
            spec,
            lane,
            lane_pos,
            kind,
            pr_flow,
            dry_run,
            actor,
        } => {
            let input = CreateInput {
                title,
                source,
                source_id,
                repo,
                next_action,
                idempotency_key,
                spec,
                lane_key: lane,
                lane_pos,
                kind,
                pr_flow,
            };
            one(ledger.work_create(&actor.resolve(), &input, None, dry(dry_run))??)
        }
        WorkCmd::Claim {
            id,
            worktree,
            branch,
            base_sha,
            lease_secs,
            takeover,
            pipeline,
            dry_run,
            actor,
        } => {
            let item = ledger.work_get(id, &ledger.now()?)??;
            let input = ClaimInput {
                worktree_path: worktree,
                branch,
                base_sha,
                lease_secs,
                takeover,
                pipeline,
                source_text: item
                    .pipeline_id
                    .is_none()
                    .then(|| source_text(&item))
                    .flatten(),
            };
            one(ledger.work_claim(&actor.resolve(), id, &input, None, dry(dry_run))??)
        }
        WorkCmd::Heartbeat { id, dry_run, actor } => {
            one(ledger.work_heartbeat(&actor.resolve(), id, None, dry(dry_run))??)
        }
        WorkCmd::Update {
            id,
            status,
            reason,
            blocker,
            next_action,
            pr,
            branch,
            base_sha,
            spec,
            lane,
            lane_pos,
            expected_updated_at,
            dry_run,
            actor,
        } => {
            let input = UpdateInput {
                status,
                reason,
                blocker,
                next_action,
                pr_ref: pr,
                branch,
                base_sha,
                spec,
                lane_key: lane,
                lane_pos,
                head_sha: None,
            };
            one(ledger.work_update(
                &actor.resolve(),
                id,
                &input,
                Some(&expected_updated_at),
                None,
                dry(dry_run),
            )??)
        }
        WorkCmd::Release { id, dry_run, actor } => {
            one(ledger.work_release(&actor.resolve(), id, None, dry(dry_run))??)
        }
        WorkCmd::Cancel {
            id,
            reason,
            expected_updated_at,
            dry_run,
            actor,
        } => one(ledger.work_cancel(
            &actor.resolve(),
            id,
            reason,
            expected_updated_at.as_deref(),
            None,
            dry(dry_run),
        )??),
        WorkCmd::Handoff {
            id,
            file,
            stdin: _,
            dry_run,
            actor,
        } => {
            let raw = match file.filter(|f| f.as_os_str() != "-") {
                Some(path) => std::fs::read_to_string(&path).map_err(|e| {
                    Refusal::InvalidInput(format!("could not read {}: {e}", path.display()))
                })?,
                None => read_stdin()?,
            };
            let input: HandoffInput = serde_json::from_str(&raw)
                .map_err(|e| Refusal::InvalidInput(format!("handoff JSON: {e}")))?;
            // Only a handoff that enters ready is checked against the worktree.
            let head = if input.enters_ready() {
                // An unknown item is the write's to refuse.
                let item = ledger.work_get(id, &ledger.now()?)?.ok();
                let (wt, branch) = item
                    .map(|i| (i.worktree_path, i.branch))
                    .unwrap_or_default();
                observed_head(wt.as_deref(), branch.as_deref())?
            } else {
                None
            };
            let actor = actor.resolve();
            let written =
                ledger.work_handoff(&actor, id, &input, head.as_deref(), None, dry(dry_run))??;
            if !input.enters_ready() || dry_run {
                return one(written);
            }
            let item = ledger.work_get(id, &ledger.now()?)??;
            let wt = item.worktree_path.as_deref().map(Path::new);
            let url = input.pull_request.as_deref().and_then(crate::checks::pr_of);
            let (Some(pr), Some(wt)) = (item.pr_ref.or(url), wt.filter(|wt| wt.is_absolute()))
            else {
                return one(written);
            };
            let reviewers = match crate::checks::request_reviewers(wt, &pr) {
                Ok(requested) => json!({"requested": requested}),
                Err(error) => json!({"error": error}),
            };
            let mut out = value(written)?;
            out["reviewers"] = reviewers;
            one(out)
        }
        WorkCmd::Handoffs { id } => items(ledger.work_handoffs(id)?),
        WorkCmd::Merged {
            id,
            pr,
            merge_sha,
            head,
            merged_at,
            patch_id,
            actor,
        } => {
            let actor = actor.resolve();
            let item = ledger.work_get(id, &ledger.now()?)??;
            if item.pipeline_id.as_deref() == Some("v21") {
                return one(merged_on_github(
                    ledger,
                    &actor,
                    &item,
                    pr.trim(),
                    patch_id,
                )??);
            }
            let input = {
                let given = |flag: &str, v: Option<String>| {
                    v.ok_or_else(|| refuse(format!("{flag} is required off v21")))
                };
                MergedInput {
                    work: id,
                    pr,
                    merge_sha: given("--merge-sha", merge_sha)?,
                    head: given("--head", head)?,
                    merged_at: given("--merged-at", merged_at)?,
                    patch_id,
                }
            };
            one(ledger.work_merged(&actor, &input)??)
        }
        WorkCmd::Events { id } => items(ledger.work_events(id)?),
        WorkCmd::Ready {
            id,
            yes,
            no: _,
            clear,
            reason,
            dry_run,
            actor,
        } => {
            // The group makes exactly one of the three true.
            let ready = (!clear).then_some(yes);
            let dry_run = dry(dry_run);
            ledger.work_ready(&actor.resolve(), id, ready, reason, None, dry_run)??;
            if dry_run {
                return one(json!({ "id": id, "ready_override": ready }));
            }
            // Daisy's answer: what the board now says, or for an item off
            // it (done or cancelled) the override echoed.
            match ledger.work_board_item(id, &ledger.now()?)? {
                Some(b) => {
                    one(json!({ "id": id, "ready": b.ready, "ready_reason": b.ready_reason }))
                }
                None => one(json!({ "id": id, "ready_override": ready, "on_board": false })),
            }
        }
        WorkCmd::Get { id } => one(ledger.work_get(id, &ledger.now()?)??),
        WorkCmd::Board => output::write_board(ledger, &ledger.now()?, meta, format),
        WorkCmd::List {
            status,
            agent,
            mine,
            all,
            limit,
            cursor,
            session,
            mine_agent,
        } => {
            let (agent, session) = if mine {
                let me = ActorArgs {
                    agent: mine_agent,
                    session,
                }
                .resolve();
                if me.agent.is_none() || me.session.is_none() {
                    return Err(Refusal::InvalidInput(
                        "--mine needs an agent and a session (--mine-agent/--session or \
                         DAISY_AGENT/DAISY_SESSION)"
                            .into(),
                    )
                    .into());
                }
                (me.agent, me.session)
            } else {
                (agent, None)
            };
            let filter = WorkFilter {
                status,
                agent,
                session,
                include_terminal: all,
            };
            let page = ledger.work_list(&filter, limit, cursor.as_deref(), &ledger.now()?)??;
            Ok(Outcome::Page(Page {
                items: page.items.iter().map(value).collect::<Result<_, _>>()?,
                next_cursor: page.next_cursor,
                has_more: page.has_more,
            }))
        }
    }
}

/// Daisy's `arg_or_stdin`: a terminal on stdin is a usage error, not a hang.
fn read_stdin() -> Result<String, CliError> {
    if std::io::stdin().is_terminal() {
        return Err(CliError::Usage {
            message: "no handoff file and stdin is a terminal; pass a file or pipe the JSON in"
                .into(),
            kind: "MissingRequiredArgument".into(),
            help: String::new(),
        });
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| CliError::Internal(format!("reading stdin: {e}")))?;
    Ok(buf)
}

pub(crate) fn items(v: Vec<impl Serialize>) -> Result<Outcome, CliError> {
    Ok(Outcome::Page(Page {
        items: v.into_iter().map(value).collect::<Result<_, _>>()?,
        next_cursor: None,
        has_more: false,
    }))
}

/// The commit the lane is on, read just before a ready handoff is written:
/// the tip of its `branch` (stack layers share one worktree, so `HEAD` may
/// be a later layer's), or `HEAD` when the item names no branch or the
/// worktree does not have it. None when the lane has no worktree or it is
/// gone (removed after a merge): there is nothing to compare with. A
/// worktree that is there and cannot be read refuses, so a broken checkout
/// is not a pass.
pub(crate) fn observed_head(
    worktree: Option<&str>,
    branch: Option<&str>,
) -> Result<Option<String>, Refusal> {
    // A relative path would be read against this process's directory.
    let Some(wt) = worktree
        .map(Path::new)
        .filter(|wt| wt.is_absolute() && wt.is_dir())
    else {
        return Ok(None);
    };
    // One or two calls, so plain `git`: finding the real binary costs one too.
    let (bin, deadline) = (Path::new("git"), Instant::now() + HEAD_TIMEOUT);
    let tip = match branch {
        Some(b) => git::commit(bin, wt, &format!("refs/heads/{b}"), deadline),
        None => Ok(None),
    };
    tip.and_then(|tip| match tip {
        Some(tip) => Ok(tip),
        None => git::head(bin, wt, deadline),
    })
    .map(Some)
    .map_err(|e| {
        Refusal::InvalidInput(format!(
            "could not read the commit in {}: {e}",
            wt.display()
        ))
    })
}

pub(crate) fn value(v: impl Serialize) -> Result<Value, CliError> {
    serde_json::to_value(v).map_err(|e| CliError::Internal(e.to_string()))
}

pub(crate) fn one(v: impl Serialize) -> Result<Outcome, CliError> {
    Ok(Outcome::One(value(v)?))
}

/// A github lane's issue as `gh` reads it now (`{title, body}`), or why it
/// cannot (`{error}`): a lane starts either way, and its review is told
/// which. None for a lane of another source.
fn source_text(item: &WorkItem) -> Option<Value> {
    if item.source != "github" {
        return None;
    }
    let id = item.source_id.as_deref().unwrap_or_default();
    let read = || {
        let (repo, n) = id
            .rsplit_once('#')
            .filter(|_| work::pr_ref_ok(id))
            .ok_or(format!("source_id must be owner/repo#N, got {id}"))?;
        let args = ["issue", "view", n, "-R", repo, "--json", "title,body"];
        let out = child::cli("gh", OsStr::new("gh"), &args, FORGE_TIMEOUT, "")?;
        let v: Value = serde_json::from_str(&out).map_err(|e| format!("gh issue view: {e}"))?;
        Ok(json!({"title": v["title"], "body": v["body"]}))
    };
    Some(read().unwrap_or_else(|e: String| json!({"error": e})))
}

/// A v21 lane's merge as GitHub shows it (#308): the PR must be in the
/// lane's repo, not from a fork, from the lane's branch and merged, and its
/// merge commit, head and time are GitHub's. A lane with a `pr_ref` records
/// only that PR. A lane with none takes the PR as its `pr_ref` only once the
/// ledger refuses it for never held alone, so any other refusal leaves it.
fn merged_on_github(
    ledger: &Ledger,
    actor: &Actor,
    item: &WorkItem,
    pr: &str,
    patch_id: Option<String>,
) -> Result<Result<Recorded, Refusal>, CliError> {
    let (repo, n) = match pr.rsplit_once('#') {
        Some(ref_) if work::pr_ref_ok(pr) => ref_,
        _ => return Err(refuse(format!("--pr must be owner/repo#N, got {pr}"))),
    };
    if let Some(held) = item
        .pr_ref
        .as_deref()
        .filter(|h| !h.eq_ignore_ascii_case(pr))
    {
        return Err(refuse(format!("{pr} is not the lane's PR {held}")));
    }
    let fields = "state,headRefName,headRefOid,isCrossRepository,mergeCommit,mergedAt";
    let args = ["pr", "view", n, "-R", repo, "--json", fields];
    let out =
        child::cli("gh", OsStr::new("gh"), &args, FORGE_TIMEOUT, "").map_err(CliError::Internal)?;
    let v: Value =
        serde_json::from_str(&out).map_err(|e| CliError::Internal(format!("gh pr view: {e}")))?;
    let (lane_repo, lane_branch) = (
        item.repo_slug.as_deref().unwrap_or("(none)"),
        item.branch.as_deref().unwrap_or("(none)"),
    );
    let branch = v["headRefName"].as_str().unwrap_or_default();
    let why = if !item
        .repo_slug
        .as_deref()
        .is_some_and(|r| r.eq_ignore_ascii_case(repo))
    {
        format!("is not in the lane's repo {lane_repo}")
    } else if v["isCrossRepository"] != json!(false) {
        "has its head in another repository on GitHub".into()
    } else if item.branch.as_deref() != Some(branch) {
        format!("has head branch {branch} on GitHub, not the lane's branch {lane_branch}")
    } else if v["state"] != "MERGED" {
        "is not merged on GitHub".into()
    } else {
        String::new()
    };
    if !why.is_empty() {
        return Err(refuse(format!("{pr} {why}")));
    }
    let head = v["headRefOid"].as_str().unwrap_or_default().to_string();
    let input = MergedInput {
        patch_id,
        ..merge::merged_input(item.id, pr, head, &v)
    };
    Ok(match ledger.work_merged(actor, &input)? {
        Err(Refusal::InvalidInput(m)) if m == never_held(item.id, pr) => {
            let take = UpdateInput {
                pr_ref: Some(pr.into()),
                ..UpdateInput::default()
            };
            ledger.work_update(actor, item.id, &take, None, None, false)??;
            ledger.work_merged(actor, &input)?
        }
        r => r,
    })
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// The lane's branch tip is read, else the worktree's HEAD; a lane with
    /// no worktree, one that is gone or a relative path has none; a
    /// directory that is no repository refuses.
    #[test]
    fn observed_head_reads_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().to_str().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(wt)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        assert_eq!(observed_head(None, None).unwrap(), None);
        let gone = dir.path().join("gone");
        assert_eq!(observed_head(gone.to_str(), None).unwrap(), None);
        assert_eq!(observed_head(Some("."), None).unwrap(), None);
        let Err(Refusal::InvalidInput(why)) = observed_head(Some(wt), None) else {
            panic!("a directory that is no repository was read");
        };
        assert!(why.starts_with("could not read the commit in "), "{why}");
        git(&["init", "-q", "-b", "layer-1"]);
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let one = git(&["rev-parse", "HEAD"]);
        assert_eq!(observed_head(Some(wt), None).unwrap(), Some(one.clone()));
        // A stack's next layer is checked out: the lower layer is still at
        // its own tip, and a branch the worktree lacks reads HEAD.
        git(&["checkout", "-q", "-b", "layer-2"]);
        git(&["commit", "-q", "--allow-empty", "-m", "two"]);
        let two = git(&["rev-parse", "HEAD"]);
        assert_ne!(one, two);
        assert_eq!(observed_head(Some(wt), Some("layer-1")).unwrap(), Some(one));
        assert_eq!(observed_head(Some(wt), Some("gone")).unwrap(), Some(two));
    }
}
