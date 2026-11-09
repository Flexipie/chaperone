//! The Pipeline page: each pipeline version with its changelog entry and
//! its lanes' numbers (`pipeline_history`), the installed `chaperone
//! doctor`'s drift results, and the skills and scripts with the version
//! that last changed each and any uncommitted edit. Its thread works only
//! while the page is shown: the history at most every [`HISTORY_EVERY`],
//! the skills every [`SKILLS_EVERY`], doctor every [`DOCTOR_EVERY`].

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chaperone_core::child;
use chaperone_core::git::{Failure, git, git_bin};
use chaperone_core::ledger::{Ledger, LedgerError, Refusal};
use chaperone_core::stats::{Version, changelog, version_at};
use eframe::egui::{self, RichText};
use serde_json::Value;

use crate::board;
use crate::theme;
use crate::watch::Shown;

/// The changelog is read again each time, so a new version shows with no
/// ledger write; the history only when it or the ledger changed.
const HISTORY_EVERY: Duration = Duration::from_secs(5);
const SKILLS_EVERY: Duration = Duration::from_secs(60);
/// doctor parses every skill file; drift comes from edits, not lanes.
const DOCTOR_EVERY: Duration = Duration::from_secs(300);
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const OUT_CAP: usize = 16 << 20;
const POLL: Duration = Duration::from_secs(1);
/// Where the skills and scripts live under the dotfiles root.
const DIRS: [&str; 2] = ["claude/skills", "bin"];

/// What the page draws, each part with when it was read (Unix seconds).
#[derive(Default)]
pub struct Read {
    pub history: Option<Result<Vec<Value>, String>>,
    pub drift: Option<(i64, Result<Drift, String>)>,
    pub skills: Option<(i64, Result<Vec<Entry>, String>)>,
}

/// doctor's answer: no findings, or each as `file:line kind: message`.
pub struct Drift {
    /// The binary checked, as run.
    pub bin: String,
    pub invocations: Option<i64>,
    pub findings: Vec<[String; 3]>,
}

/// A skill or script, by its path under the dotfiles root.
pub struct Entry {
    pub path: String,
    /// Its newest commit's date (UTC) and the changelog version then.
    pub changed: Option<(String, String)>,
    pub edited: bool,
}

/// Starts the thread. `changed` runs after each change to `shown.pipeline`.
pub fn spawn(
    path: PathBuf,
    root: PathBuf,
    shown: Arc<Mutex<Shown>>,
    changed: impl Fn() + Send + 'static,
) {
    thread::spawn(move || {
        let mut ledger: Option<Ledger> = None;
        let mut read: Option<(Vec<Version>, i64)> = None;
        let mut versions = Vec::new();
        let (mut history_at, mut skills_at, mut doctor_at) = (None, None, None);
        let due = |at: Option<Instant>, every| at.is_none_or(|t: Instant| t.elapsed() >= every);
        loop {
            if !shown.lock().unwrap().pipeline_open {
                thread::sleep(POLL);
                continue;
            }
            // The numbers first: doctor may take its whole deadline.
            if due(history_at, HISTORY_EVERY) {
                history_at = Some(Instant::now());
                let got = match changelog(&root) {
                    Ok(v) => {
                        versions = v.clone();
                        history(&path, &mut ledger, v, &mut read)
                    }
                    Err(r) => {
                        read = None;
                        Some(Err(refusal(&r)))
                    }
                };
                if let Some(got) = got {
                    shown.lock().unwrap().pipeline.history = Some(got);
                    changed();
                }
            }
            if due(skills_at, SKILLS_EVERY) {
                skills_at = Some(Instant::now());
                let got = skills(&root, &versions);
                shown.lock().unwrap().pipeline.skills = Some((board::now(), got));
                changed();
            }
            if due(doctor_at, DOCTOR_EVERY) {
                let got = doctor(&root);
                doctor_at = Some(Instant::now());
                shown.lock().unwrap().pipeline.drift = Some((board::now(), got));
                changed();
            }
            thread::sleep(POLL);
        }
    });
}

