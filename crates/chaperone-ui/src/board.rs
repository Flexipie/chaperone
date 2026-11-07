//! The board grouped for drawing: repo, then spec, lanes in board order.

use std::collections::BTreeMap;

use chaperone_core::work::{BoardLane, LaneBoard, SpecTitle};

pub struct Repo<'a> {
    pub slug: &'a str,
    pub specs: Vec<Spec<'a>>,
}

pub struct Spec<'a> {
    /// None for lanes with no spec.
    pub spec: Option<&'a SpecTitle>,
    pub lanes: Vec<&'a BoardLane>,
}

/// Sorts lanes with no spec after the rest.
type SpecKey = (bool, Option<i64>);

/// Repos by slug; in each, specs by id with lanes without one last.
pub fn group(board: &LaneBoard) -> Vec<Repo<'_>> {
    let mut repos: BTreeMap<&str, BTreeMap<SpecKey, Vec<&BoardLane>>> = BTreeMap::new();
    for lane in &board.lanes {
        let item = &lane.board.item;
        let slug = item.repo_slug.as_deref().unwrap_or("(no repo)");
        repos
            .entry(slug)
            .or_default()
            .entry((item.spec_id.is_none(), item.spec_id))
            .or_default()
            .push(lane);
    }
    repos
        .into_iter()
        .map(|(slug, specs)| Repo {
            slug,
            specs: specs
                .into_iter()
                .map(|((_, id), lanes)| Spec {
                    spec: id.and_then(|id| board.specs.iter().find(|s| s.id == id)),
                    lanes,
                })
                .collect(),
        })
        .collect()
}

/// Time since `at` (the ledger's RFC 3339 UTC form) at `now`, in seconds
/// since the epoch: `<1m`, `5m`, `3h`, `2d`.
pub fn age(at: &str, now: i64) -> String {
    let Some(then) = unix_secs(at) else {
        return "?".into();
    };
    match (now - then).max(0) {
        s if s < 60 => "<1m".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// `2026-11-07T05:01:02.693Z` as seconds since the epoch (days from civil,
/// H. Hinnant), the fraction dropped.
fn unix_secs(at: &str) -> Option<i64> {
    let n = |from: usize, to: usize| at.get(from..to)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let secs = n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?;
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(unix_secs("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(unix_secs("2026-11-07T05:01:02.693Z"), Some(1_794_027_662));
        let now = 1_794_027_662;
        assert_eq!(age("2026-11-07T05:01:02.693Z", now), "<1m");
        assert_eq!(age("2026-11-07T04:01:02.000Z", now), "1h");
        assert_eq!(age("2026-11-05T05:01:02.000Z", now), "2d");
        assert_eq!(age("garbage", now), "?");
    }
}
