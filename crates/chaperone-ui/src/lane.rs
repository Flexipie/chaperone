//! The lane page (#109 slice 4): what the lane is, how it is doing against
//! its repo and what it changed. A header, six tiles, the timeline, the
//! decisions, what to check by hand and the files, then the #41 panel's
//! handoffs, findings, areas and diff, folded.

use std::collections::HashMap;

use chaperone_core::stats::{BoardStats, PEAK_OVER};
use chaperone_core::work::{BoardLane, LanePage};
use eframe::egui::{self, RichText};

use crate::areas::{self, Areas};
use crate::diff::{Diff, Files};
use crate::watch::{Detail, Median, Selected};
use crate::{board, detail, numbers, repo, theme};

/// What the viewer asked for on the page.
#[derive(Default)]
pub struct Asked {
    pub back: bool,
    /// The diff's Refresh.
    pub refresh: bool,
}

/// What the page reads, each for the selected lane only.
pub struct Reads<'a> {
    pub lane: Option<&'a BoardLane>,
    pub detail: Option<&'a Detail>,
    pub diff: Option<&'a Diff>,
    pub areas: &'a HashMap<i64, Areas>,
    pub stats: Option<&'a BoardStats>,
    pub median: Option<&'a Median>,
}

pub fn draw(ui: &mut egui::Ui, sel: &Selected, r: &Reads, now: i64) -> Asked {
    let mut asked = Asked::default();
    // Each lane keeps its own scroll offset.
    let salt = ("lane", sel.id);
    egui::ScrollArea::vertical().id_salt(salt).show(ui, |ui| {
        asked.back = ui.button("< Back").clicked();
        let page = match r.detail.map(|d| &d.read) {
            Some(Ok(p)) => Some(p),
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                None
            }
            None => None,
        };
        let Some(lane) = r.lane else {
            ui.label("Reading the lane...");
            return;
        };
        header(ui, sel, lane, page, r, now);
        let Some(page) = page else {
            if r.detail.is_none() {
                ui.label("Reading the lane...");
            }
            return;
        };
        tiles(ui, lane, page, r, now);
        card(ui, "Timeline", |ui| timeline(ui, page));
        card(ui, "Decisions", |ui| decisions(ui, page));
        card(ui, "Check by hand", |ui| check_by_hand(ui, page));
        card(ui, "Files", |ui| {
            files(ui, r.diff.and_then(|d| d.files.as_ref()))
        });
        let d = &page.detail;
        ui.collapsing("Handoffs", |ui| detail::handoffs(ui, d));
        ui.collapsing("Review findings", |ui| detail::findings(ui, d));
        let declared = areas::declared(lane);
        let a = r.areas.get(&sel.id);
        ui.collapsing("Areas", |ui| detail::draw_areas(ui, declared, a));
        ui.collapsing("Diff against the base", |ui| {
            asked.refresh = ui.button("Refresh").clicked();
            detail::draw_diff(ui, r.diff);
        });
    });
    asked
}

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    theme::glass().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.strong(title);
        body(ui);
    });
}

fn header(
    ui: &mut egui::Ui,
    sel: &Selected,
    lane: &BoardLane,
    page: Option<&LanePage>,
    r: &Reads,
    now: i64,
) {
    let item = &lane.board.item;
    let row = repo::row(lane, r.stats, r.areas, now);
    ui.horizontal_wrapped(|ui| {
        ui.heading(&item.title);
        match row.why {
            Some(_) => ui.label(theme::alert(row.state)),
            None => ui.label(RichText::new(row.state).strong()),
        };
    });
    // The newest brief's goal, else the spec slice's title, once read.
    if let Some(p) = page {
        let goal = p
            .detail
            .handoffs
            .first()
            .and_then(|h| h.handoff.brief.as_ref())
            .and_then(|b| b.goal.trim().lines().next())
            .filter(|g| !g.is_empty());
        let slice = p.detail.slice.as_ref().and_then(|s| s.in_spec.as_ref());
        let slice = slice.map(|s| s.title.as_str());
        ui.label(goal.or(slice).unwrap_or("No goal recorded yet."));
    }
    ui.horizontal_wrapped(|ui| {
        detail::issue(ui, sel.key.as_deref());
        let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
        for (k, v) in [
            ("lane", row.label.clone()),
            ("agent", text(&item.agent)),
            ("branch", text(&item.branch)),
            ("PR", text(&item.pr_ref)),
            ("pipeline", text(&item.pipeline_id)),
            ("running", row.running.clone()),
        ] {
            ui.label(RichText::new(k).small().weak());
            ui.label(v);
        }
    });
}

