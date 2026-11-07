//! chaperone-ui: a read-only live board of every lane. It opens the ledger
//! read-only and redraws when the ledger changes; herdr stays where you type.

mod board;
mod watch;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chaperone_core::work::{BoardLane, LaneBoard};
use clap::Parser;
use eframe::egui;

use watch::Shown;

/// Repaints with no ledger change, so "updated" ages move.
const AGE_TICK: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Path to the ledger (defaults to chaperone/ledger.db in the user data dir)
    #[arg(long)]
    db: Option<PathBuf>,
    /// Close after the first frame that shows the board or an error (R14
    /// startup measurement)
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
        let shown = self.shown.lock().unwrap();
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(error) = &shown.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            match &shown.board {
                Some(board) => draw(ui, board, now),
                None => {
                    ui.label("Reading the ledger...");
                }
            }
        });
        if self.first_frame && (shown.board.is_some() || shown.error.is_some()) {
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

fn draw(ui: &mut egui::Ui, board: &LaneBoard, now: i64) {
    if board.lanes.is_empty() {
        ui.label("No open lanes.");
        return;
    }
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
                            for cell in cells(lane, now) {
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
        (Some(v), Some(sha)) => format!("{v} @ {}", &sha[..sha.len().min(7)]),
        (v, _) => text(v),
    };
    [
        lane_pos,
        text(&item.source_key),
        item.title.clone(),
        text(&item.branch),
        text(&item.agent),
        item.status.clone(),
        board::age(&item.updated_at, now),
        text(&item.pr_ref),
        // No ledger record carries CI yet (ci_snapshot is reserved).
        "not recorded".into(),
        text(&s.plan_check),
        text(&s.quality_gate),
        verdict,
    ]
}
