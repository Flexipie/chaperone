//! The budget ledger and R14 U2 for the spike.
//!
//!     seed <db>                  writes the ledger of chaperone-ui's U2 test
//!     seed u2 <db> <app>         U2 against the GPUI app on that ledger
//!
//! The ledger is `commit_to_redraw`'s in crates/chaperone-ui/src/watch.rs:
//! 50 lanes over 5 repos, a spec of 20 decisions, 200 events, a question
//! and 300 `usage.reported` per lane, every other lane merged. U2 starts the
//! app with `--trace`, commits a heartbeat, and times it to the app's
//! `notify <data_version>` line for the new board: the board handed to the
//! view as it asks for a redraw, where egui's test stops too. That includes
//! GPUI's hop from the background read to the main thread, not the render:
//! GPUI skips rendering a window that is not drawing, such as a hidden one.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chaperone_core::checks::{AttentionInput, CheckInput, MergedInput};
use chaperone_core::ledger::{Actor, EventKind, Ledger, Mutation, Payload};
use chaperone_core::work::{Brief, ClaimInput, CreateInput, HandoffInput, UpdateInput};

const LANES: i64 = 50;
const EVENTS: u64 = 100;
const USAGE: u64 = 300;
const SAMPLES: usize = 50;
/// The seed's spec, which `u2` checks for before it writes.
const SPEC: &str = "Flexipie/repo0#1";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [db] => seed(Path::new(db)),
        ["u2", db, app] => u2(Path::new(db), app),
        _ => {
            eprintln!("usage: seed <db> | seed u2 <db> <app>");
            std::process::exit(2);
        }
    }
}

fn lane(s: &str) -> Actor {
    Actor {
        agent: Some("lane".into()),
        session: Some(s.into()),
    }
}

fn seed(path: &Path) {
    assert!(!path.exists(), "{} exists; seed a new file", path.display());
    let rw = Ledger::open_and_migrate(path).unwrap();
    let checker = lane("checker");
    let decisions: String = (0..20)
        .map(|n| format!("- Q: question {n}? A: answer {n}.\n"))
        .collect();
    let body = format!("# P\n\n## slice 1: one  (Flexipie/repo0#2)\n\n## decisions\n\n{decisions}");
    rw.spec_create(&checker, SPEC, None, &body, &rw.now().unwrap(), false)
        .unwrap()
        .unwrap();
    for id in 1..=LANES {
        let me = lane(&format!("s{id}"));
        let create = CreateInput {
            title: format!("lane {id}"),
            repo: Some(format!("Flexipie/repo{}", id % 5)),
            source: Some("github".into()),
            source_id: Some(format!("Flexipie/repo0#{}", id + 1)),
            spec: Some(SPEC.into()),
            lane_key: Some(format!("l{id}")),
            kind: "code".into(),
            pr_flow: "pr".into(),
            ..CreateInput::default()
        };
        rw.work_create(&me, &create, None, false).unwrap().unwrap();
        let claim = ClaimInput {
            branch: Some(format!("b{id}")),
            ..ClaimInput::default()
        };
        rw.work_claim(&me, id, &claim, None, false)
            .unwrap()
            .unwrap();
        if id == 1 {
            for job in 1..=5 {
                let view = serde_json::json!({
                    "job_id": job, "source": "local", "status": "reviewed",
                    "phase": "done", "done": true, "repo": "Flexipie/repo1",
                    "branch": "b1", "head_sha": "abc1234", "score": job,
                    "verdict": "safe", "summary": "s", "findings": [],
                });
                rw.review_record(&checker, id, job, &view).unwrap().unwrap();
            }
            let handoff = HandoffInput {
                summary: "built".into(),
                commit: Some("abc1234".into()),
                brief: Some(Brief {
                    goal: "a goal".into(),
                    check_by_hand: vec!["the look".into()],
                    ..Brief::default()
                }),
                ..HandoffInput::default()
            };
            rw.work_handoff(&me, id, &handoff, None, false)
                .unwrap()
                .unwrap();
        }
        for n in 0..EVENTS {
            let update = UpdateInput {
                next_action: Some(format!("step {n}")),
                ..UpdateInput::default()
            };
            rw.work_update(&me, id, &update, None, None, false)
                .unwrap()
                .unwrap();
            let check = CheckInput {
                work: id,
                check: "plan_check".into(),
                head: "abc1234".into(),
                outcome: "ran".into(),
                findings: Some(n),
                ..CheckInput::default()
            };
            rw.check_record(&checker, &check).unwrap().unwrap();
        }
        let ask = AttentionInput {
            work: id,
            kind: "asked".into(),
            by: "owner".into(),
            reason: Some("question.scope".into()),
            note: Some("split it?".into()),
            ..AttentionInput::default()
        };
        rw.attention_record(&checker, &ask).unwrap().unwrap();
        let at = rw.now().unwrap();
        for n in 0..USAGE {
            let payload = Payload {
                source: Some(if n % 3 == 0 { "subagent" } else { "parent" }.into()),
                rest: serde_json::json!({
                    "message_id": format!("m{n}"), "input_total": 1000 * n as i64,
                    "cache_read": 900 * n as i64, "cache_write": 50, "output_total": 40,
                })
                .as_object()
                .unwrap()
                .clone(),
                ..Payload::default()
            };
            let event = Mutation::Event {
                kind: EventKind::UsageReported,
                lane_id: Some(id),
                spec_id: None,
                payload,
            };
            let key = format!("usage:{id}:m{n}");
            rw.apply(&checker, Some(&key), &at, event).unwrap();
        }
        if id % 2 == 0 {
            let pr = UpdateInput {
                pr_ref: Some(format!("Flexipie/repo0#{}", 100 + id)),
                ..UpdateInput::default()
            };
            rw.work_update(&me, id, &pr, None, None, false)
                .unwrap()
                .unwrap();
            let merged = MergedInput {
                work: id,
                pr: format!("Flexipie/repo0#{}", 100 + id),
                merge_sha: "fedcba9".into(),
                head: "abc1234".into(),
                merged_at: at,
                patch_id: None,
            };
            rw.work_merged(&checker, &merged).unwrap().unwrap();
        }
    }
    println!("seeded {}", path.display());
}

