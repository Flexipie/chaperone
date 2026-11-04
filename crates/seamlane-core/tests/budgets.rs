//! R14 performance budgets measured in the process. Ignored by default,
//! because a debug build is not what the budgets describe:
//! `cargo test --release -p seamlane-core --test budgets -- --ignored --nocapture`.

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use common::{AT, actor, committed, create_lane};
use seamlane_core::ledger::{Change, EventKind, Mutation, Payload};
use serde_json::json;

const COMMITS: usize = 1000;

/// E1: one writer, `synchronous=FULL`, `BEGIN IMMEDIATE`, one event plus
/// fold per commit, a payload of about 256 bytes. Timed around the loop only.
#[test]
#[ignore = "release-only budget; run with --ignored"]
fn e1_one_writer() {
    let (_dir, _path, ledger) = common::ledger();
    let who = actor("0");
    let lane = committed(ledger.apply(&who, None, AT, create_lane("L")).unwrap())
        .lane_id
        .unwrap();
    let note = "n".repeat(200);
    let mut last = json!(null);
    let start = Instant::now();
    for n in 0..COMMITS {
        let to = json!(format!("{note}{n}"));
        let changed = BTreeMap::from([(
            "next_action".to_string(),
            Change {
                from: last,
                to: to.clone(),
            },
        )]);
        last = to;
        let update = Mutation::Event {
            kind: EventKind::LaneUpdated,
            lane_id: Some(lane),
            spec_id: None,
            payload: Payload {
                changed,
                ..Payload::default()
            },
        };
        committed(ledger.apply(&who, None, AT, update).unwrap());
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed <= Duration::from_secs(10),
        "E1 CI ceiling: {elapsed:?}"
    );
    eprintln!(
        "E1: {COMMITS} commits in {elapsed:?}, {:.0} commits/s, mean {:.3} ms",
        COMMITS as f64 / elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / COMMITS as f64
    );
}
