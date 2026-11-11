//! `chaperone install` and `uninstall`: the Claude Code plugin embedded in
//! this binary, as a one-plugin marketplace in the user data dir that
//! `claude plugin` registers. Claude Code loads a plugin from a local
//! marketplace in place, so rewriting the directory is the update. Never
//! reads the ledger or Claude Code's settings.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::child;
use serde_json::{Value, json};

use crate::output::{CliError, Outcome};

/// Every file under the repo's `plugin/`, by path relative to it.
const FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/plugin.rs"));
/// The plugin's name and its marketplace's.
const NAME: &str = "chaperone";
const ID: &str = "chaperone@chaperone";
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(120);

fn marketplace_dir() -> Result<PathBuf, CliError> {
    dirs::data_dir()
        .map(|d| d.join("chaperone").join("marketplace"))
        .ok_or_else(|| CliError::Internal("no user data directory".into()))
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> CliError {
    move |e| CliError::Internal(format!("{}: {e}", path.display()))
}

/// `claude <args>`, as `{args, exit, stderr}`; only a run that never
/// finished is an error.
fn claude(bin: &Path, args: &[&str]) -> Result<Value, CliError> {
    let failed = |why: String| CliError::Internal(format!("claude {}: {why}", args.join(" ")));
    let deadline = Instant::now() + CLAUDE_TIMEOUT;
    match child::run(
        std::process::Command::new(bin).args(args),
        deadline,
        1 << 20,
    ) {
        Ok(done) => Ok(json!({
            "args": args,
            "exit": done.status.code(),
            "stderr": String::from_utf8_lossy(&done.err).trim(),
        })),
        Err(child::Failed::TimedOut) => Err(failed("timed out".into())),
        Err(child::Failed::Spawn(e) | child::Failed::Wait(e)) => Err(failed(e.to_string())),
    }
}

fn remove(dir: &Path) -> Result<(), CliError> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(dir)(e)),
        _ => Ok(()),
    }
}

/// Writes the marketplace afresh, the plugin's version this binary's
/// (`<version>+<short sha>`), then `claude plugin marketplace add` and
/// `claude plugin install`, both of which pass when already done.
pub fn install(bin: &Path) -> Result<Outcome, CliError> {
    let dir = marketplace_dir()?;
    let version = match env!("CHAPERONE_GIT_SHA").get(..7) {
        Some(sha) => format!("{}+{sha}", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").into(),
    };
    remove(&dir)?;
    let marketplace = json!({
        "name": NAME,
        "description": "The chaperone plugin, written by chaperone install",
        "owner": { "name": "Chaperone" },
        "plugins": [{ "name": NAME, "source": format!("./{NAME}") }],
    });
    let write = |path: PathBuf, bytes: &[u8]| {
        std::fs::create_dir_all(path.parent().unwrap()).map_err(io(&path))?;
        std::fs::write(&path, bytes).map_err(io(&path))
    };
    for &(rel, bytes) in FILES {
        let path = dir.join(NAME).join(rel);
        if rel == ".claude-plugin/plugin.json" {
            let mut v: Value = serde_json::from_slice(bytes).expect("plugin.json is JSON");
            v["version"] = json!(version);
            write(path, format!("{v:#}\n").as_bytes())?;
        } else {
            write(path, bytes)?;
        }
    }
    let index = dir.join(".claude-plugin/marketplace.json");
    write(index, format!("{marketplace:#}\n").as_bytes())?;
    let mut ran = Vec::new();
    let add = dir
        .to_str()
        .ok_or_else(|| CliError::Internal(format!("{} is not UTF-8", dir.display())))?;
    for args in [
        &["plugin", "marketplace", "add", add][..],
        &["plugin", "install", ID, "--scope", "user"],
    ] {
        let run = claude(bin, args)?;
        if run["exit"] != 0 {
            return Err(CliError::Internal(format!(
                "claude {}: {}",
                args.join(" "),
                run["stderr"]
            )));
        }
        ran.push(run);
    }
    Ok(Outcome::One(
        json!({ "dir": dir, "plugin": ID, "version": version, "ran": ran }),
    ))
}

/// `claude plugin uninstall` and `marketplace remove`, whose refusal of
/// what is not there is reported, not an error; then the directory goes.
pub fn uninstall(bin: &Path) -> Result<Outcome, CliError> {
    let dir = marketplace_dir()?;
    let ran = vec![
        claude(bin, &["plugin", "uninstall", ID, "--scope", "user"])?,
        claude(bin, &["plugin", "marketplace", "remove", NAME])?,
    ];
    remove(&dir)?;
    Ok(Outcome::One(
        json!({ "dir": dir, "plugin": ID, "ran": ran }),
    ))
}
