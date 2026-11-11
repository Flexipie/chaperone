//! Reads token usage out of agent transcripts: R11's normalized fields per
//! provider message, plus the ids that place it. Counts, ids, model names
//! and a subagent's type and description leave this module; message text
//! never does.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

/// One provider message's usage. `fields` is the event's payload apart
/// from `source`.
pub(crate) struct Usage {
    pub message_id: String,
    pub source: &'static str,
    pub fields: Map<String, Value>,
}

/// One transcript the scan matched. `pending` messages are still being
/// written (no stop yet), so a later scan records them. `unknown` is a
/// transcript that carries no usage this scan can read.
#[derive(Clone, Debug, Serialize)]
pub struct Session {
    pub cli: &'static str,
    pub session_id: String,
    pub agent_id: Option<String>,
    pub messages: usize,
    pub pending: usize,
    pub unknown: bool,
}

#[derive(Default)]
pub(crate) struct Scan {
    pub usage: Vec<Usage>,
    pub sessions: Vec<Session>,
}

/// Claude Code sessions whose first `cwd` is `worktree` or a folder under
/// it (lane-stats' rule), each with its subagents. `config` is the Claude
/// config dir; a missing `projects` under it has no sessions.
pub(crate) fn claude(config: &Path, worktree: &str, scan: &mut Scan) -> io::Result<()> {
    let Some(projects) = dir(&config.join("projects"))? else {
        return Ok(());
    };
    let projects = projects.map(|p| p.map(|p| p.path()));
    claude_projects(projects.collect::<io::Result<_>>()?, worktree, scan)
}

/// [`claude`]'s sessions in these folders of `<config>/projects`.
pub(crate) fn claude_projects(
    projects: Vec<PathBuf>,
    worktree: &str,
    scan: &mut Scan,
) -> io::Result<()> {
    for project in projects {
        if !project.is_dir() {
            continue;
        }
        let Some(files) = dir(&project)? else {
            continue;
        };
        for file in files {
            let path = file?.path();
            let Some(session) = jsonl_stem(&path, "") else {
                continue;
            };
            let cwd = |d: &Value| str_at(d, "cwd").map(|c| under(c, worktree));
            if !first(&path, b"\"cwd\"", cwd)?.unwrap_or(false) {
                continue;
            }
            let base = json!({"cli": "claude", "session_id": session});
            claude_file(&path, "parent", base, scan)?;
            let subagents = project.join(&session).join("subagents");
            for sub in dir(&subagents)?.into_iter().flatten() {
                let sub = sub?.path();
                let Some(agent) = jsonl_stem(&sub, "agent-") else {
                    continue;
                };
                let meta = std::fs::read(sub.with_extension("meta.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .unwrap_or(Value::Null);
                let base = json!({
                    "cli": "claude",
                    "session_id": session,
                    "agent_id": agent,
                    "agent_type": meta.get("agentType"),
                    "description": meta.get("description"),
                });
                claude_file(&sub, "subagent", base, scan)?;
            }
        }
    }
    Ok(())
}

/// How long a transcript goes unwritten before its last message counts
/// without a stop.
const IDLE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// One Claude transcript. A response is written as one line per content
/// block, and the earlier lines can carry partial usage, so the last line
/// of each message id counts (lane-stats). A message is final once a line
/// of it has a `stop_reason`, a later message has begun, or the file has
/// been idle for [`IDLE`].
fn claude_file(path: &Path, source: &'static str, base: Value, scan: &mut Scan) -> io::Result<()> {
    #[derive(Default)]
    struct Msg {
        usage: Option<Value>,
        model: Option<Value>,
        stopped: bool,
    }
    let mut order: Vec<String> = Vec::new();
    let mut msgs: HashMap<String, Msg> = HashMap::new();
    lines(path, |line| {
        let Some(d) = parse(line, b"\"assistant\"") else {
            return true;
        };
        if d.get("type").and_then(Value::as_str) != Some("assistant") {
            return true;
        }
        let m = d.get("message").unwrap_or(&Value::Null);
        let Some(id) = str_at(m, "id").or_else(|| str_at(&d, "uuid")) else {
            return true;
        };
        let msg = msgs.entry(id.to_string()).or_insert_with(|| {
            order.push(id.to_string());
            Msg::default()
        });
        msg.usage = m.get("usage").filter(|u| u.is_object()).cloned();
        msg.model = m.get("model").cloned();
        msg.stopped |= m.get("stop_reason").is_some_and(|s| !s.is_null());
        true
    })?;
    // A transcript idle past IDLE is done, so its last message stays as it
    // is (an interrupt or a killed subagent never writes a stop).
    let idle = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|e| e > IDLE));
    let mut session = session("claude", &base);
    for (i, id) in order.iter().enumerate() {
        let msg = &msgs[id];
        if !msg.stopped && !idle && i + 1 == order.len() {
            session.pending += 1;
            continue;
        }
        let u = msg.usage.as_ref();
        let n = |k: &str| u.and_then(|u| u.get(k)).and_then(Value::as_u64);
        let mut fields = claude_tokens([
            n("input_tokens"),
            n("cache_read_input_tokens"),
            n("cache_creation_input_tokens"),
            n("output_tokens"),
        ]);
        fields["model"] = json!(msg.model);
        fields["raw"] = json!(u.map(counts));
        push(scan, &base, &mut session, id, source, fields);
    }
    scan.sessions.push(session);
    Ok(())
}

