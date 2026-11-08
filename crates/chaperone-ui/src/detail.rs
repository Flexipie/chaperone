//! The selected lane's panel (issue, spec slice, handoffs, checks,
//! findings, diff) and the board's feed of decisions and questions.

use chaperone_core::work::{BoardLane, LaneBoard, LaneDetail};
use eframe::egui::{self, Color32, RichText, TextStyle};

use crate::diff::{Diff, State};
use crate::watch::Selected;

/// Lines of diff shown before the panel scrolls them.
const DIFF_HEIGHT: f32 = 420.0;

/// The lane's panel; true when Refresh was clicked.
pub fn draw(
    ui: &mut egui::Ui,
    sel: &Selected,
    lane: Option<&BoardLane>,
    detail: Option<&LaneDetail>,
    diff: Option<&Diff>,
) -> bool {
    let mut refresh = false;
    egui::ScrollArea::vertical()
        .id_salt("detail")
        .show(ui, |ui| {
            if let Some(lane) = lane {
                ui.heading(&lane.board.item.title);
            }
            issue(ui, sel.key.as_deref());
            let Some(d) = detail else {
                ui.label("Reading the lane...");
                return;
            };
            section(ui, "Spec slice", |ui| slice(ui, d));
            section(ui, "Handoffs", |ui| handoffs(ui, sel.id, d));
            section(ui, "Checks", |ui| checks(ui, lane, d));
            section(ui, "Review findings", |ui| findings(ui, d));
            section(ui, "Diff against the base", |ui| {
                refresh = ui.button("Refresh").clicked();
                draw_diff(ui, diff);
            });
        });
    refresh
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(true)
        .show(ui, body);
}

/// The key, linked when it names a GitHub issue. The body stays in the
/// tracker (ADR 0007); a Linear key has no URL in the ledger.
fn issue(ui: &mut egui::Ui, key: Option<&str>) {
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

fn slice(ui: &mut egui::Ui, d: &LaneDetail) {
    let Some(dossier) = &d.dossier else {
        ui.label("Nothing in the ledger on this key.");
        return;
    };
    if let Some(i) = &dossier.slice.in_spec {
        let n = i.n.map_or(String::new(), |n| format!("slice {n}: "));
        ui.strong(format!("{n}{}", i.title));
        ui.label(format!(
            "{} {} (v{})",
            i.parent_key, i.parent_title, i.version
        ));
        lines(ui, &i.decisions, "No decisions.");
    } else {
        ui.label("Not a slice of a spec.");
    }
}

fn handoffs(ui: &mut egui::Ui, id: i64, d: &LaneDetail) {
    let Some(dossier) = &d.dossier else {
        ui.label("No handoffs read: the lane has no dossier.");
        return;
    };
    let mut any = false;
    for h in dossier.handoffs.items.iter().filter(|h| h.work_id == id) {
        any = true;
        let ready = match h.ready {
            Some(true) => " ready",
            Some(false) => " not ready",
            None => "",
        };
        let status = h.status.as_deref().unwrap_or("-");
        ui.strong(format!("{} {status}{ready}", h.at));
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
    if !any {
        ui.label("No handoffs.");
    }
}

fn checks(ui: &mut egui::Ui, lane: Option<&BoardLane>, d: &LaneDetail) {
    if let Some(s) = lane.map(|l| &l.stamps)
        && let Some(v) = &s.verdict
    {
        let sha = s.verdict_sha.as_deref().unwrap_or("-");
        ui.label(format!("merge verdict: {v} @ {}", &sha[..sha.len().min(7)]));
    }
    if d.checks.is_empty() {
        ui.label("No checks recorded.");
        return;
    }
    egui::Grid::new("checks").striped(true).show(ui, |ui| {
        for c in &d.checks {
            ui.label(&c.check);
            ui.monospace(&c.head_sha[..c.head_sha.len().min(7)]);
            ui.label(&c.outcome);
            ui.label(&c.at);
            ui.end_row();
        }
    });
}

fn findings(ui: &mut egui::Ui, d: &LaneDetail) {
    if d.findings.is_empty() {
        ui.label("No review findings recorded.");
        return;
    }
    for f in &d.findings {
        let path = f.path.as_deref().map_or(String::new(), |p| format!(" {p}"));
        ui.label(format!(
            "{} {} [{}]{path}: {}",
            f.check, f.severity, f.status, f.title
        ));
    }
}

fn draw_diff(ui: &mut egui::Ui, diff: Option<&Diff>) {
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
        ui.colored_label(ui.visuals().warn_fg_color, "Cut at 1 MiB.");
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

/// What lanes and the people answering them agreed, newest first:
/// questions and answers, each spec's decisions, each lane's newest
/// handoff decisions.
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
            ui.label(format!(
                "{} #{} {} ({}{reason}): {note}",
                a.at, a.work_id, a.kind, a.by
            ));
        }
        for s in board.specs.iter().filter(|s| !s.decisions.is_empty()) {
            ui.strong(format!("{} {}: decisions", s.key, s.title));
            lines(ui, &s.decisions, "");
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
                lines(ui, d, "");
            }
        }
    });
}

fn lines(ui: &mut egui::Ui, lines: &[String], none: &str) {
    if lines.is_empty() && !none.is_empty() {
        ui.label(none);
    }
    for l in lines {
        ui.label(format!("- {l}"));
    }
}
