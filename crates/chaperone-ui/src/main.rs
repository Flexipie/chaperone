//! chaperone-ui: a read-only live board of every lane. It opens the ledger
//! read-only and redraws when the ledger changes; herdr stays where you type.
//! Clicking a lane shows its detail and its worktree's diff. Above it, the
//! R11 numbers of the lanes merged since a date. Each lane names the
//! lanes it shares files with (`chaperone overlap`).

mod areas;
mod board;
mod changes;
mod detail;
mod diff;
mod graph;
mod numbers;
mod overlap;
mod watch;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chaperone_core::stats::BoardStats;
use chaperone_core::work::{BoardLane, LaneBoard};
use clap::Parser;
use eframe::egui;

use watch::{Selected, Shown};

/// Repaints with no ledger change, so "updated" ages move.
const AGE_TICK: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Path to the ledger (defaults to chaperone/ledger.db in the user data dir)
    #[arg(long)]
    db: Option<PathBuf>,
    /// Close after the first frame that shows the board (R14 startup
    /// measurement)
    #[arg(long, hide = true)]
    first_frame: bool,
}

fn main() -> eframe::Result {
    let args = Args::parse();
    let Some(path) = args
        .db
        .or_else(|| dirs::data_dir().map(|d| d.join("chaperone").join("ledger.db")))
    else {
        eprintln!("no user data directory; pass --db");
        std::process::exit(2);
    };
    let shown = Arc::new(Mutex::new(Shown::default()));
    eframe::run_native(
        "Chaperone",
        eframe::NativeOptions::default(),
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            watch::spawn(path.clone(), shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            overlap::spawn(path, shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            diff::spawn(shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            areas::spawn(shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            changes::spawn(path, shown.clone(), move || ctx.request_repaint());
            Ok(Box::new(App {
                shown,
                first_frame: args.first_frame,
                since: numbers::Since::default(),
                graph: false,
            }))
        }),
    )
}

struct App {
    shown: Arc<Mutex<Shown>>,
    first_frame: bool,
    since: numbers::Since,
    /// The change graph in place of the board.
    graph: bool,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let mut shown = self.shown.lock().unwrap();
        // A claim or update since the click moves the worktree or base; a
        // lane done or cancelled since leaves the board, and its panel closes.
        let fresh = match (&shown.selected, &shown.board) {
            (Some(sel), Some(board)) => {
                let lane = board.lanes.iter().find(|l| l.board.item.id == sel.id);
                let now = lane.map(|l| Selected::of(&l.board.item));
                (now.as_ref() != Some(sel)).then_some(now)
            }
            _ => None,
        };
        if let Some(fresh) = fresh {
            shown.selected = fresh;
        }
        let mut picked = None;
        let mut refresh = false;
        let mut since = None;
        egui::Panel::top("cohort").resizable(true).show(ui, |ui| {
            since = numbers::cohort(ui, shown.stats.as_ref(), &mut self.since)
        });
        if since.is_some() {
            shown.since = since;
        }
        if let Some(board) = &shown.board {
            egui::Panel::bottom("feed")
                .resizable(true)
                .default_size(180.0)
                .show(ui, |ui| detail::feed(ui, board));
        }
        if let Some(sel) = &shown.selected {
            let lane = shown
                .board
                .as_ref()
                .and_then(|b| b.lanes.iter().find(|l| l.board.item.id == sel.id));
            let detail = shown.detail.as_ref().filter(|d| d.lane == sel.id);
            let diff = shown.diff.as_ref().filter(|d| d.lane == sel.id);
            let areas = shown.areas.get(&sel.id);
            egui::Panel::right("detail")
                .resizable(true)
                .default_size(560.0)
                .show(ui, |ui| {
                    refresh = detail::draw(ui, sel, lane, detail, diff, areas)
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(error) = &shown.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.graph, false, "Board");
                ui.selectable_value(&mut self.graph, true, "Change graph");
            });
            shown.graph_open = self.graph;
            if self.graph {
                match &shown.graph {
                    Some(Ok(repos)) => graph::draw(ui, repos),
                    Some(Err(e)) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    None => {
                        ui.label("Reading the change graph...");
                    }
                }
                return;
            }
            match &shown.board {
                Some(board) => picked = draw(ui, &shown, board, now),
                None => {
                    ui.label("Reading the ledger...");
                }
            }
        });
        if let Some(pick) = picked {
            // Clicking the selected lane again closes its detail.
            shown.selected = (shown.selected.as_ref() != Some(&pick)).then_some(pick);
        }
        shown.refresh += u64::from(refresh);
        if self.first_frame && shown.board.is_some() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint_after(AGE_TICK);
    }
}

