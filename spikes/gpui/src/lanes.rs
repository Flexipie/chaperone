//! The lane table: one row per open lane in plain words, needs-you first,
//! drawn by GPUI Kit's virtualized table (only the visible rows render).

use chaperone_core::work::{BoardLane, LaneBoard};
use gpui::{App, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use gpui_component::ActiveTheme;
use gpui_component::table::{Column, TableDelegate, TableState};

use crate::theme::{self, STEP};
use crate::{View, board, needs};

pub struct Row {
    label: SharedString,
    title: SharedString,
    state: SharedString,
    needs_you: bool,
    next: SharedString,
    running: SharedString,
}

pub struct Lanes {
    pub rows: Vec<Row>,
    columns: Vec<Column>,
}

impl Lanes {
    pub fn new() -> Self {
        let col = |key: &'static str, name: &'static str, w: f32| {
            Column::new(key, name).width(px(w)).movable(false)
        };
        Self {
            rows: Vec::new(),
            columns: vec![
                col("label", "Lane", 100.0),
                col("title", "Title", 260.0),
                col("state", "State", 120.0),
                col("next", "Next step", 300.0),
                col("running", "Running", 80.0),
            ],
        }
    }
}

impl TableDelegate for Lanes {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let row = &self.rows[row_ix];
        let cell = div().px(px(STEP)).truncate();
        match col_ix {
            0 => cell
                .text_color(cx.theme().muted_foreground)
                .child(row.label.clone()),
            1 => cell.child(row.title.clone()),
            2 if row.needs_you => cell.text_color(theme::alert()).child(row.state.clone()),
            2 => cell.child(row.state.clone()),
            3 => cell
                .text_color(cx.theme().muted_foreground)
                .child(row.next.clone()),
            _ => cell.child(row.running.clone()),
        }
    }
}

/// The open lanes `view` shows at `now`, needs-you first, each group in
/// board order. The words are chaperone-ui's `repo::row`.
pub fn rows(board: &LaneBoard, view: &View, now: i64) -> Vec<Row> {
    let mut rows: Vec<Row> = board
        .lanes
        .iter()
        .filter(|l| view.shows(board::slug(l.board.item.repo_slug.as_deref())))
        .map(|lane| {
            let item = &lane.board.item;
            let (state, needs_you) = state(lane, now);
            let blocker = item.blocker.as_deref().filter(|_| item.status == "blocked");
            Row {
                label: board::label(item).into(),
                title: item.title.clone().into(),
                state: state.into(),
                needs_you,
                next: blocker
                    .or(item.next_action.as_deref())
                    .unwrap_or("-")
                    .to_string()
                    .into(),
                running: lane
                    .claimed_secs
                    .map_or("not started".into(), |c| board::age(c, now))
                    .into(),
            }
        })
        .collect();
    rows.sort_by_key(|r| !r.needs_you);
    rows
}

/// The lane's state in plain words, and whether it needs you.
pub fn state(lane: &BoardLane, now: i64) -> (&'static str, bool) {
    let why = needs::why(lane, now);
    let state = match (why, lane.board.item.status.as_str()) {
        (Some(w), _) => w.words(),
        (None, "queued") => "waiting to start",
        (None, "claimed") => "starting",
        (None, "active") => "working",
        (None, "review") => "in review",
        (None, _) => "open",
    };
    (state, why.is_some())
}
