//! chaperone-ui-gpui: the spike (#134) of chaperone-ui's board in GPUI and
//! GPUI Kit. It opens the ledger read-only and redraws when it changes. A
//! sidebar picks all repos or one, each with how many lanes need you; the
//! main view lists the lanes it shows, or one lane's page. ⌘K opens a
//! command palette (a repo, a lane, Pipeline); a lane's Merge opens a
//! confirm that writes nothing. The Pipeline view is empty here.

mod lanes;
mod page;
mod palette;
mod theme;
mod watch;

// chaperone-ui's own helpers, compiled from its source so "needs you" is
// the same code, never a copy.
#[allow(dead_code)]
#[path = "../../../crates/chaperone-ui/src/board.rs"]
mod board;
#[path = "../../../crates/chaperone-ui/src/needs.rs"]
mod needs;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chaperone_core::git::{git_bin, lane_files};
use chaperone_core::work::{BoardLane, LaneBoard};
use clap::Parser;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, px, size,
};
use gpui_component::WindowExt;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::command::CommandState;
use gpui_component::sidebar::{Sidebar, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem};
use gpui_component::table::{DataTable, TableState};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use theme::STEP;

/// Redraws with no ledger change, so ages and lapsed leases move.
const AGE_TICK: Duration = Duration::from_secs(30);

/// Copied from crates/chaperone-ui/src/diff.rs `DEADLINE` and `OUT_BYTES`.
const FILES_DEADLINE: Duration = Duration::from_secs(5);
const FILES_BYTES: usize = 1024 * 1024;

actions!(cockpit, [OpenPalette, GoAll, GoPipeline]);

#[derive(Parser)]
#[command(about)]
struct Args {
    /// Path to the ledger. Required: the spike never guesses the real one.
    #[arg(long)]
    db: PathBuf,
    /// Quit after the first frame that shows the board (R14 U1)
    #[arg(long, hide = true)]
    first_frame: bool,
    /// Print `notify <data_version>` when a new board reaches the view, as
    /// it asks for a redraw: where egui's U2 stops (R14 U2)
    #[arg(long, hide = true)]
    trace: bool,
    /// Start on this lane's page (screenshots and U4 without a click)
    #[arg(long, hide = true)]
    lane: Option<i64>,
    /// Open the command palette once the board shows
    #[arg(long, hide = true)]
    palette: bool,
    /// Open the lane's Merge confirm once the board shows (with --lane)
    #[arg(long, hide = true)]
    dialog: bool,
}

/// What the main view shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    All,
    /// One repo, by its slug lowercased (repos match in any case).
    Repo(String),
    /// One lane's page, by work item id.
    Lane(i64),
    Pipeline,
}

impl View {
    /// Whether a repo slug, any case, is in this view.
    pub fn shows(&self, slug: &str) -> bool {
        match self {
            View::All => true,
            View::Repo(r) => slug.eq_ignore_ascii_case(r),
            View::Lane(_) | View::Pipeline => false,
        }
    }
}

