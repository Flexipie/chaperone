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

/// Repos by slug, any case (the first spelling seen is shown); in each,
/// specs by id with lanes without one last.
pub fn group(board: &LaneBoard) -> Vec<Repo<'_>> {
    let mut repos: BTreeMap<String, (&str, BTreeMap<SpecKey, Vec<&BoardLane>>)> = BTreeMap::new();
    for lane in &board.lanes {
        let item = &lane.board.item;
        let slug = item.repo_slug.as_deref().unwrap_or("(no repo)");
        repos
            .entry(slug.to_lowercase())
            .or_insert_with(|| (slug, BTreeMap::new()))
            .1
            .entry((item.spec_id.is_none(), item.spec_id))
            .or_default()
            .push(lane);
    }
    repos
        .into_values()
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

/// Time from `then` to `now`, both seconds since the epoch: `<1m`, `5m`,
/// `3h`, `2d`.
pub fn age(then: i64, now: i64) -> String {
    match (now - then).max(0) {
        s if s < 60 => "<1m".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}
