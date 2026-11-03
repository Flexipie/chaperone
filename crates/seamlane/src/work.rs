//! `seamlane work ...`: runs a parsed command against the ledger and
//! returns an `Outcome`; rendering is `output`'s.

use seamlane_core::cursor::Page;
use seamlane_core::ledger::{Ledger, Refusal};
use seamlane_core::work::{ClaimInput, CreateInput, UpdateInput, WorkFilter};
use serde::Serialize;
use serde_json::Value;

use crate::cli::{ActorArgs, WorkCmd};
use crate::output::{CliError, Meta, Outcome};

pub fn run(ledger: &Ledger, cmd: WorkCmd, meta: &mut Meta) -> Result<Outcome, CliError> {
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
            dry_run,
            actor,
        } => {
            let input = ClaimInput {
                worktree_path: worktree,
                branch,
                base_sha,
                lease_secs,
                takeover,
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
        WorkCmd::Get { id } => one(ledger.work_get(id, &ledger.now()?)??),
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

fn value(v: impl Serialize) -> Result<Value, CliError> {
    serde_json::to_value(v).map_err(|e| CliError::Internal(e.to_string()))
}

fn one(v: impl Serialize) -> Result<Outcome, CliError> {
    Ok(Outcome::One(value(v)?))
}
