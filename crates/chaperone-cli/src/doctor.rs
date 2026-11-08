//! `chaperone doctor`: skill and script files against this binary, so a
//! call to a command, flag or value the CLI lacks, a handoff field the
//! ledger drops, or another pipeline version is caught before a lane runs
//! it. Everything is read from this binary (clap's parser, the core tables,
//! `pipeline::current`), never listed here. Reads files and git, never the
//! ledger.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::checks::CHECKS;
use chaperone_core::git::{Failure, git, git_bin};
use chaperone_core::ledger::STATUSES;
use chaperone_core::pipeline;
use chaperone_core::work::{Brief, HandoffInput, LookAt, Smoke, Verification};
use chaperone_graph::Error;
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory, Parser};
use serde_json::{Value, json};

use crate::cli::Cli;
use crate::output::{CliError, Outcome};

/// For every git call of one run, together.
const DEADLINE: Duration = Duration::from_secs(5);
const OUT_BYTES: usize = 64 * 1024 * 1024;
/// Stands in for a placeholder (`<id>`, `"$PR"`); a refusal of this value
/// is the placeholder's, not the file's, and is not reported.
const DUMMY: &str = "0";
/// Profile findings listed per kind; the rest are counted.
const LISTED: usize = 20;
/// What the binary builds from: a commit that differs from main only
/// elsewhere (docs) is not stale.
const CODE: [&str; 4] = ["crates", "pipelines", "Cargo.toml", "Cargo.lock"];
/// How a file names the binary before its argv. `chaperone ` alone also
/// counts, after a path or a space (see `markers`).
const MARKERS: [&str; 4] = ["\"$L\" ", "$L ", "\"$chaperone\" ", "$chaperone "];

#[derive(Clone, Copy)]
enum Lang {
    Markdown,
    Shell,
    Python,
}

struct Scan {
    findings: Vec<Value>,
    /// The engine's pipeline id, which every version a file names must be.
    current: &'static str,
    names_version: bool,
    reads_current: bool,
}

impl Scan {
    fn add(&mut self, file: &Path, line: Option<usize>, kind: &str, message: String) {
        self.findings
            .push(json!({ "file": file, "line": line, "kind": kind, "message": message }));
    }
}

pub fn run(
    dirs: &[PathBuf],
    repo: Option<&Path>,
    profile: Option<PathBuf>,
) -> Result<Outcome, CliError> {
    let current = pipeline::current();
    let mut scan = Scan {
        findings: Vec::new(),
        current: &current.id,
        names_version: false,
        reads_current: false,
    };
    let top: BTreeSet<String> = Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    let mut files = Vec::new();
    for d in dirs {
        let before = files.len();
        collect(d, &mut files, &mut scan);
        if files.len() == before {
            scan.add(
                d,
                None,
                "no_files",
                "no .md, .sh, .py or script here".into(),
            );
        }
    }
    let mut counted = Vec::new();
    let mut total = 0;
    for (path, lang) in &files {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                scan.add(path, None, "unreadable", e.to_string());
                continue;
            }
        };
        scan.reads_current |= text.contains("pipeline current");
        let n = match lang {
            Lang::Markdown => markdown(path, &text, &top, &mut scan),
            Lang::Shell => shell(path, 1, &text.lines().collect::<Vec<_>>(), &top, &mut scan),
            Lang::Python => python(path, &text, &top, &mut scan),
        };
        total += n;
        counted.push(json!({ "path": path, "invocations": n }));
    }
    if total == 0
        && let Some(d) = dirs.first()
    {
        // A wrong path, or a rename of the marker, would otherwise pass.
        scan.add(
            d,
            None,
            "no_invocations",
            "no chaperone call in any file".into(),
        );
    }
    // Files that name no version must read it, or nothing ties them.
    if !scan.names_version
        && !scan.reads_current
        && let Some(d) = dirs.first()
    {
        let msg = format!(
            "no file names pipeline {} or reads `chaperone pipeline current`",
            current.id
        );
        scan.add(d, None, "pipeline_version", msg);
    }
    let mut data = json!({
        "pipeline": { "id": current.id, "hash": current.hash },
        "build": env!("CHAPERONE_GIT_SHA"),
        "invocations": total,
        "files": counted,
    });
    if let Some(dir) = repo {
        data["origin_main"] = json!(repo_checks(dir, profile, &mut scan));
    }
    if scan.findings.is_empty() {
        Ok(Outcome::One(data))
    } else {
        Err(CliError::Drift(scan.findings))
    }
}

