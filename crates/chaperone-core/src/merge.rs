//! The owner's merge of a lane's PR (#117), the UI's and the CLI's: as
//! `/merge` does it, GitHub first, then `pr.merged`, then the done
//! handoff when no live lease holds the lane.

use std::ffi::OsStr;
use std::time::Duration;

use serde_json::Value;

use crate::checks::MergedInput;
use crate::child;
use crate::ledger::{Actor, Checked, Ledger, LedgerError, has_held};
use crate::pipeline::sha_prefix_eq;
use crate::work::HandoffInput;

/// How long one gh call may take.
const GH_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Merged {
    /// The merge commit.
    pub sha: String,
    pub done: Done,
}

/// How the lane comes to be done after its merge.
pub enum Done {
    /// It was done already.
    Was,
    /// Its lease was not live: the merge handed it off done.
    HandedOff,
    /// Its lease is live: only its agent hands off done.
    ByLane,
}

/// Squash-merges `pr` at `head`, the handoff head the owner confirmed,
/// once the ledger and GitHub both still hold it; a PR already merged at
/// that head is only recorded. `from` names the caller in the done
/// handoff. `Ok(Err(why))` is a refusal, before any merge.
pub fn merge(
    ledger: &Ledger,
    actor: &Actor,
    gh: &OsStr,
    work: i64,
    pr: &str,
    head: &str,
    from: &str,
) -> Result<Result<Merged, String>, String> {
    let read = |e: LedgerError| e.to_string();
    // The owner's, by `work ready`'s rule.
    let Some(session) = actor.session.as_deref() else {
        return Ok(Err("merge needs a session".into()));
    };
    if has_held(ledger.conn(), work, session).map_err(read)? {
        return Ok(Err(format!(
            "merge is the owner's; session {session} has held work item {work}"
        )));
    }
    let now = ledger.now().map_err(read)?;
    // What the confirm showed, read again now: a YES verdict on the
    // handoff head the owner confirmed, on the PR the lane holds.
    let page = ledger.lane_page(work, None).map_err(read)?;
    let gates = page.gates.as_ref();
    let passed = |id: &str| gates.is_some_and(|g| g.passed(id));
    let handed = gates.and_then(|g| g.head.as_deref()).unwrap_or_default();
    let item = done(ledger.work_get(work, &now))?;
    let board = ledger.work_board_item(work, &now).map_err(read)?;
    let not = |why: String| Ok(Err(why));
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
    let cli = |args: &[&str]| child::cli("gh", gh, args, GH_TIMEOUT, "");
    let view = || -> Result<Value, String> {
        let fields = "state,headRefOid,mergeCommit,mergedAt";
        let out = cli(&["pr", "view", n, "-R", repo, "--json", fields])?;
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
            let merge = cli(&[["pr", "merge", n, "-R", repo].as_slice(), &squash].concat());
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
        work,
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
    let ok = |done| {
        Ok(Ok(Merged {
            sha: sha.into(),
            done,
        }))
    };
    // Only the lane's agent hands off done while its lease is live; the
    // caller does once it is not.
    let now = ledger.now().map_err(read)?;
    let item = done(ledger.work_get(work, &now))?;
    if item.status == "done" {
        return ok(Done::Was);
    }
    if item.lease_state == "active" {
        return ok(Done::ByLane);
    }
    let newest = page.detail.handoffs.first().map(|h| h.handoff.clone());
    let input = HandoffInput {
        summary: format!("{merged} from {from}."),
        status: Some("done".into()),
        ready: Some(false),
        next_action: Some(format!("done: merged as {sha}")),
        commit: Some(handed.into()),
        ..newest.unwrap_or_default()
    };
    done(ledger.work_handoff(actor, work, &input, None, None, false))
        .map_err(|e| format!("{merged}; the done handoff failed: {e}"))?;
    ok(Done::HandedOff)
}

fn done<T>(c: Checked<T>) -> Result<T, String> {
    match c {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(r)) => Err(format!("refused: {r:?}")),
        Err(e) => Err(e.to_string()),
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}