/// Codex rollouts whose `session_meta.cwd` is `worktree` or under it.
/// `home` is the Codex home; a missing `sessions` under it has none. Each
/// `token_usage_record` is one response; `input_tokens` includes the cached
/// count (local records: `total_tokens` is input plus output, and input is
/// never below cached), so R11's inclusive mapping applies.
pub(crate) fn codex(home: &Path, worktree: &str, scan: &mut Scan) -> io::Result<()> {
    let mut stack = vec![home.join("sessions")];
    while let Some(d) = stack.pop() {
        for entry in dir(&d)?.into_iter().flatten() {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let named = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
            if named {
                codex_file(&path, worktree, scan)?;
            }
        }
    }
    Ok(())
}

fn codex_file(path: &Path, worktree: &str, scan: &mut Scan) -> io::Result<()> {
    let meta = first(path, b"\"session_meta\"", |d| {
        (str_at(d, "type")? == "session_meta").then(|| {
            let p = d.get("payload")?;
            under(str_at(p, "cwd")?, worktree).then(|| p.clone())
        })
    })?
    .flatten();
    let Some(p) = meta else {
        return Ok(());
    };
    let Some(id) = str_at(&p, "id") else {
        return Ok(());
    };
    // A subagent's rollout names its parent thread; it is grouped under it,
    // as Claude's subagents are under their session.
    let spawned = p.get("source").and_then(|s| s.get("subagent"));
    let (source, base) = match spawned {
        Some(sub) => (
            "subagent",
            json!({
                "cli": "codex",
                "session_id": str_at(&p, "parent_thread_id").unwrap_or(id),
                "agent_id": id,
                "agent_type": sub.pointer("/thread_spawn/agent_role"),
                "description": p.get("agent_nickname"),
            }),
        ),
        None => ("parent", json!({"cli": "codex", "session_id": id})),
    };
    let mut model = Value::Null;
    let mut order: Vec<String> = Vec::new();
    let mut records: HashMap<String, (Value, Value)> = HashMap::new();
    lines(path, |line| {
        let Some(d) = parse(line, b"\"type\"") else {
            return true;
        };
        let p = d.get("payload").unwrap_or(&Value::Null);
        match d.get("type").and_then(Value::as_str) {
            Some("turn_context") => model = p.get("model").cloned().unwrap_or(Value::Null),
            Some("token_usage_record") => {
                if let (Some(rid), Some(u)) = (str_at(p, "response_id"), p.get("usage")) {
                    if !records.contains_key(rid) {
                        order.push(rid.to_string());
                    }
                    records.insert(rid.to_string(), (u.clone(), model.clone()));
                }
            }
            _ => {}
        }
        true
    })?;
    let mut session = session("codex", &base);
    session.unknown = order.is_empty();
    for rid in &order {
        let (u, model) = &records[rid];
        let mut fields = codex_tokens(u);
        fields["model"] = json!(model);
        fields["raw"] = counts(u);
        push(scan, &base, &mut session, rid, source, fields);
    }
    scan.sessions.push(session);
    Ok(())
}

/// R11's token fields of Anthropic's input, cache read, cache write and
/// output counts.
pub(crate) fn claude_tokens([input, read, write, output]: [Option<u64>; 4]) -> Value {
    let input_total = input.zip(read).zip(write).map(|((i, r), w)| i + r + w);
    // Anthropic counts thinking inside output_tokens and reports no
    // separate reasoning number.
    json!({
        "uncached_input": input,
        "cache_read": read,
        "cache_write": write,
        "output": output,
        "reasoning": null,
        "input_includes_cache": false,
        "output_includes_reasoning": true,
        "input_total": input_total,
        "output_total": output,
    })
}

