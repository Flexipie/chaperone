//! The repo page and the All repos cards (#109 slice 2): a repo's open
//! lanes one row each in plain words, needs-you first, then what merged
//! in the last week; All repos is one card per repo.

use std::collections::HashMap;

use chaperone_core::stats::{BoardStats, MergedLane};
use chaperone_core::work::{BoardLane, LaneBoard};
use eframe::egui::{self, RichText, Sense};

use crate::areas::{self, Areas};
use crate::nav::View;
use crate::watch::Selected;
use crate::{board, needs, numbers, theme};

/// A merged lane's sentence past this is cut.
const SENTENCE_CHARS: usize = 200;

/// One open lane on the repo page, in plain words.
pub struct RepoLaneRow<'a> {
    pub lane: &'a BoardLane,
    pub label: String,
    pub title: &'a str,
    pub why: Option<needs::Why>,
    pub state: &'static str,
    pub next: &'a str,
    pub running: String,
    pub tokens: String,
    pub in_scope: String,
}

/// The repos to show: those with open lanes, then those with only recent
/// merges, by slug in any case (the first spelling seen is shown).
pub fn repos<'a>(board: Option<&'a LaneBoard>, stats: Option<&'a BoardStats>) -> Vec<&'a str> {
    let mut out: Vec<&str> = board
        .map(board::group)
        .unwrap_or_default()
        .iter()
        .map(|r| r.slug)
        .collect();
    let merged = stats.iter().flat_map(|s| &s.merged);
    for slug in merged.map(|m| m.repo_slug.as_deref().unwrap_or("(no repo)")) {
        if !out.iter().any(|s| s.eq_ignore_ascii_case(slug)) {
            out.push(slug);
        }
    }
    out.sort_by_key(|s| s.to_lowercase());
    out
}

fn slug_of(lane: &BoardLane) -> &str {
    lane.board.item.repo_slug.as_deref().unwrap_or("(no repo)")
}

/// The board's lanes in `slug`, any case.
pub fn lanes_of<'a>(board: &'a LaneBoard, slug: &'a str) -> impl Iterator<Item = &'a BoardLane> {
    board
        .lanes
        .iter()
        .filter(move |l| slug_of(l).eq_ignore_ascii_case(slug))
}

fn merged_of<'a>(stats: &'a BoardStats, slug: &'a str) -> impl Iterator<Item = &'a MergedLane> {
    stats.merged.iter().filter(move |m| {
        let s = m.repo_slug.as_deref().unwrap_or("(no repo)");
        s.eq_ignore_ascii_case(slug)
    })
}

/// The repo's open lanes, needs-you first, each group in board order.
pub fn rows<'a>(
    board: &'a LaneBoard,
    slug: &'a str,
    stats: Option<&BoardStats>,
    areas: &HashMap<i64, Areas>,
    now: i64,
) -> Vec<RepoLaneRow<'a>> {
    let mut rows: Vec<RepoLaneRow> = lanes_of(board, slug)
        .map(|lane| {
            let item = &lane.board.item;
            let why = needs::why(lane, now);
            let state = match (why, item.status.as_str()) {
                (Some(w), _) => w.words(),
                (None, "queued") => "waiting to start",
                (None, "claimed") => "starting",
                (None, "active") => "working",
                (None, "review") => "in review",
                (None, _) => "open",
            };
            let blocker = item.blocker.as_deref().filter(|_| item.status == "blocked");
            let row = numbers::lane(stats, item.id);
            let [io, cache] = numbers::tokens(row);
            let tokens = match cache.as_str() {
                _ if row.is_some_and(|r| r["tokens"].is_null()) => "none recorded".into(),
                "-" | "..." => io,
                c => format!("{io}, {c} cached"),
            };
            RepoLaneRow {
                lane,
                label: board::label(item),
                title: &item.title,
                why,
                state,
                next: blocker.or(item.next_action.as_deref()).unwrap_or("-"),
                running: lane
                    .claimed_secs
                    .map_or("not started".into(), |c| board::age(c, now)),
                tokens,
                in_scope: areas::card(areas::declared(lane), areas.get(&item.id)),
            }
        })
        .collect();
    rows.sort_by_key(|r| r.why.is_none());
    rows
}

