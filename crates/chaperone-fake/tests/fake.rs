//! The scripted agent itself, started as a test starts it, through
//! `child::run_input`; and how a child in its own group ends.

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

/// Whether `pid` is still running, waiting up to 5 s for a killed
/// process to be reaped.
fn alive(pid: u32) -> bool {
    let up = || {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    let start = Instant::now();
    while up() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    up()
}

/// `script` under `/bin/sh -c`, with `$0` a file for the pid it writes.
fn sh(script: &str) -> (tempfile::TempDir, Command) {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(script).arg(dir.path().join("pid"));
    (dir, cmd)
}

fn pid(dir: &tempfile::TempDir) -> u32 {
    let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
    pid.trim().parse().unwrap()
}

/// At the deadline the agent gets SIGTERM, so a tool it put in a group of
/// its own is stopped by the agent, and then its group is killed, so a
/// tool left in that group goes too.
#[test]
fn a_timed_out_child_takes_what_it_started_with_it() {
    let scripts = [
        r#"sleep 30 & echo $! > "$0"; wait"#,
        // `set -m` puts the job in its own group, as the agent CLIs do.
        r#"set -m; sleep 30 & echo $! > "$0"; trap 'kill $!; echo > "$0.term"; exit 0' TERM; wait"#,
    ];
    for (i, script) in scripts.into_iter().enumerate() {
        let (dir, mut cmd) = sh(script);
        let deadline = Instant::now() + Duration::from_secs(2);
        assert!(matches!(
            child::run_input(&mut cmd, Some(b""), deadline, 1024),
            Err(Failed::TimedOut)
        ));
        let pid = pid(&dir);
        assert!(
            !alive(pid),
            "the sleeper {pid} of {script} outlived the run"
        );
        // The trap ran: where `set -m` does nothing (dash with no
        // terminal), the group kill alone would also end the sleeper.
        let termed = dir.path().join("pid.term").exists();
        assert_eq!(termed, i == 1, "{script}");
    }
}

/// A child that exits but leaves a process holding its stdout times out
/// at the deadline instead of waiting for that process.
#[test]
fn a_pipe_held_past_the_deadline_times_out() {
    let (dir, mut cmd) = sh(r#"sleep 6 & echo $! > "$0"; echo hi"#);
    let start = Instant::now();
    let done = child::run(&mut cmd, start + Duration::from_secs(1), 1024);
    assert!(matches!(done, Err(Failed::TimedOut)));
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "waited for the pipe"
    );
    let _ = Command::new("/bin/kill")
        .arg(pid(&dir).to_string())
        .status();
}

/// In a group of its own, a child that exits but leaves a process holding
/// its stdout is done at once with its output, and the process is killed.
#[test]
fn an_exited_child_takes_its_group_with_it() {
    let (dir, mut cmd) = sh(r#"sleep 47 & echo $! > "$0"; echo hi"#);
    let start = Instant::now();
    let done = child::run_input(&mut cmd, None, start + Duration::from_secs(20), 1024);
    let done = done.ok().unwrap();
    assert_eq!((done.status.code(), &*done.out), (Some(0), &b"hi\n"[..]));
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "waited for the pipe"
    );
    assert!(!alive(pid(&dir)), "the sleeper outlived the run");
}

/// `run_tail` keeps the end of both streams, however much was printed.
#[test]
fn run_tail_keeps_the_end_of_both_streams() {
    let (_dir, mut cmd) = sh("seq 1 100000; seq 1 100000 >&2");
    let deadline = Instant::now() + Duration::from_secs(20);
    let done = child::run_tail(&mut cmd, deadline, 7).ok().unwrap();
    assert_eq!(
        (&*done.out, &*done.err, done.cut),
        (&b"100000\n"[..], &b"100000\n"[..], true)
    );
}
