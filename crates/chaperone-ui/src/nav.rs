//! The left nav: All repos, one entry per repo with open lanes or recent
//! merges, Pipeline, each with how many lanes need you.

use chaperone_core::stats::BoardStats;
use chaperone_core::work::LaneBoard;
use eframe::egui::{self, RichText};

use crate::{needs, repo, theme};

/// What the main view shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    All,
    /// One repo, by its slug lowercased (repos match in any case).
    Repo(String),
    Pipeline,
}

impl View {
    /// Whether a repo slug, any case, is in this view.
    pub fn shows(&self, slug: &str) -> bool {
        match self {
            View::All => true,
            View::Repo(r) => slug.eq_ignore_ascii_case(r),
            View::Pipeline => false,
        }
    }
}

/// Draws the nav and switches `view` on a click. A repo with no open lane
/// and no recent merge has no entry, so its view falls back to All, once
/// stats at least as new as the board are read: a merge-only repo's entry
/// comes from them.
pub fn draw(
    ui: &mut egui::Ui,
    board: Option<&LaneBoard>,
    stats: Option<&BoardStats>,
    now: i64,
    view: &mut View,
) {
    let repos = repo::repos(board, stats);
    let caught_up = board
        .zip(stats)
        .is_some_and(|(b, s)| s.data_version >= b.data_version);
    if let View::Repo(r) = view
        && caught_up
        && !repos.iter().any(|slug| slug.eq_ignore_ascii_case(r))
    {
        *view = View::All;
    }
    ui.label(RichText::new("Chaperone").heading().strong());
    ui.add_space(10.0);
    let all = board.map_or(0, |b| needs::count(&b.lanes, now));
    entry(ui, view, View::All, "All repos", all);
    ui.add_space(6.0);
    ui.label(RichText::new("REPOS").small().weak());
    for slug in repos {
        let n = board.map_or(0, |b| needs::count(repo::lanes_of(b, slug), now));
        entry(ui, view, View::Repo(slug.to_lowercase()), slug, n);
    }
    ui.add_space(6.0);
    entry(ui, view, View::Pipeline, "Pipeline", 0);
}

fn entry(ui: &mut egui::Ui, view: &mut View, to: View, name: &str, needs: usize) {
    ui.horizontal(|ui| {
        if ui.selectable_label(*view == to, name).clicked() {
            *view = to;
        }
        theme::badge(ui, needs);
    });
}