/// A merged lane in one sentence: its brief's goal, first line, else its
/// title (the ledger has no PR title).
fn sentence(m: &MergedLane) -> String {
    let text = m.goal.as_deref().and_then(|g| g.trim().lines().next());
    let text = text.unwrap_or(&m.title);
    match text.char_indices().nth(SENTENCE_CHARS) {
        Some((i, _)) => format!("{}...", &text[..i]),
        None => text.to_string(),
    }
}

/// Local midnight before `now`, both seconds since the epoch.
fn midnight(now: i64, utc_offset_secs: i64) -> i64 {
    now - (now + utc_offset_secs).rem_euclid(86_400)
}

/// The repo page; returns the lane clicked, if any.
pub fn page(
    ui: &mut egui::Ui,
    rows: &[RepoLaneRow],
    merged: Option<(&BoardStats, &str)>,
    selected: Option<&Selected>,
    now: i64,
) -> Option<Selected> {
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("repo").show(ui, |ui| {
        theme::glass().show(ui, |ui| {
            ui.strong("Lanes");
            if rows.is_empty() {
                ui.label("No open lanes.");
                return;
            }
            egui::Grid::new("repo lanes").striped(true).show(ui, |ui| {
                for c in [
                    "lane",
                    "title",
                    "state",
                    "next step",
                    "running",
                    "tokens",
                    "in scope",
                ] {
                    ui.label(RichText::new(c).small().weak());
                }
                ui.end_row();
                for r in rows {
                    let item = &r.lane.board.item;
                    let on = selected.is_some_and(|s| s.id == item.id);
                    if ui.selectable_label(on, &r.label).clicked() {
                        picked = Some(Selected::of(item));
                    }
                    ui.label(r.title);
                    match r.why {
                        Some(_) => ui.label(theme::alert(r.state)),
                        None => ui.label(r.state),
                    };
                    ui.label(r.next);
                    ui.label(&r.running);
                    ui.label(&r.tokens);
                    ui.label(&r.in_scope);
                    ui.end_row();
                }
            });
        });
        theme::glass().show(ui, |ui| {
            ui.strong("Recently merged");
            let Some((stats, slug)) = merged else {
                ui.label("Reading the stats...");
                return;
            };
            let mut any = false;
            for m in merged_of(stats, slug) {
                any = true;
                ui.horizontal_wrapped(|ui| {
                    let label = match (&m.lane_key, m.lane_pos) {
                        (Some(k), Some(p)) => format!("{k} {p}"),
                        (Some(k), None) => k.clone(),
                        _ => format!("#{}", m.id),
                    };
                    ui.label(RichText::new(label).small().weak());
                    let ago = board::age(m.merged_secs, now);
                    ui.label(format!("{ago} ago: {}", sentence(m)));
                });
            }
            if !any {
                ui.label("Nothing merged in the last 7 days.");
            }
        });
    });
    picked
}

/// One card per repo (open lanes started, needing you, merged since
/// local midnight); returns the repo clicked, if any.
pub fn cards(
    ui: &mut egui::Ui,
    board: &LaneBoard,
    stats: Option<&BoardStats>,
    now: i64,
) -> Option<View> {
    let mut to = None;
    egui::ScrollArea::vertical()
        .id_salt("cards")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for slug in repos(Some(board), stats) {
                    let running = lanes_of(board, slug)
                        .filter(|l| l.board.item.status != "queued")
                        .count();
                    let needs = needs::count(lanes_of(board, slug), now);
                    let today = stats.map_or("...".into(), |s| {
                        let from = midnight(now, s.utc_offset_secs);
                        let n = merged_of(s, slug).filter(|m| m.merged_secs >= from);
                        n.count().to_string()
                    });
                    let card = theme::glass().show(ui, |ui| {
                        ui.set_width(220.0);
                        ui.strong(slug);
                        ui.add_space(6.0);
                        let needs = needs.to_string();
                        let needs = match needs.as_str() {
                            "0" => RichText::new(needs),
                            n => theme::alert(n),
                        };
                        egui::Grid::new(("card", slug)).show(ui, |ui| {
                            ui.label("running");
                            ui.label(running.to_string());
                            ui.end_row();
                            ui.label("needs you");
                            ui.label(needs);
                            ui.end_row();
                            ui.label("merged today");
                            ui.label(today);
                            ui.end_row();
                        });
                    });
                    let click = ui.interact(
                        card.response.rect,
                        ui.scope_id().with(("card", slug)),
                        Sense::click(),
                    );
                    if click.clicked() {
                        to = Some(View::Repo(slug.to_lowercase()));
                    }
                }
            });
        });
    to
}