/// The files under `path` (or `path` itself) a skill or script is in,
/// sorted, hidden entries skipped.
fn collect(path: &Path, out: &mut Vec<(PathBuf, Lang)>, scan: &mut Scan) {
    if path.is_file() {
        if let Some(lang) = lang(path) {
            out.push((path.to_path_buf(), lang));
        }
        return;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => return scan.add(path, None, "unreadable", e.to_string()),
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if !p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            collect(&p, out, scan);
        }
    }
}

fn lang(path: &Path) -> Option<Lang> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md") => Some(Lang::Markdown),
        Some("sh") => Some(Lang::Shell),
        Some("py") => Some(Lang::Python),
        Some(_) => None,
        None => {
            // The shebang only: `~/.local/bin` also holds large binaries.
            let mut head = [0; 128];
            let n = std::io::Read::read(&mut std::fs::File::open(path).ok()?, &mut head).ok()?;
            let first = head[..n].split(|b| *b == b'\n').next()?;
            let first = std::str::from_utf8(first.strip_prefix(b"#!")?).ok()?;
            if first.contains("python") {
                Some(Lang::Python)
            } else if first.contains("sh") {
                Some(Lang::Shell)
            } else {
                None
            }
        }
    }
}

/// Fenced blocks as shell (a `json` one holding `changed_files` as a
/// handoff), and inline code spans, which may wrap within a paragraph;
/// prose is never read as a call.
fn markdown(path: &Path, text: &str, top: &BTreeSet<String>, scan: &mut Scan) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    let mut n = 0;
    // The paragraph so far, and the offset each of its lines starts at.
    let (mut para, mut starts) = (String::new(), Vec::new());
    for (i, l) in lines.iter().enumerate() {
        version_literals(path, i + 1, l, scan);
    }
    let mut i = 0;
    while i < lines.len() {
        let fence = lines[i].trim_start().strip_prefix("```");
        if fence.is_some() || lines[i].trim().is_empty() {
            n += spans(path, &para, &starts, top, scan);
            (para, starts) = (String::new(), Vec::new());
        }
        if let Some(info) = fence {
            let end = (i + 1..lines.len())
                .find(|&j| lines[j].trim_start().starts_with("```"))
                .unwrap_or(lines.len());
            let body = &lines[i + 1..end];
            if info.trim() == "json" {
                handoff(path, i + 2, body, scan);
            } else {
                n += shell(path, i + 2, body, top, scan);
            }
            i = end + 1;
            continue;
        }
        starts.push((para.len(), i + 1));
        para.push_str(lines[i]);
        para.push(' ');
        i += 1;
    }
    n + spans(path, &para, &starts, top, scan)
}

/// The calls in a paragraph's code spans, each at the line it starts on.
fn spans(
    path: &Path,
    para: &str,
    starts: &[(usize, usize)],
    top: &BTreeSet<String>,
    scan: &mut Scan,
) -> usize {
    let ticks: Vec<usize> = para.match_indices('`').map(|(i, _)| i).collect();
    let mut n = 0;
    for pair in ticks.chunks_exact(2) {
        let k = starts.partition_point(|(off, _)| *off <= pair[0]);
        let line = starts[k.saturating_sub(1)].1;
        n += calls(path, line, &para[pair[0] + 1..pair[1]], false, top, scan);
    }
    n
}