fn p(times: &mut [Duration], q: usize) -> Duration {
    times.sort();
    times[(times.len() - 1) * q / 100]
}

fn u2(path: &Path, app: &str) {
    let rw = Ledger::open(path).unwrap();
    // It heartbeats lanes 1-50: only on a ledger `seed` wrote, never a real one.
    if !matches!(rw.spec_get(SPEC, None), Ok(Ok(_))) {
        eprintln!("{} has no spec {SPEC}: not a seeded ledger", path.display());
        std::process::exit(1);
    }
    let child = Command::new(app)
        .arg("--db")
        .arg(path)
        .arg("--trace")
        .stdout(Stdio::piped())
        .spawn()
        .expect("start the app");
    let mut child = Kill(child);
    let (tx, rx) = mpsc::channel();
    let out = BufReader::new(child.0.stdout.take().unwrap());
    thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            if let Some(v) = line.strip_prefix("notify ") {
                let _ = tx.send((v.parse::<i64>().unwrap(), Instant::now()));
            }
        }
    });
    let wait = Duration::from_secs(10);
    let (mut seen, _) = rx.recv_timeout(wait).expect("the first board");
    let mut lags = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let id = i as i64 % LANES + 1;
        let start = Instant::now();
        rw.work_heartbeat(&lane(&format!("s{id}")), id, None, false)
            .unwrap()
            .unwrap();
        let notified = loop {
            let (v, at) = rx.recv_timeout(wait).expect("a new board");
            if v != seen {
                seen = v;
                break at;
            }
        };
        lags.push(notified - start);
    }
    let (p50, p99) = (p(&mut lags, 50), p(&mut lags, 99));
    println!("U2 commit to repaint request, {SAMPLES} samples: p50 {p50:?}, p99 {p99:?}");
}

/// The app, killed when U2 ends, a panic too.
struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
