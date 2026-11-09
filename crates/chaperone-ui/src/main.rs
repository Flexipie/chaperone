//! chaperone-ui: a live board of every lane. It opens the ledger
//! read-only and redraws when the ledger changes; herdr stays where you type.
//! Clicking a lane opens its page: what it is, how it is doing against its
//! repo and what it changed. Above the board, the R11 numbers of the lanes
//! merged since a date. Each lane names the lanes it shares files with
//! (`chaperone overlap`). The change graph tab
//! shows what each lane's commits change and reach. A left nav picks all
//! repos, one repo or the pipeline, each with how many lanes need you. All
//! repos opens on a card per repo, a repo on its lanes in plain words and
//! what merged lately; the full board stays a tab away. A repo's Backlog
//! tab lists its open issues not in a lane, from daisy's GitHub mirror,
//! each ready or blocked. On a lane that needs you, the lane page answers
//! it, sends it back or opens it in herdr, through a separate writer: the
//! board's ledger stays read-only.

mod areas;
mod backlog;
mod board;
mod changes;
mod detail;
mod diff;
mod graph;
mod lane;
mod nav;
mod needs;
mod numbers;
mod overlap;
mod repo;
mod theme;
mod watch;
mod writer;

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
            theme::apply(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            watch::spawn(path.clone(), shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            overlap::spawn(path.clone(), shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            diff::spawn(shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            areas::spawn(shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            changes::spawn(path.clone(), shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            backlog::spawn(path.clone(), shown.clone(), move || ctx.request_repaint());
            let ctx = cc.egui_ctx.clone();
            let writer = writer::spawn(path, "herdr".into(), shown.clone(), move || {
                ctx.request_repaint()
            });
            Ok(Box::new(App {
                shown,
                first_frame: args.first_frame,
                since: numbers::Since::default(),
                tab: Tab::default(),
                view: nav::View::default(),
                typed: lane::Typed::default(),
                writer,
            }))
        }),
    )
}

struct App {
    shown: Arc<Mutex<Shown>>,
    first_frame: bool,
    since: numbers::Since,
    tab: Tab,
    view: nav::View,
    typed: lane::Typed,
    writer: std::sync::mpsc::Sender<writer::Act>,
}

/// What the main view shows of All repos or a repo.
#[derive(Clone, Copy, Default, PartialEq)]
enum Tab {
    /// The repo cards, or the repo's page.
    #[default]
    Page,
    Board,
    Graph,
    /// Only in a repo's view.
    Backlog,
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(theme::GROUND).to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let mut shown = self.shown.lock().unwrap();
        let clicked = egui::Panel::left("nav")
            .frame(theme::glass())
            .show_separator_line(false)
            .resizable(false)
            .exact_size(220.0)
            .show(ui, |ui| {
                nav::draw(
                    ui,
                    shown.board.as_ref(),
                    shown.stats.as_ref(),
                    now,
                    &mut self.view,
                )
            })
            .inner;
        // Any nav click leaves the lane page.
        if clicked {
            shown.selected = None;
        }
        // A claim or update since the click moves the worktree or base; a
        // lane done or cancelled since leaves the board, or one the view
        // does not show, and its page closes.
        let fresh = match (&shown.selected, &shown.board) {
            (Some(sel), Some(board)) => {
                let lane = board.lanes.iter().find(|l| l.board.item.id == sel.id);
                let now = lane
                    .filter(|l| {
                        let slug = l.board.item.repo_slug.as_deref();
                        self.view.shows(board::slug(slug))
                    })
                    .map(|l| Selected::of(&l.board.item));
                (now.as_ref() != Some(sel)).then_some(now)
            }
            _ => None,
        };
        if let Some(fresh) = fresh {
            shown.selected = fresh;
        }
        let mut picked = None;
        if let Some(sel) = shown.selected.clone() {
            let s = &*shown;
            let reads = lane::Reads {
                lane: s
                    .board
                    .as_ref()
                    .and_then(|b| b.lanes.iter().find(|l| l.board.item.id == sel.id)),
                detail: s.detail.as_ref().filter(|d| d.lane == sel.id),
                diff: s.diff.as_ref().filter(|d| d.lane == sel.id),
                areas: &s.areas,
                stats: s.stats.as_ref(),
                median: s.median.as_ref(),
                writing: s.writing,
                wrote: s.wrote.as_ref(),
            };
            let mut asked = lane::Asked::default();
            egui::CentralPanel::default()
                .frame(theme::ground())
                .show(ui, |ui| {
                    if let Some(error) = &s.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    asked = lane::draw(ui, &sel, &reads, &mut self.typed, now)
                });
            // The change graph is not read behind the page.
            shown.graph_open = false;
            shown.refresh += u64::from(asked.refresh);
            if asked.back {
                shown.selected = None;
            }
            if let Some(act) = asked.act {
                // The writer ending clears it; one gone sends nothing back.
                shown.wrote = None;
                shown.writing = self.writer.send(act).is_ok();
                if !shown.writing {
                    let said = Err("the writer stopped".into());
                    shown.wrote = Some(writer::Wrote { work: sel.id, said });
                }
            }
            ui.ctx().request_repaint_after(AGE_TICK);
            return;
        }
        let mut since = None;
        egui::Panel::top("cohort")
            .frame(theme::glass())
            .show_separator_line(false)
            .resizable(true)
            .show(ui, |ui| {
                since = numbers::cohort(ui, shown.stats.as_ref(), &mut self.since)
            });
        if since.is_some() {
            shown.since = since;
        }
        if let Some(board) = &shown.board {
            egui::Panel::bottom("feed")
                .frame(theme::glass())
                .show_separator_line(false)
                .resizable(true)
                .default_size(180.0)
                .show(ui, |ui| detail::feed(ui, board));
        }
        let mut to = None;
        let view = &self.view;
        egui::CentralPanel::default()
            .frame(theme::ground())
            .show(ui, |ui| {
                if let Some(error) = &shown.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                shown.graph_open = false;
                shown.backlog_for = None;
                if *view == nav::View::Pipeline {
                    ui.heading("Pipeline");
                    ui.label("The pipeline page is not built yet (#109, slice 11).");
                    return;
                }
                // The repo as the board spells it: daisy may match case.
                let repo = match view {
                    nav::View::Repo(r) => shown.board.as_ref().and_then(|b| {
                        let repos = board::group(b);
                        // Lanes with no repo have no mirror to read.
                        let slug = repos
                            .iter()
                            .find(|x| x.slug.eq_ignore_ascii_case(r) && x.slug != "(no repo)");
                        slug.map(|x| x.slug.to_string())
                    }),
                    _ => None,
                };
                if repo.is_none() && self.tab == Tab::Backlog {
                    self.tab = Tab::default();
                }
                ui.horizontal(|ui| {
                    let page = if *view == nav::View::All {
                        "Overview"
                    } else {
                        "Lanes"
                    };
                    ui.selectable_value(&mut self.tab, Tab::Page, page);
                    ui.selectable_value(&mut self.tab, Tab::Board, "Board");
                    ui.selectable_value(&mut self.tab, Tab::Graph, "Change graph");
                    if repo.is_some() {
                        ui.selectable_value(&mut self.tab, Tab::Backlog, "Backlog");
                    }
                });
                if let (Tab::Backlog, Some(slug)) = (self.tab, &repo) {
                    shown.backlog_for = Some(slug.clone());
                    backlog::draw(ui, shown.backlog.as_ref(), slug);
                    return;
                }
                shown.graph_open = self.tab == Tab::Graph;
                if self.tab == Tab::Graph {
                    match &shown.graph {
                        Some(Ok(repos)) => {
                            let repos: graph::Repos = repos
                                .iter()
                                .filter(|(s, _)| view.shows(s))
                                .cloned()
                                .collect();
                            graph::draw(ui, &repos)
                        }
                        Some(Err(e)) => {
                            ui.colored_label(ui.visuals().error_fg_color, e);
                        }
                        None => {
                            ui.label("Reading the change graph...");
                        }
                    }
                    return;
                }
                match (&shown.board, view) {
                    (Some(board), _) if self.tab == Tab::Board => {
                        picked = draw(ui, &shown, board, view, now)
                    }
                    (Some(board), nav::View::Repo(slug)) => {
                        let stats = shown.stats.as_ref();
                        let rows = repo::rows(board, slug, stats, &shown.areas, now);
                        let merged = stats.map(|s| (s, slug.as_str()));
                        picked = repo::page(ui, &rows, merged, now);
                    }
                    (Some(board), _) => to = repo::cards(ui, board, shown.stats.as_ref(), now),
                    (None, _) => {
                        ui.label("Reading the ledger...");
                    }
                }
            });
        if let Some(to) = to {
            self.view = to;
        }
        if let Some(pick) = picked {
            shown.selected = Some(pick);
        }
        if self.first_frame && shown.board.is_some() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint_after(AGE_TICK);
    }
}

const COLUMNS: [&str; 19] = [
    "lane",
    "issue",
    "title",
    "branch",
    "agent",
    "status",
    "needs you",
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
fn draw(
    ui: &mut egui::Ui,
    shown: &Shown,
    board: &LaneBoard,
    view: &nav::View,
    now: i64,
) -> Option<Selected> {
    let repos: Vec<_> = board::group(board)
        .into_iter()
        .filter(|r| view.shows(r.slug))
        .collect();
    if repos.is_empty() {
        ui.label("No open lanes.");
        return None;
    }
    let mut picked = None;
    egui::ScrollArea::both().show(ui, |ui| {
        for repo in repos {
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
                            if ui.selectable_label(false, first).clicked() {
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
) -> [String; 18] {
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
        needs::why(lane, now).map_or("-", needs::Why::words).into(),
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
