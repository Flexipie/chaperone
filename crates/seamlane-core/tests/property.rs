//! R14 properties for the write function: rebuild, retry, refusal, lease.
//! 256 cases, the clock passed in, and the seed taken from the commit sha
//! (`SEAMLANE_SEED` overrides it) and printed, so a failure replays.

mod common;

use std::collections::BTreeMap;

use common::{dump, read_only};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use rusqlite::Connection;
use seamlane_core::ledger::{
    Actor, Applied, Change, EventKind, Finding, Findings, Ledger, Mutation, Payload, Refusal,
};
use serde_json::{Value, json};

const STATUSES: &[&str] = &[
    "queued",
    "claimed",
    "active",
    "blocked",
    "review",
    "done",
    "cancelled",
];

fn seed() -> u64 {
    let given = std::env::var("SEAMLANE_SEED").ok();
    let sha = given
        .clone()
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .or_else(|| {
            let out = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()?;
            Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
        })
        .unwrap_or_default();
    let seed = u64::from_str_radix(sha.get(..16).unwrap_or(&sha), 16).unwrap_or_else(|_| {
        assert!(given.is_none(), "SEAMLANE_SEED={sha} is not hex");
        0
    });
    eprintln!("property seed {seed:016x} (rerun: SEAMLANE_SEED={seed:016x})");
    seed
}

fn config() -> Config {
    Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(seed()),
        failure_persistence: None,
        ..Config::default()
    }
}

#[derive(Clone, Debug)]
enum Op {
    CreateSpec {
        key: u8,
    },
    ReviseSpec {
        spec: u8,
        stale: bool,
    },
    CreateLane {
        spec: Option<u8>,
        key: u8,
    },
    Claim {
        lane: u8,
        who: u8,
        secs: i64,
        stale: bool,
        takeover: bool,
    },
    Renew {
        lane: u8,
        who: u8,
    },
    Release {
        lane: u8,
        who: u8,
    },
    Update {
        lane: u8,
        who: u8,
        status: usize,
        stale: bool,
    },
    Heartbeat {
        lane: u8,
        who: u8,
    },
    Check {
        lane: u8,
        who: u8,
        ran: bool,
        sha: u8,
        findings: u8,
        review: bool,
    },
    FindingUpdate {
        lane: u8,
        who: u8,
    },
    Ready {
        lane: u8,
        who: u8,
    },
    Advance {
        secs: i64,
    },
}

fn op() -> impl Strategy<Value = Op> {
    let who = 0u8..3;
    prop_oneof![
        (0u8..4).prop_map(|key| Op::CreateSpec { key }),
        (0u8..4, any::<bool>()).prop_map(|(spec, stale)| Op::ReviseSpec { spec, stale }),
        (proptest::option::of(0u8..3), 0u8..4).prop_map(|(spec, key)| Op::CreateLane { spec, key }),
        (
            0u8..5,
            who.clone(),
            prop_oneof![Just(10), Just(60), Just(900)],
            any::<bool>(),
            any::<bool>()
        )
            .prop_map(|(lane, who, secs, stale, takeover)| Op::Claim {
                lane,
                who,
                secs,
                stale,
                takeover
            }),
        (0u8..5, who.clone()).prop_map(|(lane, who)| Op::Renew { lane, who }),
        (0u8..5, who.clone()).prop_map(|(lane, who)| Op::Release { lane, who }),
        (0u8..5, who.clone(), 0..STATUSES.len(), any::<bool>()).prop_map(
            |(lane, who, status, stale)| Op::Update {
                lane,
                who,
                status,
                stale
            }
        ),
        (0u8..5, who.clone()).prop_map(|(lane, who)| Op::Heartbeat { lane, who }),
        (
            0u8..5,
            who.clone(),
            any::<bool>(),
            0u8..2,
            0u8..3,
            any::<bool>()
        )
            .prop_map(|(lane, who, ran, sha, findings, review)| Op::Check {
                lane,
                who,
                ran,
                sha,
                findings,
                review
            }),
        (0u8..5, who.clone()).prop_map(|(lane, who)| Op::FindingUpdate { lane, who }),
        (0u8..5, who).prop_map(|(lane, who)| Op::Ready { lane, who }),
        prop_oneof![Just(5), Just(120), Just(1000)].prop_map(|secs| Op::Advance { secs }),
    ]
}

