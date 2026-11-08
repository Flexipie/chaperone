//! The lane page's lists from the #41 panel (issue, handoffs, findings,
//! areas, diff) and the board's feed of decisions and questions.

use chaperone_core::work::{DETAIL_ROWS, LaneBoard, LaneDetail};
use eframe::egui::{self, Color32, RichText, TextStyle};

use crate::areas::{self, State as AreasState};
use crate::board;
use crate::diff::{Diff, OUT_BYTES, State};

/// Lines of diff shown before the page scrolls them.
const DIFF_HEIGHT: f32 = 420.0;

/// The key, linked when it names a GitHub issue. The body stays in the
/// tracker (ADR 0007); a Linear key has no URL in the ledger.
pub fn issue(ui: &mut egui::Ui, key: Option<&str>) {
    let Some(key) = key else {
        ui.label("No tracker key.");
        return;
    };
    match key.split_once('#') {
        Some((repo, n)) if repo.contains('/') => {
            ui.hyperlink_to(key, format!("https://github.com/{repo}/issues/{n}"));
        }
        _ => {
            ui.label(key);
        }
    }
}

pub fn handoffs(ui: &mut egui::Ui, d: &LaneDetail) {
    if d.handoffs.is_empty() {
        ui.label("No handoffs.");
    }
    capped(ui, d.handoffs.len());
    for h in &d.handoffs {
        let (at, h) = (&h.created_at, &h.handoff);
        let ready = match h.ready {
            Some(true) => " ready",
            Some(false) => " not ready",
            None => "",
        };
        let status = h.status.as_deref().unwrap_or("-");
        ui.strong(format!("{at} {status}{ready}"));
        ui.label(&h.summary);
        for (name, v) in [
            ("commit", &h.commit),
            ("PR", &h.pull_request),
            ("next", &h.next_action),
        ] {
            if let Some(v) = v {
                ui.label(format!("{name}: {v}"));
            }
        }
    }
}

/// Says so when a list stopped at the read's cap.
pub fn capped(ui: &mut egui::Ui, n: usize) {
    if n as i64 == DETAIL_ROWS {
        ui.label(format!("The newest {DETAIL_ROWS} shown."));
    }
}

pub fn findings(ui: &mut egui::Ui, d: &LaneDetail) {
    if d.findings.is_empty() {
        ui.label("No review findings recorded.");
        return;
    }
    capped(ui, d.findings.len());
    for f in &d.findings {
        let path = f.path.as_deref().map_or(String::new(), |p| format!(" {p}"));
        let head = f.head_sha.as_deref().map_or("-", board::short);
        ui.label(format!(
            "{} @ {head} {} [{}]{path}: {}",
            f.check, f.severity, f.status, f.title
        ));
    }
}

/// The newest handoff's declared areas against the diff's; never an empty
/// section that would read as in scope.
pub fn draw_areas(ui: &mut egui::Ui, declared: Option<&[String]>, areas: Option<&areas::Areas>) {
    let Some(declared) = declared else {
        ui.label("Not declared: the newest handoff names no areas.");
        return;
    };
    ui.label(format!("Declared: {}", declared.join(", ")));
    let a = match areas.filter(|a| a.is_for(declared)).map(|a| &a.state) {
        None | Some(AreasState::Running) => {
            ui.label("Comparing with the diff...");
            return;
        }
        Some(AreasState::Failed(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, format!("Not checked: {e}"));
            return;
        }
        Some(AreasState::Done(a)) => a,
    };
    ui.label("Against the base: committed, uncommitted and untracked files, read again on a commit or a new handoff.");
    let warn = ui.visuals().warn_fg_color;
    for u in &a.undeclared {
        let more = match u.files_omitted {
            0 => String::new(),
            n => format!(" and {n} more"),
        };
        ui.colored_label(
            warn,
            format!(
                "Changed, not declared: {} ({}{more})",
                u.area,
                u.files.join(", ")
            ),
        );
    }
    if !a.unknown.is_empty() {
        ui.colored_label(
            warn,
            format!("Not in the profile: {}", a.unknown.join(", ")),
        );
    }
    if !a.unmapped.is_empty() {
        let more = match a.unmapped_omitted {
            0 => String::new(),
            n => format!(" and {n} more"),
        };
        ui.colored_label(warn, format!("In no area: {}{more}", a.unmapped.join(", ")));
    }
    for (what, list) in [
        ("Changed", &a.changed),
        ("Reached", &a.reached),
        ("Reached, not declared or changed", &a.reached_undeclared),
        ("Declared, not changed or reached", &a.unused),
    ] {
        let names = match list.is_empty() {
            true => "none".to_string(),
            false => list.join(", "),
        };
        ui.label(format!("{what}: {names}"));
    }
    if a.bound == "lower_bound" {
        ui.label("Lower bound: some references are unknown, so more areas may be reached.");
    }
}