/// Shell lines from line `first`: continuations joined, comments skipped.
fn shell(
    path: &Path,
    first: usize,
    lines: &[&str],
    top: &BTreeSet<String>,
    scan: &mut Scan,
) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < lines.len() {
        let at = first + i;
        let mut joined = String::new();
        while i < lines.len() {
            let l = lines[i];
            i += 1;
            match l.trim_end().strip_suffix('\\') {
                Some(head) => joined.push_str(head),
                None => {
                    joined.push_str(l);
                    break;
                }
            }
            joined.push(' ');
        }
        if !joined.trim_start().starts_with('#') {
            n += calls(path, at, &joined, true, top, scan);
        }
    }
    n
}

/// Every call of the binary in `s`, each checked. `whole`: `s` holds whole
/// commands (a fenced block, a shell file), not fragments.
fn calls(
    path: &Path,
    line: usize,
    s: &str,
    whole: bool,
    top: &BTreeSet<String>,
    scan: &mut Scan,
) -> usize {
    let mut n = 0;
    for (at, bare) in markers(s) {
        let toks = tokens(&s[at..]);
        // The bare word is prose ("the chaperone ledger") unless a command
        // follows it.
        if toks.is_empty() || bare && !toks[0].as_ref().is_some_and(|w| top.contains(w)) {
            continue;
        }
        n += 1;
        check_argv(path, line, &toks, whole, scan);
    }
    n
}

/// The offsets just past each marker in `s`, and whether it is the bare
/// word `chaperone`.
fn markers(s: &str) -> Vec<(usize, bool)> {
    let mut at = Vec::new();
    for m in MARKERS {
        at.extend(s.match_indices(m).map(|(i, _)| (i + m.len(), false)));
    }
    // After a path, a space or a bracket, not inside `$chaperone` or a
    // longer word.
    for (i, _) in s.match_indices("chaperone ") {
        let before = s[..i].chars().next_back();
        if before.is_none_or(|c| matches!(c, ' ' | '/' | '(' | '\t')) {
            at.push((i + "chaperone ".len(), true));
        }
    }
    at.sort_unstable();
    at
}

/// Shell words up to the end of the command; `None` is a placeholder
/// (`<id>`, `"$PR"`, `$(…)`).
fn tokens(s: &str) -> Vec<Option<String>> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut started, mut ph) = (false, false);
    let flush =
        |out: &mut Vec<Option<String>>, cur: &mut String, started: &mut bool, ph: &mut bool| {
            if *started {
                let placeholder = *ph || cur.contains("...") || cur.contains('…');
                out.push((!placeholder).then(|| cur.clone()));
            }
            cur.clear();
            (*started, *ph) = (false, false);
        };
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        i += 1;
        match c {
            ' ' | '\t' => flush(&mut out, &mut cur, &mut started, &mut ph),
            '\'' | '"' => {
                started = true;
                while i < cs.len() && cs[i] != c {
                    ph |= c == '"' && matches!(cs[i], '$' | '`');
                    cur.push(cs[i]);
                    i += 1;
                }
                i += 1;
            }
            '$' | '<' => {
                let (open, close) = if c == '<' { ('<', '>') } else { ('(', ')') };
                if c == '<' || cs.get(i) == Some(&'(') {
                    let mut depth = usize::from(c == '<');
                    while i < cs.len() {
                        depth += usize::from(cs[i] == open);
                        depth -= usize::from(cs[i] == close);
                        i += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                (started, ph) = (true, true);
            }
            '|' | ';' | '&' | ')' | '`' | '>' => {
                // `2>&1`: the descriptor is not an argument.
                if c == '>' && cur.chars().all(|d| d.is_ascii_digit()) {
                    started = false;
                }
                break;
            }
            '[' | ']' | '(' | '#' if !started => break,
            '{' | '}' | '*' => (started, ph) = (true, true),
            _ => {
                started = true;
                cur.push(c);
            }
        }
    }
    flush(&mut out, &mut cur, &mut started, &mut ph);
    out
}

