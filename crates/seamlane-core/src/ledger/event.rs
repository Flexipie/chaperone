//! Event kinds and the payload fields the fold reads. The kinds are the M1
//! list in `docs/events.md`, checked by the drift test. Fields the fold does
//! not read pass through in `rest`; the slice that writes a kind types the
//! rest of its payload.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! kinds {
    ($($variant:ident => $name:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum EventKind { $($variant,)* }

        impl EventKind {
            pub const ALL: &[EventKind] = &[$(EventKind::$variant,)*];

            pub fn as_str(self) -> &'static str {
                match self { $(EventKind::$variant => $name,)* }
            }
        }

        impl FromStr for EventKind {
            type Err = String;

            /// Reserved and unknown kinds do not parse, so they cannot be written.
            fn from_str(s: &str) -> Result<Self, String> {
                match s {
                    $($name => Ok(EventKind::$variant),)*
                    _ => Err(format!("unknown event kind {s}")),
                }
            }
        }
    };
}

kinds! {
    SpecCreated => "spec.created",
    SpecRevised => "spec.revised",
    SpecApproved => "spec.approved",
    LaneCreated => "lane.created",
    LaneClaimed => "lane.claimed",
    LaneRenewed => "lane.renewed",
    LaneTakenOver => "lane.taken_over",
    LaneUpdated => "lane.updated",
    LaneReopened => "lane.reopened",
    LaneCancelled => "lane.cancelled",
    LaneReleased => "lane.released",
    LaneHandedOff => "lane.handed_off",
    LaneReadyOverridden => "lane.ready_overridden",
    PhaseEntered => "phase.entered",
    GateDenied => "gate.denied",
    CheckRequested => "check.requested",
    CheckFinished => "check.finished",
    FindingUpdated => "finding.updated",
    PrOpened => "pr.opened",
}

impl EventKind {
    pub fn is_spec(self) -> bool {
        matches!(
            self,
            EventKind::SpecCreated | EventKind::SpecRevised | EventKind::SpecApproved
        )
    }

    /// Kinds that carry a lease: `lease_secs` in, `lease_expires_at` set by
    /// the write from `at`.
    pub fn leases(self) -> bool {
        matches!(
            self,
            EventKind::LaneClaimed | EventKind::LaneRenewed | EventKind::LaneTakenOver
        )
    }

    /// Kinds that leave the lane row's `updated_at` alone (events.md "Write
    /// rules"), so a check recorded by a subagent never makes the lane stale.
    pub fn keeps_updated_at(self) -> bool {
        matches!(
            self,
            EventKind::GateDenied
                | EventKind::CheckRequested
                | EventKind::CheckFinished
                | EventKind::FindingUpdated
                | EventKind::LaneReadyOverridden
        )
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Actor {
    pub agent: Option<String>,
    pub session: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub from: Value,
    pub to: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub finding_key: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// The fields the fold reads, typed. Which ones a kind needs is checked by
/// the write before it appends.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub changed: BTreeMap<String, Change>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_event_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

// Built once per write and moved into it, never stored in bulk.
#[allow(clippy::large_enum_variant)]
pub enum Mutation {
    /// `lane_id` is None for `lane.created` and spec kinds; `spec_id` is None
    /// for `spec.created`. The write assigns those ids.
    Event {
        kind: EventKind,
        lane_id: Option<i64>,
        spec_id: Option<i64>,
        payload: Payload,
    },
    /// Extends the holder's lease from `at`. Writes no event.
    Heartbeat { lane_id: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub id: i64,
    pub kind: EventKind,
    pub lane_id: Option<i64>,
    pub spec_id: Option<i64>,
    pub actor: Actor,
    pub idempotency_key: Option<String>,
    pub payload: Payload,
    pub at: String,
}

/// Why a write was refused. Nothing was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    InvalidInput(String),
    NotFound {
        entity: &'static str,
        id: i64,
    },
    /// A `changed.from` that is not the current value, or a key already taken.
    Conflict(String),
    LeaseHeld {
        by: String,
        until: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum Applied {
    Committed(Event),
    /// The stored event for this idempotency key. Nothing was appended.
    Replayed(Event),
    Refused(Refusal),
    Heartbeat {
        lane_id: i64,
        lease_expires_at: String,
    },
}