const COLUMNS: [&str; 18] = [
    "lane",
    "issue",
    "title",
    "branch",
    "agent",
    "status",
    "updated",
    "PR",
    "CI",
    "plan check",
    "quality gate",
    "verdict",
    "areas",
    "tokens in / out",
    "cache",
    "peak context",
    "attention",
    "shares files with",
];

/// The board; returns the lane clicked, if any.
fn draw(ui: &mut egui::Ui, shown: &Shown, board: &LaneBoard, now: i64) -> Option<Selected> {
    let selected = shown.selected.as_ref();
    if board.lanes.is_empty() {
        ui.label("No open lanes.");
        return None;
    }
    let mut picked = None;
    egui::ScrollArea::both().show(ui, |ui| {
        for repo in board::group(board) {
            ui.heading(repo.slug);
            for spec in repo.specs {
                let title = match spec.spec {
                    Some(s) => format!("{} {}", s.key, s.title),
                    None => "No spec".into(),
                };
                ui.strong(title);
                egui::Grid::new((repo.slug, spec.spec.map(|s| s.id)))
                    .striped(true)
                    .show(ui, |ui| {
                        for c in COLUMNS {
                            ui.strong(c);
                        }
                        ui.end_row();
                        for lane in spec.lanes {
                            let areas = shown.areas.get(&lane.board.item.id);
                            let stats = shown.stats.as_ref();
                            let [first, rest @ ..] = cells(lane, areas, stats, now);
                            let item = &lane.board.item;
                            let on = selected.is_some_and(|s| s.id == item.id);
                            if ui.selectable_label(on, first).clicked() {
                                picked = Some(Selected::of(item));
                            }
                            for cell in rest {
                                ui.label(cell);
                            }
                            let overlap = shown.overlap.as_ref();
                            let (names, paths) = numbers::shares(item.id, overlap, board);
                            ui.label(names).on_hover_text(paths);
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
            }
            ui.separator();
        }
    });
    picked
}

/// One row, in [`COLUMNS`] order but the last (overlap, drawn with its
/// hover text). "-" is no record; "..." is stats not read yet.
fn cells(
    lane: &BoardLane,
    areas: Option<&areas::Areas>,
    stats: Option<&BoardStats>,
    now: i64,
) -> [String; 17] {
    let item = &lane.board.item;
    let s = &lane.stamps;
    let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
    let row = numbers::lane(stats, item.id);
    let [tokens, cache] = numbers::tokens(row);
    let verdict = match (&s.verdict, &s.verdict_sha) {
        (Some(v), Some(sha)) => format!("{v} @ {}", board::short(sha)),
        (v, _) => text(v),
    };
    [
        board::label(item),
        text(&item.source_key),
        item.title.clone(),
        text(&item.branch),
        text(&item.agent),
        item.status.clone(),
        board::age(lane.updated_secs, now),
        text(&item.pr_ref),
        // No ledger record carries CI yet (ci_snapshot is reserved).
        "not recorded".into(),
        text(&s.plan_check),
        text(&s.quality_gate),
        verdict,
        areas::card(areas::declared(lane), areas),
        tokens,
        cache,
        numbers::peak(row),
        numbers::attention(row),
    ]
}