/// One stat: a name, a value and a line under it.
fn tile(ui: &mut egui::Ui, name: &str, value: &str, under: &str) {
    theme::glass().show(ui, |ui| {
        ui.set_width(200.0);
        ui.label(RichText::new(name).small().weak());
        ui.label(RichText::new(value).heading().strong());
        ui.label(RichText::new(under).small());
    });
}

fn tiles(ui: &mut egui::Ui, lane: &BoardLane, page: &LanePage, r: &Reads, now: i64) {
    let stats = numbers::lane(r.stats, lane.board.item.id);
    ui.horizontal_wrapped(|ui| {
        let (value, under) = time(lane, r.median, now);
        tile(ui, "Time running", &value, &under);
        let [io, cache] = numbers::tokens(stats);
        let under = match cache.as_str() {
            "-" | "..." => "cache share: -".into(),
            c => format!("{c} read from the cache"),
        };
        tile(ui, "Tokens in / out", &io, &under);
        let peak = numbers::peak(stats);
        let under = match stats.and_then(|s| s["peak_context"].as_i64()) {
            Some(p) => format!(
                "{:.0}% of {}k",
                p as f64 / PEAK_OVER as f64 * 100.0,
                PEAK_OVER / 1000
            ),
            None => format!("of {}k", PEAK_OVER / 1000),
        };
        tile(ui, "Peak context", &peak, &under);
        let (value, under) = review(page);
        tile(ui, "Review", &value, &under);
        let attention = numbers::attention(stats);
        let open = match lane.open_questions {
            0 => "no open question".to_string(),
            n => format!("{n} open"),
        };
        tile(ui, "People needed", &attention, &open);
        let (value, under) = gates(page);
        tile(ui, "Gates", &value, &under);
    });
}

/// Time running, and the repo's median claim to merge over 30 days.
fn time(lane: &BoardLane, median: Option<&Median>, now: i64) -> (String, String) {
    let Some(claimed) = lane.claimed_secs else {
        return ("not started".into(), String::new());
    };
    let value = board::age(claimed, now);
    let Some(repo) = lane.board.item.repo_slug.as_deref() else {
        return (value, "no repo to compare with".into());
    };
    let under = match median.filter(|m| m.repo == repo).map(|m| &m.read) {
        None => "repo median: ...".into(),
        Some(Err(e)) => format!("repo median not read: {e}"),
        Some(Ok(v)) => {
            let lanes = v["lanes"].as_i64().unwrap_or(0);
            let few = if v["too_few_lanes"] == true {
                ", too few to score"
            } else {
                ""
            };
            match v["claim_to_merge_h_median"].as_f64() {
                Some(h) => {
                    format!("vs repo median {h:.1}h ({lanes} merged in 30 days{few})")
                }
                None => "repo median: no merges in 30 days".into(),
            }
        }
    };
    (value, under)
}

/// The newest scores, the engine's rounds and whether its review head is
/// the newest handoff's commit (what `head_eq` compares).
fn review(page: &LanePage) -> (String, String) {
    let scores: Vec<String> = page
        .reviews
        .iter()
        .filter(|r| r.outcome == "ran")
        .filter_map(|r| r.score)
        .take(3)
        .map(|s| format!("{s}"))
        .collect();
    let value = match scores.is_empty() {
        true => "no score".into(),
        // The rows are newest first: shown oldest first, the newest last.
        false => scores.into_iter().rev().collect::<Vec<_>>().join(", "),
    };
    let head = page
        .detail
        .handoffs
        .first()
        .and_then(|h| h.handoff.commit.as_deref());
    let on = match (page.review_head.as_deref(), head) {
        (None, _) => "no local review".into(),
        (Some(_), None) => "no handoff commit".into(),
        (Some(r), Some(h)) if r == h => "on head".into(),
        (Some(r), Some(_)) => format!("not on head (reviewed {})", board::short(r)),
    };
    let rounds = match page.review_rounds {
        1 => "1 round".to_string(),
        n => format!("{n} rounds"),
    };
    (value, format!("{rounds}, {on}"))
}

