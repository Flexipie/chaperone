//! `chaperone hook <event>`: the plugin's Claude Code hooks for the lane
//! named by `CHAPERONE_WORK`, on a schema 2 pipeline (v20 on). Nudges
//! toward the binary's commands, never gates: anything else prints nothing.

use std::io::Read;
use std::path::Path;

use chaperone_core::checks::AttentionInput;
use chaperone_core::ledger::Ledger;
use chaperone_core::pipeline::sha_prefix_eq;
use chaperone_core::usage::Roots;
use chaperone_core::work::is_terminal;
use serde_json::{Value, json};

use crate::checks::{home, lane_pipeline};
use crate::cli::{ActorArgs, HookEvent};
use crate::work::observed_head;

pub fn run(event: HookEvent, db: &Path) -> ! {
    if let Some(out) = answer(event, db) {
        println!("{out}");
    }
    std::process::exit(0)
}

fn answer(event: HookEvent, db: &Path) -> Option<Value> {
    let work: i64 = std::env::var("CHAPERONE_WORK").ok()?.parse().ok()?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok()?;
    let input: Value = serde_json::from_str(&input).ok()?;
    let ledger = match event {
        HookEvent::Stop | HookEvent::StopFailure => Ledger::open(db),
        _ => Ledger::open_read_only(db),
    }
    .ok()?;
    let item = ledger.work_get(work, &ledger.now().ok()?).ok()?.ok()?;
    if is_terminal(&item.status) || lane_pipeline(&item).ok()?.schema < 2 {
        return None;
    }
    let head = || observed_head(item.worktree_path.as_deref(), item.branch.as_deref());
    match event {
        HookEvent::SessionStart => {
            let page = ledger.lane_page(work, None).ok()?;
            let gates = page.gates.iter().flat_map(|g| &g.gates);
            let failing: Vec<&str> = gates
                .filter(|g| !g.passed)
                .map(|g| g.gate.as_str())
                .collect();
            let at = head().ok().flatten();
            let title: String = item.title.chars().take(200).collect();
            let context = format!(
                "Chaperone lane: work {work}, \"{title}\", status {}, pipeline {}.\n\
                 Branch {}, head {}, PR {}.\n\
                 Ready gates not passing yet: {}.\n\
                 Commands: `chaperone check run plan --work {work} --file <plan>`, \
                 `chaperone check run smoke --work {work}`, `chaperone verify --work {work}`, \
                 `chaperone pr open --work {work}`. The owner merges.",
                item.status,
                item.pipeline_id.as_deref().unwrap_or("none"),
                item.branch.as_deref().unwrap_or("none"),
                at.as_deref().unwrap_or("none"),
                item.pr_ref.as_deref().unwrap_or("none"),
                match failing.is_empty() {
                    true => "none".into(),
                    false => failing.join(", "),
                },
            );
            Some(json!({"hookSpecificOutput": {
                "hookEventName": "SessionStart", "additionalContext": context}}))
        }
        HookEvent::PreBash => {
            let command = input["tool_input"]["command"].as_str()?;
            // Each command a separator starts, less its leading `VAR=value`
            // words; a command is denied by its first words only.
            let parts = command.split(['&', '|', ';', '\n', '(', ')', '`']);
            let parts: Vec<Vec<&str>> = parts
                .map(|p| {
                    p.split_whitespace()
                        .skip_while(|w| w.contains('='))
                        .collect()
                })
                .collect();
            let starts = |seq: &[&str]| parts.iter().any(|p| p.starts_with(seq));
            let ready = ["chaperone", "work", "ready"];
            let reason = if starts(&["gh", "pr", "create"]) {
                format!("Open the PR with `chaperone pr open --work {work}`.")
            } else if starts(&["gh", "pr", "merge"]) {
                "The owner merges, with `chaperone merge`.".to_string()
            } else if parts
                .iter()
                .any(|p| p.starts_with(&ready) && p.contains(&"--yes"))
            {
                format!(
                    "`work ready --yes` is the owner's override. Run `chaperone verify --work \
                     {work}` and hand off ready."
                )
            } else {
                return None;
            };
            Some(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse",
                "permissionDecision": "deny", "permissionDecisionReason": reason}}))
        }
        HookEvent::Stop => {
            // The lane's own session tokens, recorded every turn without
            // the lane asking; a report that finds nothing new writes nothing.
            let roots = Roots {
                claude: home("CLAUDE_CONFIG_DIR", ".claude"),
                codex: None,
            };
            if std::env::var_os("DAISY_CLI_READONLY").is_none_or(|v| v != "1") {
                let _ = ledger.usage_report(&ActorArgs::default().resolve(), work, &roots);
            }
            if input["stop_hook_active"] == true {
                return None;
            }
            let handoff = ledger.work_handoffs(work).ok()?.pop()?.handoff;
            let commit = handoff
                .commit
                .as_deref()
                .filter(|_| handoff.enters_ready())?;
            let head = head().ok()??;
            if sha_prefix_eq(commit, &head, 7..=64, false) {
                return None;
            }
            let reason = format!(
                "The branch moved to {head} after your ready handoff on {commit}. Run \
                 `chaperone verify --work {work}` and hand off again."
            );
            Some(json!({"decision": "block", "reason": reason}))
        }
        HookEvent::StopFailure => {
            let kind = input["error"].as_str()?;
            let record = AttentionInput {
                work,
                kind: "stopped".into(),
                by: "orchestrator".into(),
                note: Some(kind.into()),
                ..AttentionInput::default()
            };
            let _ = ledger.attention_record(&ActorArgs::default().resolve(), &record);
            None
        }
    }
}