/// The history's rows when the changelog or the ledger changed since
/// `read`; the version is taken before the read, so a write during it
/// reads again next time.
fn history(
    path: &Path,
    ledger: &mut Option<Ledger>,
    versions: Vec<Version>,
    read: &mut Option<(Vec<Version>, i64)>,
) -> Option<Result<Vec<Value>, String>> {
    let got = (|| -> Result<_, LedgerError> {
        let l = match ledger {
            Some(l) => l,
            None => ledger.insert(Ledger::open_read_only(path)?),
        };
        let version = l.data_version()?;
        if read.as_ref() == Some(&(versions.clone(), version)) {
            return Ok(None);
        }
        let out = l.pipeline_history(&versions, None)?;
        *read = Some((versions, version));
        Ok(Some(out))
    })();
    match got {
        Ok(None) => None,
        Ok(Some(Ok(v))) => Some(Ok(v["versions"].as_array().cloned().unwrap_or_default())),
        Ok(Some(Err(r))) => Some(Err(refusal(&r))),
        Err(e) => {
            // Reopened next time: the file may be migrated or created.
            *ledger = None;
            *read = None;
            Some(Err(e.to_string()))
        }
    }
}

/// A refusal as the owner reads it: its message, not its debug form.
fn refusal(r: &Refusal) -> String {
    match r {
        Refusal::InvalidInput(m) => m.clone(),
        r => format!("{r:?}"),
    }
}

/// The chaperone the skills call: `CHAPERONE_BIN`, else the first on
/// PATH, as `$L` finds it, by its full path so the card can name it.
fn chaperone() -> PathBuf {
    if let Some(bin) = std::env::var_os("CHAPERONE_BIN") {
        return bin.into();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join("chaperone"))
        .find(|p| p.is_file())
        .unwrap_or("chaperone".into())
}

/// `chaperone doctor --skills` over the skills and scripts: its exit 0,
/// its `drift` findings, or why it did not answer.
fn doctor(root: &Path) -> Result<Drift, String> {
    let bin = chaperone();
    let mut cmd = Command::new(&bin);
    cmd.arg("doctor").arg("--skills");
    cmd.args(DIRS.map(|d| root.join(d)));
    let deadline = Instant::now() + DOCTOR_TIMEOUT;
    let done = child::run(&mut cmd, deadline, OUT_CAP).map_err(|e| match e {
        child::Failed::Spawn(e) => format!("could not run {}: {e}", bin.display()),
        child::Failed::TimedOut => format!(
            "{} doctor: no answer in {}s",
            bin.display(),
            DOCTOR_TIMEOUT.as_secs()
        ),
        child::Failed::Wait(e) => format!("{} doctor: {e}", bin.display()),
    })?;
    if done.status.success() {
        let env: Value = serde_json::from_slice(&done.out).unwrap_or(Value::Null);
        if env["ok"] != true || done.cut {
            return Err(format!(
                "{} doctor: not a chaperone envelope",
                bin.display()
            ));
        }
        let invocations = env["data"]["invocations"].as_i64();
        return Ok(Drift {
            bin: bin.display().to_string(),
            invocations,
            findings: Vec::new(),
        });
    }
    let error = serde_json::from_slice::<Value>(&done.err).unwrap_or(Value::Null);
    let error = &error["error"];
    if error["code"] == "drift"
        && let Some(found) = error["details"]["findings"].as_array()
    {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let findings = found
            .iter()
            .map(|f| {
                // Under the root, as the skills list names them.
                let file = text(&f["file"]);
                let file = Path::new(&file)
                    .strip_prefix(root)
                    .map_or(file.clone(), |p| p.display().to_string());
                let at = match f["line"].as_u64() {
                    Some(n) => format!("{file}:{n}"),
                    None => file,
                };
                [at, text(&f["kind"]), text(&f["message"])]
            })
            .collect();
        return Ok(Drift {
            bin: bin.display().to_string(),
            invocations: None,
            findings,
        });
    }
    let message = match error["message"].as_str() {
        Some(m) => format!("{}: {m}", error["code"].as_str().unwrap_or("error")),
        None => String::from_utf8_lossy(&done.err).trim().to_string(),
    };
    Err(format!("{} doctor: {message}", bin.display()))
}