pub fn draw_diff(ui: &mut egui::Ui, diff: Option<&Diff>) {
    let (base, lines, cut) = match diff.map(|d| &d.state) {
        None | Some(State::Running) => {
            ui.label("Reading the diff...");
            return;
        }
        Some(State::Failed(why)) => {
            ui.colored_label(ui.visuals().error_fg_color, why);
            return;
        }
        Some(State::Text { base, lines, cut }) => (base, lines, *cut),
    };
    if lines.is_empty() {
        ui.label(format!("No changes against {base} (tracked files)."));
        return;
    }
    ui.label(format!(
        "Against {base}, tracked files, uncommitted edits included."
    ));
    if cut {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Cut at {} KiB.", OUT_BYTES / 1024),
        );
    }
    let row = ui.text_style_height(&TextStyle::Monospace);
    egui::ScrollArea::both()
        .id_salt("diff")
        .max_height(DIFF_HEIGHT)
        .show_rows(ui, row, lines.len(), |ui, range| {
            for l in &lines[range] {
                let text = RichText::new(l).monospace();
                ui.label(match l.as_bytes() {
                    [b'+', b'+', b'+', ..] | [b'-', b'-', b'-', ..] => text,
                    [b'+', ..] => text.color(Color32::from_rgb(60, 160, 80)),
                    [b'-', ..] => text.color(Color32::from_rgb(200, 70, 70)),
                    [b'@', b'@', ..] => text.color(Color32::from_rgb(90, 130, 200)),
                    _ => text,
                });
            }
        });
}

/// What lanes and the people answering them agreed: questions and
/// answers newest first, then each spec's decisions (newest first) and
/// each lane's newest handoff decisions, in board order.
pub fn feed(ui: &mut egui::Ui, board: &LaneBoard) {
    egui::ScrollArea::vertical().id_salt("feed").show(ui, |ui| {
        ui.strong("Questions and answers");
        if board.attention.is_empty() {
            ui.label("None on open lanes.");
        }
        for a in &board.attention {
            let reason = a
                .reason
                .as_deref()
                .map_or(String::new(), |r| format!(", {r}"));
            let note = a.note.as_deref().unwrap_or("-");
            let lane = board.lanes.iter().find(|l| l.board.item.id == a.work_id);
            let title = lane.map_or("", |l| l.board.item.title.as_str());
            ui.label(format!(
                "{} #{} {title}: {} ({}{reason}): {note}",
                a.at, a.work_id, a.kind, a.by
            ));
        }
        for s in board.specs.iter().filter(|s| !s.decisions.is_empty()) {
            ui.strong(format!("{} {}: decisions", s.key, s.title));
            lines(ui, &s.decisions);
        }
        for lane in &board.lanes {
            let decisions = lane
                .board
                .handoff
                .as_ref()
                .and_then(|h| h.handoff.brief.as_ref())
                .map(|b| &b.decisions)
                .filter(|d| !d.is_empty());
            if let Some(d) = decisions {
                ui.strong(format!(
                    "#{} {}: handoff decisions",
                    lane.board.item.id, lane.board.item.title
                ));
                lines(ui, d);
            }
        }
    });
}

fn lines(ui: &mut egui::Ui, lines: &[String]) {
    for l in lines {
        ui.label(format!("- {l}"));
    }
}