fn gates(page: &LanePage) -> (String, String) {
    let Some(g) = &page.gates else {
        return (
            "-".into(),
            "no pipeline document for the lane's stamp".into(),
        );
    };
    let passed = g.gates.iter().filter(|g| g.passed).count();
    let value = format!("{passed} of {}", g.gates.len());
    let failing: Vec<&str> = g
        .gates
        .iter()
        .filter(|g| !g.passed)
        .map(|g| g.gate.as_str())
        .collect();
    let to = format!("{:?}", g.to).to_lowercase();
    let under = match failing.is_empty() {
        true => format!("all pass to {to}"),
        false => format!("to {to}, not yet: {}", failing.join(", ")),
    };
    (value, under)
}

fn timeline(ui: &mut egui::Ui, page: &LanePage) {
    if page.timeline.is_empty() {
        ui.label("No events.");
        return;
    }
    detail::capped(ui, page.timeline.len());
    egui::Grid::new("timeline").striped(true).show(ui, |ui| {
        for t in &page.timeline {
            ui.label(RichText::new(&t.at).small().weak());
            ui.label(t.minutes.map_or("-".into(), |m| format!("+{m}m")));
            ui.label(&t.kind);
            ui.label(t.what.as_deref().unwrap_or(""));
            ui.label(RichText::new(t.agent.as_deref().unwrap_or("")).small());
            ui.end_row();
        }
    });
}

fn decisions(ui: &mut egui::Ui, page: &LanePage) {
    if page.decisions.is_empty() {
        ui.label("No decisions recorded.");
    }
    for d in &page.decisions {
        ui.horizontal_wrapped(|ui| {
            // A spec line's who and when are in its text.
            let who = d.by.as_deref().unwrap_or("spec");
            ui.label(RichText::new(who).small().weak());
            ui.label(&d.text);
        });
    }
    // The dossier keeps the spec's newest 20.
    let slice = page.detail.slice.as_ref();
    if let Some(n) = slice.and_then(|s| s.omitted.get("slice_decisions")) {
        ui.label(format!("Left out: the {n} oldest decisions."));
    }
}

fn check_by_hand(ui: &mut egui::Ui, page: &LanePage) {
    let Some(h) = page.detail.handoffs.first() else {
        ui.label("No handoff yet.");
        return;
    };
    let items = h.handoff.brief.as_ref().map(|b| &b.check_by_hand);
    match items.filter(|i| !i.is_empty()) {
        None => {
            ui.label("Nothing named in the newest handoff.");
        }
        Some(items) => {
            for i in items {
                ui.label(format!("- {i}"));
            }
        }
    }
}

fn files(ui: &mut egui::Ui, files: Option<&Result<Files, String>>) {
    let f = match files {
        None => {
            ui.label("Reading the files...");
            return;
        }
        Some(Err(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
        Some(Ok(f)) => f,
    };
    if let Some(why) = &f.no_areas {
        ui.colored_label(ui.visuals().warn_fg_color, format!("No areas: {why}"));
    }
    if f.files.is_empty() {
        ui.label(format!("No changes against {}.", f.base));
        return;
    }
    ui.label(format!(
        "Against {}: committed, uncommitted and untracked.",
        f.base
    ));
    let n = |v: Option<u64>| v.map_or("-".into(), |v| v.to_string());
    egui::Grid::new("files").striped(true).show(ui, |ui| {
        for c in ["file", "area", "+", "-"] {
            ui.label(RichText::new(c).small().weak());
        }
        ui.end_row();
        for c in &f.files {
            let path = match c.untracked {
                true => format!("{} (new, untracked)", c.path),
                false => c.path.clone(),
            };
            ui.monospace(path);
            let area = match (&c.area, &f.no_areas) {
                (Some(a), _) => a.as_str(),
                (None, None) => "in no area",
                (None, Some(_)) => "-",
            };
            ui.label(area);
            ui.label(n(c.added));
            ui.label(n(c.removed));
            ui.end_row();
        }
    });
}