/// Every skill and script (each entry of [`DIRS`] but hidden ones and
/// `__pycache__`), its newest commit and whether it has uncommitted edits.
fn skills(root: &Path, versions: &[Version]) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for dir in DIRS {
        let read = std::fs::read_dir(root.join(dir)).map_err(|e| format!("{dir}: {e}"))?;
        let mut names: Vec<String> = read
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.') && n != "__pycache__")
            .collect();
        names.sort();
        entries.extend(names.into_iter().map(|n| Entry {
            path: format!("{dir}/{n}"),
            changed: None,
            edited: false,
        }));
    }
    let deadline = Instant::now() + GIT_TIMEOUT;
    let bin = git_bin(deadline);
    let run = |args: &[&str]| -> Result<String, String> {
        // Paths as written, not quoted, so a non-ASCII name still matches.
        let all = [&["-c", "core.quotePath=false"], args].concat();
        match git(&bin, root, &all, deadline, OUT_CAP) {
            Ok((_, true)) => Err(format!("git {}: past {OUT_CAP} bytes", args[0])),
            Ok((out, false)) => Ok(String::from_utf8_lossy(&out).into_owned()),
            Err(Failure::TimedOut) => Err(format!("git {}: timed out", args[0])),
            Err(Failure::BadRevision) => Err(format!("git {}: no commits", args[0])),
            Err(Failure::Other(e)) => Err(e),
        }
    };
    // git prints paths from its root: anything else would match nothing.
    let top = run(&["rev-parse", "--show-toplevel"])?;
    let real = root.canonicalize().map_err(|e| e.to_string())?;
    if Path::new(top.trim()) != real {
        return Err(format!("{} is not a git checkout's root", root.display()));
    }
    let find = |entries: &[Entry], path: &str| -> Option<usize> {
        entries.iter().position(|e| {
            path == e.path
                || path
                    .strip_prefix(&e.path)
                    .is_some_and(|r| r.starts_with('/'))
        })
    };
    let mut args = vec!["log", "--format=%x00%ct", "--name-only", "--"];
    args.extend(DIRS);
    for commit in run(&args)?.split('\0').skip(1) {
        let mut lines = commit.lines();
        let Some(secs) = lines.next().and_then(|s| s.trim().parse().ok()) else {
            continue;
        };
        for file in lines.filter(|l| !l.is_empty()) {
            if let Some(i) = find(&entries, file)
                && entries[i].changed.is_none()
            {
                let at = utc(secs);
                // No changelog read: no version to name.
                let v = match versions.is_empty() {
                    true => "-",
                    false => version_at(versions, &at),
                };
                let v = v.to_string();
                entries[i].changed = Some((at[..10].to_string(), v));
            }
        }
    }
    let mut args = vec![
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
    ];
    args.extend(DIRS);
    // `XY path`, a rename's old path as its own field; either marks it.
    for field in run(&args)?.split('\0') {
        let path = field
            .get(3..)
            .filter(|_| field.as_bytes().get(2) == Some(&b' '));
        if let Some(i) = find(&entries, path.unwrap_or(field)) {
            entries[i].edited = true;
        }
    }
    Ok(entries)
}

/// `secs` as RFC 3339 UTC with milliseconds, as [`Version::at`].
fn utc(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

pub fn draw(ui: &mut egui::Ui, read: &Read, now: i64) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Pipeline");
        match &read.history {
            None => {
                ui.label("Reading the pipeline history...");
            }
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            Some(Ok(rows)) => versions(ui, rows),
        }
        ui.add_space(8.0);
        card(ui, "Drift check", read.drift.as_ref(), now, |ui, d| {
            drift(ui, d)
        });
        card(
            ui,
            "Skills and scripts",
            read.skills.as_ref(),
            now,
            |ui, e| entries(ui, e),
        );
    });
}

