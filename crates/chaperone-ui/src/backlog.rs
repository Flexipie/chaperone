//! The backlog thread: a repo's open issues from daisy's GitHub mirror
//! (`daisy-cli github list`, the read-only exception to ADR 0003 agreed in
//! #109), less those in a lane and spec parents, each ready or blocked by
//! its "Blocked by" lines. It runs daisy-cli only while the window shows a
//! repo's Backlog, at most every [`MIRROR_EVERY`]; a ledger change only
//! matches the issues it holds against the lanes again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chaperone_core::child;
use chaperone_core::ledger::{Ledger, LedgerError};
use eframe::egui;
use serde_json::Value;

use crate::watch::Shown;

/// daisy-cli, at the same path `reviews record` uses; `CHAPERONE_DAISY_CLI`
/// overrides it.
const DAISY_CLI: &str = "/Applications/daisy.app/Contents/MacOS/daisy-cli";
const DAISY_TIMEOUT: Duration = Duration::from_secs(30);
/// One page of 500 issues is well under this; past it the read fails.
const PAGE_CAP: usize = 32 << 20;
/// The mirror syncs on its own clock, not the ledger's.
const MIRROR_EVERY: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(1);
/// Blockers looked up per read, this repo's first: a body is anyone's
/// text, and past this they stay blocked unread.
const GETS: usize = 50;
/// Blockers kept per issue; past this the issue is blocked by "more".
const PER_ISSUE: usize = 10;

/// One backlog row. Ready when `blocked_by` is empty.
pub struct Row {
    pub number: i64,
    pub title: String,
    /// Each blocker not yet cleared, as shown: `#110`, `owner/repo#3`, with
    /// "(not in mirror)" when daisy does not know it.
    pub blocked_by: Vec<String>,
}

/// A repo's backlog: issue number order until slice 9's priority.
pub struct Backlog {
    pub rows: Vec<Row>,
    /// Open issues the mirror has for the repo, rows or not.
    pub open: usize,
    /// The mirror's freshness when it is not `fresh`.
    pub stale: Option<String>,
}

/// The repo it was read for, as the board spells it, and the read.
pub type Read = (String, Result<Backlog, String>);

/// An open issue with only what the backlog needs from its body.
struct Issue {
    number: i64,
    title: String,
    /// Lowercased `owner/repo#N` keys from its "Blocked by" lines.
    blockers: Vec<String>,
}

struct Mirror {
    issues: Vec<Issue>,
    stale: Option<String>,
    /// Blockers not open in the repo that daisy has as closed, and those
    /// it does not have.
    closed: BTreeSet<String>,
    missing: BTreeSet<String>,
}

/// Starts the thread. `changed` runs after each change to `shown.backlog`.
pub fn spawn(path: PathBuf, shown: Arc<Mutex<Shown>>, changed: impl Fn() + Send + 'static) {
    thread::spawn(move || {
        let mut ledger = None;
        // The mirror read, for which repo and when, kept while the tab is
        // closed so reopening it within a minute runs nothing.
        let mut mirror: Option<(String, Instant, Result<Mirror, String>)> = None;
        let mut matched: Option<(String, i64)> = None;
        loop {
            let want = shown.lock().unwrap().backlog_for.clone();
            if let Some(slug) = want {
                let old = mirror.as_ref().is_none_or(|(s, at, _)| {
                    !s.eq_ignore_ascii_case(&slug) || at.elapsed() >= MIRROR_EVERY
                });
                if old {
                    mirror = Some((slug.clone(), Instant::now(), read_mirror(&slug)));
                    matched = None;
                }
                let (_, _, read) = mirror.as_ref().expect("read above");
                let got = match read {
                    // Shown once a read; tried again after MIRROR_EVERY.
                    Err(e) => old.then(|| Err(e.clone())),
                    Ok(m) => match rows(&path, &mut ledger, &slug, m, matched.as_ref()) {
                        Ok(None) => None,
                        Ok(Some((version, backlog))) => {
                            matched = Some((slug.clone(), version));
                            Some(Ok(backlog))
                        }
                        Err(e) => {
                            // Reopened next poll: the file may be migrated or created.
                            ledger = None;
                            matched = None;
                            Some(Err(e.to_string()))
                        }
                    },
                };
                if let Some(got) = got {
                    shown.lock().unwrap().backlog = Some((slug, got));
                    changed();
                }
            }
            thread::sleep(POLL);
        }
    });
}