/// One call through clap's own parser, then the values core checks
/// itself (`--status`, `check record --reason`). Only a `whole` call can
/// lack a required flag or hold a bad number.
fn check_argv(path: &Path, line: usize, toks: &[Option<String>], whole: bool, scan: &mut Scan) {
    let argv =
        std::iter::once("chaperone").chain(toks.iter().map(|t| t.as_deref().unwrap_or(DUMMY)));
    if let Err(e) = Cli::try_parse_from(argv) {
        let kind = match e.kind() {
            ErrorKind::InvalidSubcommand => Some(("command", ContextKind::InvalidSubcommand)),
            ErrorKind::UnknownArgument => Some(("flag", ContextKind::InvalidArg)),
            ErrorKind::InvalidValue => Some(("value", ContextKind::InvalidValue)),
            ErrorKind::MissingRequiredArgument if whole => Some(("flag", ContextKind::InvalidArg)),
            ErrorKind::ValueValidation if whole => Some(("value", ContextKind::InvalidValue)),
            // A fragment (`$L work get` without its id), or a span or a
            // python list that may continue elsewhere: not drift.
            _ => None,
        };
        // A placeholder's refusal, or a flag whose value the fragment cut.
        if let Some((kind, ctx)) = kind
            && !matches!(e.get(ctx), Some(ContextValue::String(v)) if v == DUMMY || v.is_empty())
        {
            // The first paragraph: a missing flag's name is on its second line.
            let msg = e.to_string();
            let first: Vec<&str> = msg
                .lines()
                .take_while(|l| !l.is_empty())
                .map(str::trim)
                .collect();
            let first = first.join(" ");
            scan.add(
                path,
                Some(line),
                kind,
                first.trim_start_matches("error: ").into(),
            );
        }
    }
    let words: Vec<&str> = toks.iter().map(|t| t.as_deref().unwrap_or("")).collect();
    let flag = |name: &str| {
        words
            .iter()
            .position(|w| *w == name)
            .and_then(|i| words.get(i + 1))
            .filter(|v| !v.is_empty())
    };
    match words.get(..2) {
        Some(["work", "update" | "list"]) => {
            if let Some(s) = flag("--status")
                && !STATUSES.contains(s)
            {
                let msg = format!("--status {s}: not one of {}", STATUSES.join(", "));
                scan.add(path, Some(line), "value", msg);
            }
        }
        Some(["check", "record"]) => {
            if let (Some(check), Some(reason)) = (flag("--check"), flag("--reason"))
                && let Some((_, reasons)) = CHECKS.iter().find(|(c, _)| c == check)
                && !reasons.contains(reason)
            {
                let msg = format!(
                    "--reason {reason}: {check} skips only for {}",
                    reasons.join(", ")
                );
                scan.add(path, Some(line), "value", msg);
            }
        }
        _ => {}
    }
}

/// Python calls: on a line that names the ledger, a run of string
/// literals whose first is a chaperone command (`[ledger, "work", "get",
/// id]`, `cli(ledger, ["work", …])`, `daisy("work", "list", cli=ledger)`).
fn python(path: &Path, text: &str, top: &BTreeSet<String>, scan: &mut Scan) -> usize {
    let mut n = 0;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with('#')
            || !(line.contains("ledger") || line.contains("chaperone"))
        {
            continue;
        }
        let cs: Vec<char> = line.chars().collect();
        let mut at = 0;
        while at < cs.len() {
            let Some((first, end)) = literal(&cs, at) else {
                at += 1;
                continue;
            };
            let (toks, stop) = elements(&cs, at);
            at = stop.max(end);
            if first
                .as_deref()
                .is_some_and(|f| top.contains(f) && f != "help")
                && toks.get(1).is_some_and(Option::is_some)
            {
                n += 1;
                check_argv(path, i + 1, &toks, false, scan);
            }
        }
    }
    n
}

