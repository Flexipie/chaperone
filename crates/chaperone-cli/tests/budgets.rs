//! R14 CI ceilings for the `chaperone` binary: S1, S2 and M1. Ignored by
//! default, because a debug build is not what the budgets describe:
//! `cargo test --release -p chaperone-cli --test budgets -- --ignored --nocapture`.

use std::process::{Command, Output};
use std::time::{Duration, Instant};

use chaperone_core::ledger::Ledger;

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");
const WARM: usize = 8;
const SAMPLES: usize = 100;

/// One test, so S1, S2 and M1 never spawn at the same time and time each
/// other. S2 is `work list` on a fresh ledger: a superset of opening the WAL
/// file, `SELECT 1` and exit, as in `docs/perf/v0-review.md`.
#[test]
#[ignore = "release-only budget; run with --ignored"]
fn startup_and_rss() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledger.db");
    Ledger::open_and_migrate(&db).unwrap();
    let db = db.to_str().unwrap();
    let list = ["--db", db, "work", "list"];

    let s1 = p50(&["--version"]);
    eprintln!("S1: --version p50 {s1:?}");
    assert!(s1 <= Duration::from_millis(100), "S1 CI ceiling: {s1:?}");

    let s2 = p50(&list);
    eprintln!("S2: work list (empty ledger) p50 {s2:?}");
    assert!(s2 <= Duration::from_millis(150), "S2 CI ceiling: {s2:?}");

    for args in [&["--version"][..], &list] {
        let mib = max_rss_mib(args);
        eprintln!("M1: {args:?} max RSS {mib:.2} MiB");
        assert!(mib <= 32.0, "M1 CI ceiling for {args:?}: {mib:.2} MiB");
    }
}

/// R14's startup method: 8 unmeasured spawns, then 100 samples, wall time
/// around the spawn. Every spawn must exit 0, so an error path is never timed.
fn p50(args: &[&str]) -> Duration {
    let mut times = Vec::with_capacity(SAMPLES);
    for i in 0..WARM + SAMPLES {
        let start = Instant::now();
        let out = Command::new(BIN).args(args).output().unwrap();
        let elapsed = start.elapsed();
        succeeded(args, &out);
        if i >= WARM {
            times.push(elapsed);
        }
    }
    times.sort();
    times[SAMPLES / 2]
}

/// Maximum resident set size of one run, from `/usr/bin/time`: bytes from
/// `-l` on macOS, KiB from `-v` (GNU time) on Linux.
fn max_rss_mib(args: &[&str]) -> f64 {
    let (flag, label, unit) = if cfg!(target_os = "macos") {
        ("-l", "maximum resident set size", 1.0)
    } else {
        ("-v", "Maximum resident set size (kbytes):", 1024.0)
    };
    assert!(
        Path::new("/usr/bin/time").exists(),
        "M1 needs /usr/bin/time"
    );
    let out = Command::new("/usr/bin/time")
        .arg(flag)
        .arg(BIN)
        .args(args)
        .output()
        .unwrap();
    succeeded(args, &out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr
        .lines()
        .find(|l| l.contains(label))
        .unwrap_or_else(|| panic!("no max RSS in /usr/bin/time output: {stderr}"));
    let n: f64 = line
        .split_whitespace()
        .find_map(|w| w.parse().ok())
        .unwrap_or_else(|| panic!("no number in {line:?}"));
    n * unit / (1024.0 * 1024.0)
}

fn succeeded(args: &[&str], out: &Output) {
    assert!(
        out.status.success(),
        "{args:?} exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}