/// An op with an optional idempotency key from a small set, so keys repeat.
fn step() -> impl Strategy<Value = (Op, Option<u8>)> {
    (op(), proptest::option::weighted(0.3, 0u8..6))
}

fn actor(who: u8) -> Actor {
    Actor {
        agent: Some(format!("a{who}")),
        session: Some(format!("s{who}")),
    }
}

struct World {
    _dir: tempfile::TempDir,
    ledger: Ledger,
    db: Connection,
    secs: i64,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let ledger = Ledger::open_and_migrate(&path).unwrap();
        let db = read_only(&path);
        Self {
            _dir: dir,
            ledger,
            db,
            secs: 0,
        }
    }

    fn at(&self) -> String {
        self.db
            .query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', '2026-11-03T00:00:00.000Z', ?1 || ' seconds')",
                [format!("+{}", self.secs)],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    /// A lane id: an existing lane, or one past the last (not found).
    fn lane(&self, n: u8) -> i64 {
        n as i64 % (self.count("lanes") + 1) + 1
    }

    fn lane_value(&self, id: i64, column: &str) -> Value {
        self.db
            .query_row(
                &format!("SELECT {column} FROM lanes WHERE id = ?1"),
                [id],
                |r| {
                    Ok(match r.get_ref(0)? {
                        rusqlite::types::ValueRef::Integer(i) => json!(i),
                        rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        _ => Value::Null,
                    })
                },
            )
            .unwrap_or(Value::Null)
    }

    /// `changed` from the current row, or from a wrong value when stale.
    fn change(&self, lane: i64, sets: &[(&str, Value)], stale: bool) -> BTreeMap<String, Change> {
        sets.iter()
            .map(|(field, to)| {
                let from = if stale {
                    json!("stale")
                } else {
                    self.lane_value(lane, field)
                };
                (
                    field.to_string(),
                    Change {
                        from,
                        to: to.clone(),
                    },
                )
            })
            .collect()
    }

    fn mutation(&mut self, op: &Op) -> Option<(Actor, Mutation)> {
        let event = |kind, lane_id, spec_id, payload| Mutation::Event {
            kind,
            lane_id,
            spec_id,
            payload,
        };
        Some(match *op {
            Op::Advance { secs } => {
                self.secs += secs;
                return None;
            }
            Op::CreateSpec { key } => (
                actor(0),
                event(
                    EventKind::SpecCreated,
                    None,
                    None,
                    Payload {
                        parent_key: Some(format!("Flexipie/harness#{key}")),
                        title: Some("t".into()),
                        body: Some("b".into()),
                        version: Some(1),
                        ..Payload::default()
                    },
                ),
            ),
            Op::ReviseSpec { spec, stale } => {
                let id = spec as i64 % (self.count("specs") + 1) + 1;
                let head: i64 = self
                    .db
                    .query_row("SELECT head_version FROM specs WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })
                    .unwrap_or(0);
                let version = if stale { head } else { head + 1 };
                (
                    actor(0),
                    event(
                        EventKind::SpecRevised,
                        None,
                        Some(id),
                        Payload {
                            version: Some(version),
                            body: Some(format!("b{version}")),
                            ..Payload::default()
                        },
                    ),
                )
            }
            Op::CreateLane { spec, key } => (
                actor(0),
                event(
                    EventKind::LaneCreated,
                    None,
                    spec.map(|s| s as i64 + 1),
                    Payload {
                        lane_key: Some(format!("L{key}")),
                        title: Some("t".into()),
                        source: Some("manual".into()),
                        kind: Some("code".into()),
                        pr_flow: Some("pr".into()),
                        ..Payload::default()
                    },
                ),
            ),
            Op::Claim {
                lane,
                who,
                secs,
                stale,
                takeover,
            } => {
                let id = self.lane(lane);
                let a = actor(who);
                let changed = self.change(
                    id,
                    &[
                        ("status", json!("claimed")),
                        ("agent", json!(a.agent)),
                        ("session", json!(a.session)),
                    ],
                    stale,
                );
                let kind = if takeover {
                    EventKind::LaneTakenOver
                } else {
                    EventKind::LaneClaimed
                };
                let payload = Payload {
                    changed,
                    lease_secs: Some(secs),
                    ..Payload::default()
                };
                (a, event(kind, Some(id), None, payload))
            }
            Op::Renew { lane, who } => (
                actor(who),
                event(
                    EventKind::LaneRenewed,
                    Some(self.lane(lane)),
                    None,
                    Payload {
                        lease_secs: Some(900),
                        ..Payload::default()
                    },
                ),
            ),
            Op::Release { lane, who } => {
                let id = self.lane(lane);
                let changed = self.change(
                    id,
                    &[
                        ("status", json!("queued")),
                        ("agent", Value::Null),
                        ("session", Value::Null),
                    ],
                    false,
                );
                (
                    actor(who),
                    event(
                        EventKind::LaneReleased,
                        Some(id),
                        None,
                        Payload {
                            changed,
                            ..Payload::default()
                        },
                    ),
                )
            }
            Op::Update {
                lane,
                who,
                status,
                stale,
            } => {
                let id = self.lane(lane);
                let changed = self.change(id, &[("status", json!(STATUSES[status]))], stale);
                (
                    actor(who),
                    event(
                        EventKind::LaneUpdated,
                        Some(id),
                        None,
                        Payload {
                            changed,
                            ..Payload::default()
                        },
                    ),
                )
            }
            Op::Heartbeat { lane, who } => (
                actor(who),
                Mutation::Heartbeat {
                    lane_id: self.lane(lane),
                },
            ),
            Op::Check {
                lane,
                who,
                ran,
                sha,
                findings,
                review,
            } => {
                // A review lists its findings; a session check counts them.
                let (check, findings) = if review {
                    let list = (0..findings)
                        .map(|n| Finding {
                            finding_key: format!("f{n}"),
                            severity: "risk".into(),
                            title: "t".into(),
                            body: "b".into(),
                            path: None,
                            rest: Default::default(),
                        })
                        .collect();
                    ("local_review", Findings::List(list))
                } else {
                    ("plan_check", Findings::Count(findings.into()))
                };
                (
                    actor(who + 10),
                    event(
                        EventKind::CheckFinished,
                        Some(self.lane(lane)),
                        None,
                        Payload {
                            check: Some(check.into()),
                            outcome: Some(if ran { "ran" } else { "skipped" }.into()),
                            head_sha: Some(format!("abc123{sha}")),
                            findings: Some(findings),
                            ..Payload::default()
                        },
                    ),
                )
            }
            Op::FindingUpdate { lane, who } => {
                let id = self.lane(lane);
                let (check, key): (i64, String) = self
                    .db
                    .query_row(
                        "SELECT check_event_id, finding_key FROM findings WHERE lane_id = ?1 LIMIT 1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap_or((999, "f0".into()));
                (
                    actor(who + 10),
                    event(
                        EventKind::FindingUpdated,
                        Some(id),
                        None,
                        Payload {
                            check_event_id: Some(check),
                            finding_key: Some(key),
                            status: Some("fixed".into()),
                            ..Payload::default()
                        },
                    ),
                )
            }
            Op::Ready { lane, who } => (
                actor(who + 20),
                event(
                    EventKind::LaneReadyOverridden,
                    Some(self.lane(lane)),
                    None,
                    Payload {
                        rest: [("ready".to_string(), json!(true))].into_iter().collect(),
                        ..Payload::default()
                    },
                ),
            ),
        })
    }

    fn apply(&mut self, op: &Op, key: Option<u8>) -> Option<Applied> {
        let (actor, mutation) = self.mutation(op)?;
        let key = match mutation {
            Mutation::Heartbeat { .. } => None,
            _ => key.map(|k| format!("k{k}")),
        };
        let at = self.at();
        Some(
            self.ledger
                .apply(&actor, key.as_deref(), &at, mutation)
                .unwrap(),
        )
    }
}

fn lanes_without_lease(db: &Connection) -> Vec<String> {
    let mut stmt = db
        .prepare(
            "SELECT id, spec_id, lane_key, status, agent, session, lease_secs, head_event_id,
                    updated_at FROM lanes ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((0..9)
            .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
            .collect::<Vec<_>>()
            .join("|"))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

proptest! {
    #![proptest_config(config())]

    /// A random sequence folded from the log equals the tables, at a random
    /// prefix and at the end.
    #[test]
    fn rebuild_matches_log(steps in proptest::collection::vec(step(), 1..60), cut in any::<prop::sample::Index>()) {
        let mut w = World::new();
        let cut = cut.index(steps.len());
        for (i, (op, key)) in steps.iter().enumerate() {
            w.apply(op, *key);
            if i == cut || i + 1 == steps.len() {
                let before = dump(&w.db);
                w.ledger.rebuild().unwrap();
                prop_assert_eq!(&before, &dump(&w.db), "rebuild after step {}", i);
            }
        }
    }

    /// The same key inserts one row: a retry, even with another payload,
    /// returns the stored event and changes nothing.
    #[test]
    fn retry_replays_one_row(steps in proptest::collection::vec(step(), 1..40)) {
        let mut w = World::new();
        for (op, key) in &steps {
            let Some(Applied::Committed(event)) = w.apply(op, *key) else { continue };
            let Some(key) = event.idempotency_key.clone() else { continue };
            let before = dump(&w.db);
            let other = Mutation::Event {
                kind: EventKind::LaneUpdated,
                lane_id: Some(999),
                spec_id: None,
                payload: Payload::default(),
            };
            let at = w.at();
            let again = w.ledger.apply(&actor(2), Some(&key), &at, other).unwrap();
            prop_assert_eq!(again, Applied::Replayed(event));
            prop_assert_eq!(&before, &dump(&w.db));
        }
    }

    /// A refused write leaves every table, the log included, as it was.
    #[test]
    fn refusal_writes_nothing(steps in proptest::collection::vec(step(), 1..60)) {
        let mut w = World::new();
        for (op, key) in &steps {
            let before = dump(&w.db);
            if let Some(Applied::Refused(_)) = w.apply(op, *key) {
                prop_assert_eq!(&before, &dump(&w.db), "{:?}", op);
            }
        }
    }

    /// Two claimants who read the same row: one wins, the loser appends no
    /// event. A live lease refuses the other pair; a heartbeat moves only
    /// the holder's lease columns and appends nothing.
    #[test]
    fn lease_one_winner(a in 0u8..3, b_off in 1u8..3, secs in 30i64..=86_400, beats in 1usize..4) {
        let b = (a + b_off) % 3;
        let mut w = World::new();
        w.apply(&Op::CreateLane { spec: None, key: 0 }, None);
        let claim = |w: &mut World, who| {
            w.mutation(&Op::Claim { lane: 0, who, secs, stale: false, takeover: false }).unwrap()
        };
        let first = claim(&mut w, a);
        let second = claim(&mut w, b);
        let at = w.at();
        let won = w.ledger.apply(&first.0, None, &at, first.1).unwrap();
        prop_assert!(matches!(won, Applied::Committed(_)));
        let events = w.count("events");
        let lost = w.ledger.apply(&second.0, None, &at, second.1).unwrap();
        prop_assert!(matches!(lost, Applied::Refused(Refusal::Conflict(_) | Refusal::LeaseHeld { .. })), "{:?}", lost);
        prop_assert_eq!(w.count("events"), events);

        let before = dump(&w.db);
        let held = w.apply(&Op::Update { lane: 0, who: b, status: 2, stale: false }, None).unwrap();
        prop_assert!(matches!(held, Applied::Refused(Refusal::LeaseHeld { .. })), "{:?}", held);
        prop_assert_eq!(&before, &dump(&w.db));

        for _ in 0..beats {
            w.secs += 10;
            let lanes = lanes_without_lease(&w.db);
            let beat = w.apply(&Op::Heartbeat { lane: 0, who: a }, None).unwrap();
            prop_assert!(matches!(beat, Applied::Heartbeat { lane_id: 1, .. }), "{:?}", beat);
            prop_assert_eq!(w.count("events"), events);
            prop_assert_eq!(&lanes, &lanes_without_lease(&w.db));
            prop_assert_eq!(w.lane_value(1, "last_heartbeat_at"), json!(w.at()));
        }
        let before = dump(&w.db);
        let other = w.apply(&Op::Heartbeat { lane: 0, who: b }, None).unwrap();
        prop_assert!(matches!(other, Applied::Refused(Refusal::LeaseHeld { .. })), "{:?}", other);
        prop_assert_eq!(&before, &dump(&w.db));
    }
}