fn main() {
    let args = Args::parse();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(&theme::DARK, cx);
        // No key context, so the palette finds them for its hints.
        cx.bind_keys([
            KeyBinding::new("cmd-k", OpenPalette, None),
            KeyBinding::new("cmd-1", GoAll, None),
            KeyBinding::new("cmd-2", GoPipeline, None),
        ]);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1200.0), px(760.0)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Chaperone".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Cockpit::new(args, window, cx))
        })
        .expect("open the window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}

struct Cockpit {
    view: View,
    board: Option<LaneBoard>,
    error: Option<String>,
    /// The time the last reload used, so badges and rows agree.
    now: i64,
    table: Entity<TableState<lanes::Lanes>>,
    first_frame: bool,
    focus: FocusHandle,
    palette: Entity<CommandState>,
    /// The lane the watcher reads a page for, set by `reload`.
    wanted: watch::Wanted,
    /// The shown lane's page, stats row and repo median as last read.
    page: Option<watch::Page>,
    median: Option<(String, Result<serde_json::Value, String>)>,
    /// The shown lane's changed files, read once when it opens.
    files: Option<page::Files>,
    _files: Option<Task<()>>,
    /// Opened once the first board shows (--palette, --dialog).
    open_palette: bool,
    open_dialog: bool,
    _watch: Task<()>,
    _tick: Task<()>,
}

impl Cockpit {
    fn new(args: Args, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            TableState::new(lanes::Lanes::new(), window, cx)
                .col_movable(false)
                .sortable(false)
        });
        let (path, trace) = (args.db, args.trace);
        let wanted = watch::Wanted::default();
        let want = wanted.clone();
        let watch = cx.spawn(async move |this, cx| {
            watch::run(
                path,
                this,
                cx,
                want,
                move |c: &mut Cockpit, read, cx| {
                    match read {
                        Ok(board) => {
                            if trace {
                                println!("notify {}", board.data_version);
                            }
                            c.board = Some(board);
                            c.error = None;
                        }
                        Err(e) => c.error = Some(e),
                    }
                    c.reload(cx);
                },
                // Kept for any lane, as the watcher keys its next read on it;
                // the page draws only the shown lane's.
                |c: &mut Cockpit, mut page, cx| {
                    if let Some(m) = page.median.take() {
                        c.median = Some(m);
                    }
                    c.page = Some(page);
                    cx.notify();
                },
            )
            .await
        });
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGE_TICK).await;
                if this.update(cx, |c, cx| c.reload(cx)).is_err() {
                    return;
                }
            }
        });
        Self {
            view: args.lane.map_or(View::All, View::Lane),
            board: None,
            error: None,
            now: board::now(),
            table,
            first_frame: args.first_frame,
            focus: cx.focus_handle(),
            palette: cx.new(|cx| CommandState::new(window, cx)),
            wanted,
            page: None,
            median: None,
            files: None,
            _files: None,
            open_palette: args.palette,
            open_dialog: args.dialog,
            _watch: watch,
            _tick: tick,
        }
    }

    /// Rebuilds the table's rows for the view at the current time.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.now = board::now();
        // A repo whose lanes all closed has no entry: back to All.
        if let (View::Repo(r), Some(b)) = (&self.view, &self.board)
            && !board::group(b)
                .iter()
                .any(|g| g.slug.eq_ignore_ascii_case(r))
        {
            self.view = View::All;
        }
        self.read_files(cx);
        *self.wanted.lock().unwrap() = match self.view {
            View::Lane(id) => self.lane(id).map(|l| watch::Want {
                id,
                key: l.board.item.source_key.clone(),
                repo: l.board.item.repo_slug.clone(),
            }),
            _ => None,
        };
        let rows = self
            .board
            .as_ref()
            .map(|b| lanes::rows(b, &self.view, self.now))
            .unwrap_or_default();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().rows = rows;
            t.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn go(&mut self, to: View, cx: &mut Context<Self>) {
        if self.view != to {
            self.files = None;
            self._files = None;
        }
        self.view = to;
        self.reload(cx);
    }

    /// The open lane `id` on the board.
    fn lane(&self, id: i64) -> Option<&BoardLane> {
        let lanes = &self.board.as_ref()?.lanes;
        lanes.iter().find(|l| l.board.item.id == id)
    }

    /// Reads the shown lane's files once, off the main thread, when the
    /// board has it and they are not read or being read.
    fn read_files(&mut self, cx: &mut Context<Self>) {
        let View::Lane(id) = self.view else { return };
        if self.files.is_some() || self._files.is_some() {
            return;
        }
        let Some(item) = self.lane(id).map(|l| &l.board.item) else {
            return;
        };
        let wt = item.worktree_path.clone();
        let base = item.base_sha.clone();
        let read = cx.background_executor().spawn(async move {
            let deadline = Instant::now() + FILES_DEADLINE;
            let wt = wt.map(PathBuf::from).filter(|w| w.is_dir());
            let wt = wt.ok_or_else(|| "no worktree to read".to_string())?;
            let bin = git_bin(deadline);
            lane_files(&bin, &wt, base.as_deref(), deadline, FILES_BYTES, |_| None)
        });
        self._files = Some(cx.spawn(async move |this, cx| {
            let files: page::Files = read.await;
            let _ = this.update(cx, |c, cx| {
                c.files = Some(files);
                cx.notify();
            });
        }));
    }

    /// The Merge confirm: the spike writes nothing, so both buttons close.
    fn confirm(&mut self, label: String, window: &mut Window, cx: &mut Context<Self>) {
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .confirm()
                .title(format!("Merge {label}?"))
                .description(
                    "Squash-merges the PR at the reviewed head. In this spike, Merge \
                     writes nothing and closes.",
                )
                .ok_text("Merge")
                .on_ok(|_, _, _| true)
        });
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now;
        let groups = self.board.as_ref().map(board::group).unwrap_or_default();
        let all = self
            .board
            .as_ref()
            .map_or(0, |b| needs::count(&b.lanes, now));
        let entry = |to: View, name: SharedString, n: usize| {
            let active = self.view == to;
            SidebarMenuItem::new(name)
                .active(active)
                .suffix(move |_, _| badge(n))
                .on_click(cx.listener(move |c, _, _, cx| c.go(to.clone(), cx)))
        };
        let top = SidebarMenu::new()
            .child(entry(View::All, "All repos".into(), all))
            .child(entry(View::Pipeline, "Pipeline".into(), 0));
        let repos = SidebarMenu::new().children(groups.iter().map(|g| {
            let n = needs::count(g.specs.iter().flat_map(|s| s.lanes.iter().copied()), now);
            entry(
                View::Repo(g.slug.to_lowercase()),
                g.slug.to_string().into(),
                n,
            )
        }));
        Sidebar::new("nav")
            .header(SidebarHeader::new().child("Chaperone"))
            .child(SidebarGroup::new("Board").child(top))
            .child(SidebarGroup::new("Repos").child(repos))
    }

    fn main_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match &self.view {
            View::All => "All repos".to_string(),
            View::Repo(r) => self
                .board
                .as_ref()
                .and_then(|b| {
                    let groups = board::group(b);
                    let g = groups.iter().find(|g| g.slug.eq_ignore_ascii_case(r))?;
                    Some(g.slug.to_string())
                })
                .unwrap_or_else(|| r.clone()),
            View::Lane(id) => self
                .lane(*id)
                .map_or(format!("Lane #{id}"), |l| board::label(&l.board.item)),
            View::Pipeline => "Pipeline".to_string(),
        };
        let body = match (&self.view, &self.board) {
            (View::Lane(id), Some(_)) => {
                let id = *id;
                let lane = self.lane(id);
                let label = lane
                    .map(|l| board::label(&l.board.item))
                    .unwrap_or_default();
                let merge = Button::new("merge").label("Merge").primary().on_click(
                    cx.listener(move |c, _, window, cx| c.confirm(label.clone(), window, cx)),
                );
                let reads = page::Reads {
                    id,
                    lane,
                    page: self.page.as_ref().filter(|p| p.id == id),
                    median: self.median.as_ref(),
                    files: self.files.as_ref(),
                };
                div()
                    .id("lane-page")
                    .size_full()
                    .overflow_y_scroll()
                    .child(page::draw(&reads, merge, board::now(), cx))
                    .into_any_element()
            }
            (View::Pipeline, _) => div()
                .text_color(cx.theme().muted_foreground)
                .child("Not in this spike slice.")
                .into_any_element(),
            (_, None) => div()
                .text_color(cx.theme().muted_foreground)
                .child("Reading the ledger...")
                .into_any_element(),
            (_, Some(_)) => DataTable::new(&self.table).into_any_element(),
        };
        v_flex()
            .size_full()
            .gap(px(STEP * 3.0))
            .p(px(STEP * 5.0))
            .child(div().text_xl().child(title))
            .children(
                self.error
                    .as_ref()
                    .map(|e| div().text_color(cx.theme().danger).child(e.clone())),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

/// The needs-you count pill; nothing for zero.
fn badge(n: usize) -> impl IntoElement {
    div().when(n > 0, |d| {
        d.px(px(STEP * 1.5))
            .rounded(px(STEP * 2.0))
            .bg(theme::alert().opacity(0.2))
            .text_color(theme::alert())
            .text_xs()
            .child(n.to_string())
    })
}

impl Render for Cockpit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.board.is_some() {
            if self.first_frame {
                self.first_frame = false;
                cx.on_next_frame(window, |_, _, cx| cx.quit());
            }
            if std::mem::take(&mut self.open_palette) {
                let this = cx.entity();
                window.defer(cx, move |window, cx| palette::open(&this, window, cx));
            }
            if std::mem::take(&mut self.open_dialog)
                && let View::Lane(id) = self.view
            {
                let label = self.lane(id).map(|l| board::label(&l.board.item));
                let this = cx.entity();
                window.defer(cx, move |window, cx| {
                    this.update(cx, |c, cx| c.confirm(label.unwrap_or_default(), window, cx))
                });
            }
        }
        if !window.has_active_dialog(cx) && window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        h_flex()
            .id("cockpit")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|_, _: &OpenPalette, window, cx| {
                    palette::open(&cx.entity(), window, cx)
                }),
            )
            .on_action(cx.listener(|c, _: &GoAll, _, cx| c.go(View::All, cx)))
            .on_action(cx.listener(|c, _: &GoPipeline, _, cx| c.go(View::Pipeline, cx)))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.sidebar(cx))
            .child(self.main_view(cx))
    }
}
