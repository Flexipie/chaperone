//! ⌘K: GPUI Kit's command palette in a dialog. Go (all repos, Pipeline,
//! with their keys), every repo on the board and every open lane, needs-you
//! first. The dialog layer is not under the cockpit, so an item's action
//! would not reach it: a confirm navigates by the item's place, from the
//! targets built with the items.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, Entity, ParentElement, Window, px};
use gpui_component::WindowExt;
use gpui_component::command::{Command, CommandGroup, CommandItem};

use crate::{Cockpit, GoAll, GoPipeline, View, board, lanes};

pub fn open(cockpit: &Entity<Cockpit>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let cockpit = cockpit.clone();
    let focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            let state = cockpit.read(cx).palette.clone();
            state.update(cx, |s, cx| s.focus(window, cx));
        }
        let (command, targets) = items(cockpit.read(cx));
        let to = cockpit.clone();
        let command = command
            .placeholder("Go to a repo, a lane or a page...")
            .max_h(px(420.0))
            .on_confirm(move |at, window, cx| {
                let Some(view) = targets.get(at.section).and_then(|g| g.get(at.row)) else {
                    return;
                };
                let view = view.clone();
                to.update(cx, |c, cx| c.go(view, cx));
                window.close_dialog(cx);
            });
        dialog.w(px(560.0)).close_button(false).child(command)
    });
}

/// The palette's groups from the board as it is now, and each item's view
/// by its (group, row): only groups with items, so the places match.
fn items(c: &Cockpit) -> (Command, Rc<Vec<Vec<View>>>) {
    let now = board::now();
    let mut command = Command::new(&c.palette);
    let mut targets = Vec::new();
    command = command.group(
        CommandGroup::new().label("Go").items([
            CommandItem::new()
                .label("All repos")
                .action(Box::new(GoAll)),
            CommandItem::new()
                .label("Pipeline")
                .action(Box::new(GoPipeline)),
        ]),
    );
    targets.push(vec![View::All, View::Pipeline]);
    let Some(b) = &c.board else {
        return (command, Rc::new(targets));
    };
    let repos = board::group(b);
    if !repos.is_empty() {
        command = command.group(
            CommandGroup::new().label("Repos").items(
                repos
                    .iter()
                    .map(|g| CommandItem::new().label(g.slug.to_string())),
            ),
        );
        targets.push(
            repos
                .iter()
                .map(|g| View::Repo(g.slug.to_lowercase()))
                .collect(),
        );
    }
    let mut open: Vec<_> = b.lanes.iter().map(|l| (l, lanes::state(l, now))).collect();
    open.sort_by_key(|(_, (_, needs_you))| !needs_you);
    if !open.is_empty() {
        command = command.group(CommandGroup::new().label("Lanes").items(open.iter().map(
            |(l, (state, _))| {
                let item = &l.board.item;
                CommandItem::new()
                    .label(format!("{}  {}  ({state})", board::label(item), item.title))
                    .keywords([board::slug(item.repo_slug.as_deref()).to_string()])
            },
        )));
        targets.push(
            open.iter()
                .map(|(l, _)| View::Lane(l.board.item.id))
                .collect(),
        );
    }
    (command, Rc::new(targets))
}
