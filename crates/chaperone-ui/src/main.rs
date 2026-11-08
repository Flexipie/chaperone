//! chaperone-ui: a read-only live board of every lane. It opens the ledger
//! read-only and redraws when the ledger changes; herdr stays where you type.
//! Clicking a lane shows its detail and its worktree's diff.

mod board;
mod detail;
mod diff;
mod watch;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
            watch::spawn(path, shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            diff::spawn(shown.clone(), move || ctx.request_repaint());
            Ok(Box::new(App {
                shown,
                first_frame: args.first_frame,
            }))
        }),
    )
}

struct App {
    shown: Arc<Mutex<Shown>>,
    first_frame: bool,
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
            egui::Panel::right("detail")
                .resizable(true)
                .default_size(560.0)
                .show(ui, |ui| refresh = detail::draw(ui, sel, lane, detail, diff));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(error) = &shown.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            match &shown.board {
                Some(board) => picked = draw(ui, board, shown.selected.as_ref(), now),
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

const COLUMNS: [&str; 12] = [
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
];

/// The board; returns the lane clicked, if any.
fn draw(
    ui: &mut egui::Ui,
    board: &LaneBoard,
    selected: Option<&Selected>,
    now: i64,
) -> Option<Selected> {
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
                            let [first, rest @ ..] = cells(lane, now);
                            let item = &lane.board.item;
                            let on = selected.is_some_and(|s| s.id == item.id);
                            if ui.selectable_label(on, first).clicked() {
                                picked = Some(Selected::of(item));
                            }
                            for cell in rest {
                                ui.label(cell);
                            }
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

/// One row, in [`COLUMNS`] order. "-" is no record.
fn cells(lane: &BoardLane, now: i64) -> [String; 12] {
    let item = &lane.board.item;
    let s = &lane.stamps;
    let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
    let lane_pos = match (&item.lane_key, item.lane_pos) {
        (Some(key), Some(pos)) => format!("{key} {pos}"),
        (Some(key), None) => key.clone(),
        _ => format!("#{}", item.id),
    };
    let verdict = match (&s.verdict, &s.verdict_sha) {
        (Some(v), Some(sha)) => format!("{v} @ {}", board::short(sha)),
        (v, _) => text(v),
    };
    [
        lane_pos,
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
    ]
}
