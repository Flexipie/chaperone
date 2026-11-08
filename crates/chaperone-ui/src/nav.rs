//! The left nav: All repos, one entry per repo on the board, Pipeline,
//! each with how many lanes need you.

use chaperone_core::work::LaneBoard;
use eframe::egui::{self, RichText};

use crate::{board, needs, theme};

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

/// Draws the nav and switches `view` on a click. A repo whose last open
/// lane closed has no entry, so its view falls back to All.
pub fn draw(ui: &mut egui::Ui, board: Option<&LaneBoard>, now: i64, view: &mut View) {
    let repos = board.map(board::group).unwrap_or_default();
    if let View::Repo(r) = view
        && board.is_some()
        && !repos.iter().any(|repo| repo.slug.eq_ignore_ascii_case(r))
    {
        *view = View::All;
    }
    ui.label(RichText::new("Chaperone").heading().strong());
    ui.add_space(10.0);
    let all = board.map_or(0, |b| needs::count(&b.lanes, now));
    entry(ui, view, View::All, "All repos", all);
    ui.add_space(6.0);
    ui.label(RichText::new("REPOS").small().weak());
    for repo in &repos {
        let lanes = repo.specs.iter().flat_map(|s| s.lanes.iter().copied());
        let n = needs::count(lanes, now);
        entry(ui, view, View::Repo(repo.slug.to_lowercase()), repo.slug, n);
    }
    ui.add_space(6.0);
    entry(ui, view, View::Pipeline, "Pipeline", 0);
}

fn entry(ui: &mut egui::Ui, view: &mut View, to: View, name: &str, needs: usize) {
    ui.horizontal(|ui| {
        if ui.selectable_label(*view == to, name).clicked() {
            *view = to;
        }
        theme::badge(ui, needs, theme::ALERT);
    });
}