/// The rows when the ledger changed since `matched` (or the repo did).
fn rows(
    path: &Path,
    ledger: &mut Option<Ledger>,
    slug: &str,
    mirror: &Mirror,
    matched: Option<&(String, i64)>,
) -> Result<Option<(i64, Backlog)>, LedgerError> {
    let ledger = match ledger {
        Some(l) => l,
        None => ledger.insert(Ledger::open_read_only(path)?),
    };
    let version = ledger.data_version()?;
    if matched.is_some_and(|(s, v)| s == slug && *v == version) {
        return Ok(None);
    }
    let lanes = ledger.issue_lanes()?;
    let parents: BTreeSet<String> = ledger
        .spec_list()?
        .into_iter()
        .map(|s| s.linear_key.to_lowercase())
        .collect();
    let repo = slug.to_lowercase();
    let here = format!("{repo}#");
    let mut rows = Vec::new();
    for issue in &mirror.issues {
        let key = format!("{repo}#{}", issue.number);
        if lanes.contains_key(&key) || parents.contains(&key) {
            continue;
        }
        // Cleared only on evidence: a done lane, or daisy has it closed.
        let blocked_by = issue
            .blockers
            .iter()
            .filter(|b| lanes.get(*b) != Some(&true))
            .filter(|b| !mirror.closed.contains(*b))
            .map(|b| {
                let name = match b.strip_prefix(&here) {
                    Some(n) => format!("#{n}"),
                    None => b.clone(),
                };
                match mirror.missing.contains(b) {
                    true => format!("{name} (not in mirror)"),
                    false => name.to_string(),
                }
            })
            .collect();
        rows.push(Row {
            number: issue.number,
            title: issue.title.clone(),
            blocked_by,
        });
    }
    rows.sort_by_key(|r| r.number);
    let backlog = Backlog {
        rows,
        open: mirror.issues.len(),
        stale: mirror.stale.clone(),
    };
    Ok(Some((version, backlog)))
}

