//! "Needs you": a lane waiting on the owner (the #109 decision). A lane
//! ready for you, one with a question to you, a blocked one, or one whose
//! agent stopped renewing its lease. Plain in review is not.

use chaperone_core::work::BoardLane;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Why {
    Blocked,
    Question,
    Ready,
    /// No ledger record says an agent died (`agent.*` is reserved for
    /// `chaperone supervise`), so a working lane's lapsed lease stands in.
    LeaseExpired,
}

impl Why {
    pub fn words(self) -> &'static str {
        match self {
            Why::Blocked => "blocked",
            Why::Question => "has a question",
            Why::Ready => "ready for you",
            Why::LeaseExpired => "lease expired",
        }
    }
}

/// Why `lane` needs the owner at `now` (seconds since the epoch), if it does.
/// Ready for you is a ready lane, or one whose `next_action` (set by handoffs
/// and `work update`) starts "needs you".
pub fn why(lane: &BoardLane, now: i64) -> Option<Why> {
    let item = &lane.board.item;
    let says = item
        .next_action
        .as_deref()
        .is_some_and(|n| n.trim_start().to_lowercase().starts_with("needs you"));
    if item.status == "blocked" {
        Some(Why::Blocked)
    } else if lane.open_questions > 0 {
        Some(Why::Question)
    } else if lane.board.ready || says {
        Some(Why::Ready)
    } else if matches!(item.status.as_str(), "claimed" | "active")
        && lane.lease_expires_secs.is_some_and(|at| at <= now)
    {
        Some(Why::LeaseExpired)
    } else {
        None
    }
}

/// How many of `lanes` need the owner at `now`.
pub fn count<'a>(lanes: impl IntoIterator<Item = &'a BoardLane>, now: i64) -> usize {
    lanes.into_iter().filter(|l| why(l, now).is_some()).count()
}
