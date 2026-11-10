//! The scripted agent itself, started as a test starts it, through
//! `child::run_input`.

use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::child::{self, Failed};

/// It says its version, plays the run its prompt names, logs the call,
/// and exits 64 for a prompt it has no run for. A child that stops reading
/// its input is a failed run, whatever it printed.
#[test]
fn the_fake_plays_its_script() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("claude");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_chaperone-fake"), &bin).unwrap();
    let script = |stdin_bytes: &str| {
        let runs = r#"[{"when": "review", "stdout": "found", "stderr": "warn", "exit": 3}]"#;
        let script = format!(r#"{{"version": "1.2 (fake)", {stdin_bytes} "runs": {runs}}}"#);
        std::fs::write(dir.path().join("claude.json"), script).unwrap();
    };
    let run = |args: &[&str], input: &str| {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut cmd = Command::new(&bin);
        child::run_input(cmd.args(args), Some(input.as_bytes()), deadline, 1024)
    };
    script("");
    let version = run(&["--version"], "").ok().unwrap();
    assert_eq!(version.out, b"1.2 (fake)\n");

    let done = run(&["-p", "--model", "opus"], "review this").ok().unwrap();
    assert_eq!(done.status.code(), Some(3));
    assert_eq!((&*done.out, &*done.err), (&b"found"[..], &b"warn"[..]));
    assert_eq!(run(&[], "unknown").ok().unwrap().status.code(), Some(64));
    let log = std::fs::read_to_string(dir.path().join("claude.log")).unwrap();
    let calls: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0]["args"],
        serde_json::json!(["-p", "--model", "opus"])
    );
    assert_eq!(calls[0]["prompt"], "review this");

    script(r#""stdin_bytes": 6,"#);
    let long = format!("review {}", "x".repeat(2 * 1024 * 1024));
    assert!(matches!(run(&[], &long), Err(Failed::Wait(_))));
}