/// Every open issue daisy mirrors for `slug`, then the state of each
/// blocker that is not one of them. Any failed or cut page fails the read.
fn read_mirror(slug: &str) -> Result<Mirror, String> {
    let repo = slug.to_lowercase();
    let mut issues = Vec::new();
    let mut stale = None;
    let mut cursor: Option<String> = None;
    loop {
        let mut args = vec!["github", "list", "--issues", "--state", "open"];
        args.extend(["--repo", slug, "--limit", "500"]);
        if let Some(c) = &cursor {
            args.extend(["--cursor", c]);
        }
        let env = daisy(&args)?;
        let status = &env["meta"]["source_freshness"]["github"]["status"];
        // No status proves nothing: not fresh.
        let status = status.as_str().unwrap_or("of unknown freshness");
        stale = stale.or((status != "fresh").then(|| status.to_string()));
        let data = &env["data"];
        for item in data["items"].as_array().into_iter().flatten() {
            let same = item["repo"]
                .as_str()
                .is_some_and(|r| r.eq_ignore_ascii_case(slug));
            let (Some(number), Some(title)) = (item["number"].as_i64(), item["title"].as_str())
            else {
                continue;
            };
            if same && item["kind"] == "issue" {
                let blockers = blocked_by(item["body"].as_str().unwrap_or(""), &repo);
                issues.push(Issue {
                    number,
                    title: title.into(),
                    blockers,
                });
            }
        }
        match (data["has_more"].as_bool(), data["next_cursor"].as_str()) {
            (Some(true), Some(next)) => cursor = Some(next.into()),
            (Some(true), None) => return Err("daisy-cli github list: no next_cursor".into()),
            _ => break,
        }
    }
    let open: BTreeSet<String> = issues
        .iter()
        .map(|i| format!("{repo}#{}", i.number))
        .collect();
    let (mut closed, mut missing) = (BTreeSet::new(), BTreeSet::new());
    let others: BTreeSet<(bool, &String)> = issues
        .iter()
        .flat_map(|i| &i.blockers)
        .filter(|b| !open.contains(*b))
        .map(|b| (!b.starts_with(&format!("{repo}#")), b))
        .collect();
    for (n, (_, key)) in others.into_iter().enumerate() {
        if n >= GETS {
            missing.insert(key.clone());
            continue;
        }
        // A body is anyone's text: only `owner/name#N` reaches daisy's argv.
        let shaped = key.split_once('/').is_some_and(|(owner, rest)| {
            let name = |s: &str| {
                !s.is_empty()
                    && !s.starts_with('-')
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            };
            rest.split_once('#')
                .is_some_and(|(repo, _)| name(owner) && name(repo))
        });
        if !shaped {
            missing.insert(key.clone());
            continue;
        }
        match daisy(&["github", "get", key]) {
            Ok(env) if env["data"]["item"]["state"] == "open" => {}
            Ok(env) if env["data"]["item"]["state"] == "closed" => {
                closed.insert(key.clone());
            }
            // daisy answers an item it has not synced with `invalid_input`;
            // an answer with no state proves nothing either.
            Ok(_) => {
                missing.insert(key.clone());
            }
            Err(e) if e.starts_with("not_found") || e.starts_with("invalid_input") => {
                missing.insert(key.clone());
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Mirror {
        issues,
        stale,
        closed,
        missing,
    })
}

/// The `owner/repo#N` keys, lowercased, on a body's "Blocked by" lines;
/// a bare `#N` is in `repo`.
fn blocked_by(body: &str, repo: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in body.lines() {
        let line = line.trim_start_matches(['-', '*', ' ', '\t']);
        let Some(rest) = line
            .get(..10)
            .filter(|h| h.eq_ignore_ascii_case("blocked by"))
            .map(|_| &line[10..])
        else {
            continue;
        };
        let words = rest.split(|c: char| c.is_whitespace() || c == ',' || c == ';');
        for word in words {
            let Some((owner_repo, n)) = word.rsplit_once('#') else {
                continue;
            };
            let n: String = n.chars().take_while(char::is_ascii_digit).collect();
            if n.is_empty() {
                continue;
            }
            let owner_repo = if owner_repo.is_empty() {
                repo
            } else {
                owner_repo
            };
            let key = format!("{owner_repo}#{n}").to_lowercase();
            if !keys.contains(&key) {
                if keys.len() == PER_ISSUE {
                    keys.push("more".into());
                    return keys;
                }
                keys.push(key);
            }
        }
    }
    keys
}

/// The envelope of a successful daisy-cli call; else `<code>: <message>`
/// (`runtime_unavailable` for daisy's exit 5, the app not running).
fn daisy(args: &[&str]) -> Result<Value, String> {
    let bin = std::env::var_os("CHAPERONE_DAISY_CLI").unwrap_or_else(|| DAISY_CLI.into());
    let call = format!("daisy-cli {}", args.join(" "));
    let deadline = Instant::now() + DAISY_TIMEOUT;
    let done =
        child::run(Command::new(&bin).args(args), deadline, PAGE_CAP).map_err(|e| match e {
            child::Failed::Spawn(e) => format!("internal: could not run {}: {e}", bin.display()),
            child::Failed::TimedOut => {
                format!(
                    "internal: {call}: no answer in {}s",
                    DAISY_TIMEOUT.as_secs()
                )
            }
            child::Failed::Wait(e) => format!("internal: {call}: {e}"),
        })?;
    if done.cut {
        return Err(format!("internal: {call}: answer past {PAGE_CAP} bytes"));
    }
    if done.status.success() {
        let env: Value = serde_json::from_slice(&done.out).unwrap_or(Value::Null);
        return match env["ok"] == true {
            true => Ok(env),
            false => Err(format!("internal: {call}: not a daisy-cli envelope")),
        };
    }
    if done.status.code() == Some(5) {
        return Err("runtime_unavailable: daisy is not running; open the daisy app".into());
    }
    let error = serde_json::from_slice::<Value>(&done.err).unwrap_or(Value::Null);
    let code = error["error"]["code"].as_str().unwrap_or("internal");
    let message = match error["error"]["message"].as_str() {
        Some(m) => m.to_string(),
        None => String::from_utf8_lossy(&done.err).trim().to_string(),
    };
    Err(format!("{code}: {call}: {message}"))
}

/// The Backlog tab of `slug`, as the board spells it.
pub fn draw(ui: &mut egui::Ui, read: Option<&Read>, slug: &str) {
    let Some((_, read)) = read.filter(|(s, _)| s == slug) else {
        ui.label("Reading daisy's GitHub mirror...");
        return;
    };
    let backlog = match read {
        Ok(b) => b,
        Err(e) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
    };
    let warn = ui.visuals().warn_fg_color;
    match (&backlog.stale, backlog.open) {
        (Some(status), 0) => {
            let line =
                format!("daisy's GitHub mirror is {status}: no open issues read for {slug}.");
            ui.colored_label(warn, line);
            return;
        }
        (None, 0) => {
            ui.label(format!(
                "daisy's GitHub mirror has no open issues for {slug}."
            ));
            return;
        }
        (Some(status), _) => {
            let line = format!("daisy's GitHub mirror is {status}: this may be out of date.");
            ui.colored_label(warn, line);
        }
        (None, _) => {}
    }
    if backlog.rows.is_empty() {
        let n = backlog.open;
        ui.label(format!(
            "None of the {n} open issues is waiting: each is in a lane or a spec parent."
        ));
        return;
    }
    let ready = backlog
        .rows
        .iter()
        .filter(|r| r.blocked_by.is_empty())
        .count();
    let waiting = backlog.rows.len();
    ui.label(format!(
        "{waiting} waiting to start, {ready} ready. Issue number order."
    ));
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new(("backlog", slug))
            .striped(true)
            .show(ui, |ui| {
                for c in ["issue", "title", "can start"] {
                    ui.strong(c);
                }
                ui.end_row();
                for row in &backlog.rows {
                    ui.label(format!("#{}", row.number));
                    ui.label(&row.title);
                    match row.blocked_by.is_empty() {
                        true => ui.label("ready"),
                        false => ui.label(format!("blocked by {}", row.blocked_by.join(", "))),
                    };
                    ui.end_row();
                }
            });
    });
}