/// R11's token fields of a Codex usage object.
pub(crate) fn codex_tokens(u: &Value) -> Value {
    let n = |k: &str| u.get(k).and_then(Value::as_u64);
    // Codex's input_tokens already holds cached_input_tokens.
    let uncached = n("input_tokens")
        .zip(n("cached_input_tokens"))
        .and_then(|(i, c)| i.checked_sub(c));
    json!({
        "uncached_input": uncached,
        "cache_read": n("cached_input_tokens"),
        "cache_write": n("cache_write_input_tokens"),
        "output": n("output_tokens"),
        "reasoning": n("reasoning_output_tokens"),
        "input_includes_cache": true,
        "output_includes_reasoning": true,
        "input_total": n("input_tokens"),
        "output_total": n("output_tokens"),
    })
}

fn session(cli: &'static str, base: &Value) -> Session {
    Session {
        cli,
        session_id: str_at(base, "session_id").unwrap_or_default().into(),
        agent_id: str_at(base, "agent_id").map(String::from),
        messages: 0,
        pending: 0,
        unknown: false,
    }
}

fn push(
    scan: &mut Scan,
    base: &Value,
    session: &mut Session,
    id: &str,
    source: &'static str,
    fields: Value,
) {
    let mut f = base.as_object().cloned().unwrap_or_default();
    f.insert("origin".into(), json!("transcript"));
    f.insert("message_id".into(), json!(id));
    f.insert("cost_usd".into(), Value::Null);
    f.extend(fields.as_object().cloned().unwrap_or_default());
    session.messages += 1;
    scan.usage.push(Usage {
        message_id: id.into(),
        source,
        fields: f,
    });
}

/// R13's allowlist for `raw`: the usage object's counts, and objects of
/// counts (Claude's `cache_creation`, `server_tool_use`). Strings are
/// dropped, so no text rides along.
pub(crate) fn counts(u: &Value) -> Value {
    match u {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, v)| v.is_number() || v.is_object())
                .map(|(k, v)| (k.clone(), counts(v)))
                .collect(),
        ),
        v if v.is_number() => v.clone(),
        _ => Value::Null,
    }
}

/// `cwd` is the worktree or a folder under it.
fn under(cwd: &str, worktree: &str) -> bool {
    cwd.strip_prefix(worktree)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// The entries of `d`, or None when it does not exist.
fn dir(d: &Path) -> io::Result<Option<std::fs::ReadDir>> {
    match std::fs::read_dir(d) {
        Ok(r) => Ok(Some(r)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(named(d, e)),
    }
}

/// `e` with the path it is about, so a failed report says which file.
fn named(path: &Path, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// The name of a `<prefix><name>.jsonl` file.
fn jsonl_stem(path: &Path, prefix: &str) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    Some(
        name.strip_suffix(".jsonl")?
            .strip_prefix(prefix)?
            .to_string(),
    )
}

/// A line as JSON when it holds `needle`; a malformed line is skipped, as
/// lane-stats does.
fn parse(line: &[u8], needle: &[u8]) -> Option<Value> {
    line.windows(needle.len())
        .any(|w| w == needle)
        .then(|| serde_json::from_slice(line).ok())
        .flatten()
}

/// The first `read` that answers, over the lines holding `needle`.
fn first<T>(
    path: &Path,
    needle: &[u8],
    read: impl Fn(&Value) -> Option<T>,
) -> io::Result<Option<T>> {
    let mut out = None;
    lines(path, |line| {
        out = parse(line, needle).and_then(|d| read(&d));
        out.is_none()
    })?;
    Ok(out)
}

/// Each complete line of `path` as bytes, until `each` says stop. A last
/// line without its newline is still being written and is left for the
/// next scan. A file gone since it was listed has no lines; any other
/// failure is the caller's.
fn lines(path: &Path, mut each: impl FnMut(&[u8]) -> bool) -> io::Result<()> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(named(path, e)),
    };
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| named(path, e))?;
        if read == 0 || buf.last() != Some(&b'\n') {
            return Ok(());
        }
        if !each(&buf) {
            return Ok(());
        }
    }
}