/// The string literal at `at`: its value (`None` for an f-string or one
/// with a `{`) and the offset past it.
fn literal(cs: &[char], at: usize) -> Option<(Option<String>, usize)> {
    let (f, q) = match cs.get(at)? {
        'f' => (true, *cs.get(at + 1)?),
        c => (false, *c),
    };
    if !matches!(q, '"' | '\'') {
        return None;
    }
    let start = at + usize::from(f) + 1;
    let len = cs[start..].iter().position(|c| *c == q)?;
    let s: String = cs[start..start + len].iter().collect();
    Some(((!f && !s.contains('{')).then_some(s), start + len + 1))
}

/// Comma-separated elements from `at` to the closing bracket or a
/// keyword argument; anything not a literal is a placeholder.
fn elements(cs: &[char], mut at: usize) -> (Vec<Option<String>>, usize) {
    let mut out = Vec::new();
    loop {
        while cs.get(at) == Some(&' ') {
            at += 1;
        }
        if let Some((v, end)) = literal(cs, at) {
            out.push(v);
            at = end;
        } else {
            let start = at;
            let mut depth = 0i32;
            while let Some(&c) = cs.get(at) {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' if depth == 0 => break,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => break,
                    '=' if depth == 0 && cs.get(at + 1) != Some(&'=') => return (out, at),
                    _ => {}
                }
                at += 1;
            }
            if at == start {
                return (out, at);
            }
            out.push(None);
        }
        while cs.get(at) == Some(&' ') {
            at += 1;
        }
        if cs.get(at) != Some(&',') {
            return (out, at);
        }
        at += 1;
    }
}

/// Each `pipeline vX` / `Pipeline version: **vX**` on a line must be
/// the engine's.
fn version_literals(path: &Path, line: usize, text: &str, scan: &mut Scan) {
    let lower = text.to_lowercase();
    for (i, _) in lower.match_indices("pipeline ") {
        let rest = lower[i + "pipeline ".len()..].trim_start_matches("version:");
        let rest = rest.trim_start_matches([' ', '*']);
        let Some(v) = rest.strip_prefix('v') else {
            continue;
        };
        let id: String = v
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let id = id.trim_end_matches('.');
        if !id.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        scan.names_version = true;
        if scan.current.strip_prefix('v') != Some(id) {
            let msg = format!("names pipeline v{id}; the engine runs {}", scan.current);
            scan.add(path, Some(line), "pipeline_version", msg);
        }
    }
}

/// A JSON block with `changed_files` is a handoff: each key must be one
/// the ledger keeps (it drops the rest silently).
fn handoff(path: &Path, first: usize, body: &[&str], scan: &mut Scan) {
    let text = body.join("\n");
    if !text.contains("\"changed_files\"") {
        return;
    }
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            let at = first + e.line().saturating_sub(1);
            return scan.add(path, Some(at), "handoff_field", format!("not JSON: {e}"));
        }
    };
    let known = keys(&json!(full_handoff()));
    for key in keys(&v).difference(&known) {
        let quoted = format!("\"{key}\"");
        let at = body.iter().position(|l| l.contains(&quoted)).unwrap_or(0);
        let msg = format!("`{key}` is not a handoff field; the ledger drops it");
        scan.add(path, Some(first + at), "handoff_field", msg);
    }
}

fn keys(v: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                out.insert(k.clone());
                out.extend(keys(v));
            }
        }
        Value::Array(a) => a.iter().for_each(|v| out.extend(keys(v))),
        _ => {}
    }
    out
}

/// Every field set and none defaulted, so a new field breaks the build
/// here rather than reading as unknown.
fn full_handoff() -> HandoffInput {
    let s = || "x".to_string();
    HandoffInput {
        summary: s(),
        status: Some(s()),
        changed_files: vec![s()],
        areas: Some(vec![s()]),
        verification: vec![Verification {
            command: s(),
            status: s(),
            note: Some(s()),
        }],
        smoke: vec![Smoke {
            what: s(),
            shows: s(),
            evidence: Some(s()),
        }],
        blockers: vec![s()],
        open_questions: vec![s()],
        next_action: Some(s()),
        commit: Some(s()),
        pull_request: Some(s()),
        ready: Some(true),
        brief: Some(Brief {
            goal: s(),
            merge_case: s(),
            decisions: vec![s()],
            look_at: vec![LookAt {
                path: s(),
                line: Some(1),
                why: s(),
            }],
            check_by_hand: vec![s()],
        }),
    }
}