/// One fold per version, the live one (the newest in the changelog) open.
fn versions(ui: &mut egui::Ui, rows: &[Value]) {
    let live = rows.iter().find(|r| r["started_at"].is_string());
    for row in rows {
        let id = row["version"].as_str().unwrap_or("?");
        let engine: Vec<&str> = row["engine_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let mut title = id.to_string();
        if !engine.is_empty() {
            title += &format!(" · engine {}", engine.join(", "));
        }
        let is_live = live.is_some_and(|l| std::ptr::eq(l, row));
        if is_live {
            title += " · live";
        }
        let since = row["started_at"].as_str().map(|s| &s[..s.len().min(10)]);
        theme::glass().show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::CollapsingHeader::new(RichText::new(title).strong())
                .id_salt(("version", id))
                .default_open(is_live)
                .show(ui, |ui| {
                    ui.label(numbers(row));
                    if let Some(since) = since {
                        ui.label(RichText::new(format!("since {since}")).small().weak());
                    }
                    match row["entry"].as_str().filter(|e| !e.is_empty()) {
                        Some(entry) => ui.label(entry),
                        None => ui.weak("Not in the changelog."),
                    };
                });
        });
    }
}

fn numbers(row: &Value) -> String {
    let c = &row["cohort"];
    let n = |v: &Value| v.as_i64().unwrap_or(0);
    let median = |v: &Value, unit: &str| v.as_f64().map_or("-".into(), |m| format!("{m}{unit}"));
    let mut line = format!(
        "{} lanes, {} merged · median claim to merge {} · {} send-backs · review rounds median {} over {} with local rounds",
        n(&c["lanes"]),
        n(&c["merged"]),
        median(&c["claim_to_merge_h_median"], " h"),
        n(&c["attention"]["sent_back"]),
        median(&row["review_rounds_median"], ""),
        n(&row["lanes_review_rounds"]),
    );
    if c["too_few_lanes"] == true {
        line += " (too few lanes to compare)";
    }
    line
}

/// A glass card with when its read ran, or that it has not yet.
fn card<T>(
    ui: &mut egui::Ui,
    title: &str,
    read: Option<&(i64, Result<T, String>)>,
    now: i64,
    body: impl FnOnce(&mut egui::Ui, &T),
) {
    theme::glass().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.strong(title);
            if let Some((at, _)) = read {
                let ago = format!("checked {} ago", board::age(*at, now));
                ui.label(RichText::new(ago).small().weak());
            }
        });
        match read {
            None => {
                ui.label("Reading...");
            }
            Some((_, Err(e))) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            Some((_, Ok(t))) => body(ui, t),
        }
    });
}

fn drift(ui: &mut egui::Ui, d: &Drift) {
    if d.findings.is_empty() {
        let calls = d.invocations.unwrap_or(0);
        ui.label(format!(
            "No drift: {calls} chaperone calls match {}.",
            d.bin
        ));
        return;
    }
    let n = d.findings.len();
    ui.label(theme::alert(&format!(
        "{n} mismatches between the skills and {}",
        d.bin
    )));
    egui::Grid::new("drift").striped(true).show(ui, |ui| {
        for c in ["where", "kind", "what"] {
            ui.strong(c);
        }
        ui.end_row();
        for row in &d.findings {
            for cell in row {
                ui.label(cell);
            }
            ui.end_row();
        }
    });
}

fn entries(ui: &mut egui::Ui, entries: &[Entry]) {
    let edited = entries.iter().filter(|e| e.edited).count();
    if edited > 0 {
        ui.label(theme::alert(&format!("{edited} with uncommitted edits")));
    }
    egui::Grid::new("skills").striped(true).show(ui, |ui| {
        for c in ["path", "last changed", "in version", "uncommitted"] {
            ui.strong(c);
        }
        ui.end_row();
        for e in entries {
            ui.label(&e.path);
            let (date, version) = match &e.changed {
                Some((d, v)) => (d.as_str(), v.as_str()),
                None => ("never committed", "-"),
            };
            ui.label(date);
            ui.label(version);
            match e.edited {
                true => ui.label(theme::alert("edited")),
                false => ui.label("-"),
            };
            ui.end_row();
        }
    });
}
