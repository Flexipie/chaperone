//! chaperone-ui-gpui: the spike (#134) of chaperone-ui's board in GPUI and
//! GPUI Kit. It opens the ledger read-only and redraws when it changes. A
//! sidebar picks all repos or one, each with how many lanes need you; the
//! main view lists the lanes it shows. The lane and Pipeline views are
//! empty here: slice B fills the lane page.

mod lanes;
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
use std::time::Duration;

use chaperone_core::work::LaneBoard;
use clap::Parser;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_component::sidebar::{Sidebar, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem};
use gpui_component::table::{DataTable, TableState};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use theme::STEP;

/// Redraws with no ledger change, so ages and lapsed leases move.
const AGE_TICK: Duration = Duration::from_secs(30);

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
}

/// What the main view shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    All,
    /// One repo, by its slug lowercased (repos match in any case).
    Repo(String),
    /// One lane's page, by work item id (slice B).
    #[allow(dead_code)]
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
    table: Entity<TableState<lanes::Lanes>>,
    first_frame: bool,
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
        let watch = cx.spawn(async move |this, cx| {
            watch::run(path, this, cx, move |c: &mut Cockpit, read, cx| {
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
            })
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
            view: View::default(),
            board: None,
            error: None,
            table,
            first_frame: args.first_frame,
            _watch: watch,
            _tick: tick,
        }
    }

    /// Rebuilds the table's rows for the view at the current time.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let now = board::now();
        // A repo whose lanes all closed has no entry: back to All.
        if let (View::Repo(r), Some(b)) = (&self.view, &self.board)
            && !board::group(b)
                .iter()
                .any(|g| g.slug.eq_ignore_ascii_case(r))
        {
            self.view = View::All;
        }
        let rows = self
            .board
            .as_ref()
            .map(|b| lanes::rows(b, &self.view, now))
            .unwrap_or_default();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().rows = rows;
            t.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn go(&mut self, to: View, cx: &mut Context<Self>) {
        self.view = to;
        self.reload(cx);
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = board::now();
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
            View::Lane(id) => format!("Lane #{id}"),
            View::Pipeline => "Pipeline".to_string(),
        };
        let body = match (&self.view, &self.board) {
            (View::Lane(_) | View::Pipeline, _) => div()
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
        if self.first_frame && self.board.is_some() {
            self.first_frame = false;
            cx.on_next_frame(window, |_, _, cx| cx.quit());
        }
        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.sidebar(cx))
            .child(self.main_view(cx))
    }
}