/// The profile's areas against the tracked files, and this binary's code
/// against origin/main. Returns origin/main's sha when git has one.
fn repo_checks(dir: &Path, profile: Option<PathBuf>, scan: &mut Scan) -> Option<String> {
    let deadline = Instant::now() + DEADLINE;
    let bin = git_bin(deadline);
    let call = |args: &[&str]| match git(&bin, dir, args, deadline, OUT_BYTES) {
        Ok((out, false)) => Ok(String::from_utf8_lossy(&out).into_owned()),
        Ok((_, true)) => Err(format!("git {}: output past {OUT_BYTES} bytes", args[0])),
        Err(Failure::TimedOut) => Err(format!("git {}: timed out", args[0])),
        Err(Failure::BadRevision) => Err(format!("git {}: a commit this clone lacks", args[0])),
        Err(Failure::Other(e)) => Err(e),
    };
    let origin = call(&["config", "--get", "remote.origin.url"]).ok();
    match chaperone_graph::rules(profile, origin.as_deref().map(str::trim)) {
        Err(Error::Invalid(e) | Error::Internal(e)) => scan.add(dir, None, "profile", e),
        Ok(rules) => match call(&["ls-files", "-z", "--full-name", "--", ":/"]) {
            Err(e) => scan.add(dir, None, "profile", e),
            Ok(out) => {
                let tree: Vec<&str> = out.split('\0').filter(|p| !p.is_empty()).collect();
                let (mut none, mut several) = (Vec::new(), Vec::new());
                for p in &tree {
                    let areas: Vec<&str> = rules.areas(p).collect();
                    match areas.len() {
                        0 => none.push(format!("{p}: in no area")),
                        1 => {}
                        _ => several.push(format!("{p}: in areas {}", areas.join(", "))),
                    }
                }
                for list in [none, several] {
                    let more = list.len().saturating_sub(LISTED);
                    for m in list.into_iter().take(LISTED) {
                        scan.add(dir, None, "area", m);
                    }
                    if more > 0 {
                        scan.add(dir, None, "area", format!("{more} more like the above"));
                    }
                }
                // Its refusal (an entry point in no or several areas) is
                // the per-file list above; only the unmatched rules are new.
                if let Ok(unmatched) = rules.check(tree.into_iter()) {
                    for u in unmatched {
                        let msg = format!("{} rule {} names no tracked file", u.kind, u.path);
                        scan.add(dir, None, "entry_point", msg);
                    }
                }
            }
        },
    }
    let sha = env!("CHAPERONE_GIT_SHA");
    let main = match call(&["rev-parse", "--verify", "origin/main^{commit}"]) {
        Ok(m) => m.trim().to_string(),
        Err(e) => {
            scan.add(
                dir,
                None,
                "binary",
                format!("no origin/main to compare with: {e}"),
            );
            return None;
        }
    };
    if sha.is_empty() {
        let msg = "built outside a git checkout: its commit is unknown".into();
        scan.add(dir, None, "binary", msg);
        return Some(main);
    }
    let mut args = vec!["diff", "--name-only", sha, &main, "--"];
    args.extend(CODE);
    match call(&args) {
        Ok(out) if out.trim().is_empty() => {}
        Ok(out) => {
            let msg = format!(
                "built from {}, its code differs from origin/main {} in {} files: reinstall",
                &sha[..7],
                &main[..7],
                out.lines().count()
            );
            scan.add(dir, None, "binary", msg);
        }
        Err(e) => scan.add(dir, None, "binary", format!("built from {sha}: {e}")),
    }
    Some(main)
}
