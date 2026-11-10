//! Scripted agent for lane tests: it stands in for `claude`, started under
//! any name (a test links it as `<dir>/claude`).
//!
//! The script is the JSON file `<argv0>.json`:
//! `{"version": "...", "runs": [{"when": "...", "stdout": "...", "stderr":
//! "...", "exit": 0, "sleep_ms": 0}]}`. `--version` prints the version.
//! Any other call reads the prompt from stdin, appends one JSON line
//! `{args, env, cwd, prompt, files}` (`env` the variable names, `files`
//! the text of each argument that names a file) to `<argv0>.log`, and
//! plays the first run whose `when` is in the prompt: its output, its
//! exit code, after its sleep. A run's `"write": {"-o": "text"}` writes
//! the text to the path after that flag, as an agent writes its answer to
//! a file. With no such run it exits 64. A script with `"stdin_bytes": n`
//! reads only that much of the prompt.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Script {
    version: String,
    stdin_bytes: Option<u64>,
    #[serde(default)]
    runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Run {
    when: String,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    exit: u8,
    #[serde(default)]
    sleep_ms: u64,
    #[serde(default)]
    write: BTreeMap<String, String>,
}

fn main() -> ExitCode {
    let mut args = std::env::args();
    let me = args.next().unwrap_or_default();
    let args: Vec<String> = args.collect();
    let script = std::fs::read_to_string(format!("{me}.json")).unwrap_or_default();
    let Ok(script) = serde_json::from_str::<Script>(&script) else {
        eprintln!("chaperone-fake: no script at {me}.json");
        return ExitCode::from(64);
    };
    if args == ["--version"] {
        println!("{}", script.version);
        return ExitCode::SUCCESS;
    }
    let mut prompt = String::new();
    let stdin = std::io::stdin().take(script.stdin_bytes.unwrap_or(u64::MAX));
    let _ = { stdin }.read_to_string(&mut prompt);
    let call = json!({
        "args": args,
        "env": std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "cwd": std::env::current_dir().unwrap_or_default(),
        "prompt": prompt,
        "files": args.iter().filter_map(|a| Some((a, std::fs::read_to_string(a).ok()?))).collect::<BTreeMap<_, _>>(),
    });
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{me}.log"));
    if let Ok(mut log) = log {
        // One write, so calls at once do not mix their lines.
        let _ = log.write_all(format!("{call}\n").as_bytes());
    }
    let Some(run) = script.runs.iter().find(|r| prompt.contains(&r.when)) else {
        eprintln!("chaperone-fake: no run for this prompt");
        return ExitCode::from(64);
    };
    std::thread::sleep(Duration::from_millis(run.sleep_ms));
    for (flag, text) in &run.write {
        let at = args.iter().position(|a| a == flag);
        if let Some(path) = at.and_then(|at| args.get(at + 1)) {
            let _ = std::fs::write(path, text);
        }
    }
    print!("{}", run.stdout);
    eprint!("{}", run.stderr);
    ExitCode::from(run.exit)
}
