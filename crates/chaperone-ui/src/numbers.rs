//! The board's numbers, read from `stats` as the CLI prints it: a lane's
//! tokens, peak context and attention, and the cohort panel.

use chaperone_core::checks::ATTENTION_TYPES;
use chaperone_core::ledger::Refusal;
use chaperone_core::stats::{BoardStats, TOO_FEW_LANES};
use chaperone_core::work::LaneBoard;
use eframe::egui;
use serde_json::Value;

use crate::board;
use crate::overlap::Markers;

/// What a lane with no usage events is missing: `stats` names the cohort's
/// as `usage.reported`; its lane rows name nothing.
const NO_USAGE: &str = "no usage.reported";

/// The lane's `stats` row, if the stats read has it yet.
pub fn lane(stats: Option<&BoardStats>, id: i64) -> Option<&Value> {
    stats?.lanes.iter().find(|l| l["id"] == id)
}

/// Tokens in and out over every source, and the share of input read from
/// the cache.
pub fn tokens(row: Option<&Value>) -> [String; 2] {
    let Some(row) = row else {
        return ["...".into(), "...".into()];
    };
    let Some(sources) = row["tokens"].as_object() else {
        return [NO_USAGE.into(), "-".into()];
    };
    let sum = |k: &str| {
        sources
            .values()
            .filter_map(|s| s[k].as_i64())
            .reduce(|a, b| a + b)
    };
    let n = |v: Option<i64>| v.map_or("-".into(), count);
    let input = sum("input_total");
    let share = match (sum("cache_read"), input) {
        // A message with a null part has no input_total but keeps its
        // cache_read, so a share over the whole is no share.
        (Some(read), Some(input)) if input > 0 && read <= input => {
            format!("{:.0}%", read as f64 / input as f64 * 100.0)
        }
        _ => "-".into(),
    };
    [format!("{} / {}", n(input), n(sum("output_total"))), share]
}

pub fn peak(row: Option<&Value>) -> String {
    match row.map(|r| &r["peak_context"]) {
        None => "...".into(),
        Some(Value::Number(n)) => n.as_i64().map_or("-".into(), count),
        Some(_) => "-".into(),
    }
}

/// The attention types the lane has, in their order: `asked 2, nudged 1`.
pub fn attention(row: Option<&Value>) -> String {
    let Some(row) = row else {
        return "...".into();
    };
    let parts: Vec<String> = ATTENTION_TYPES
        .iter()
        .filter_map(|k| row["attention"][k].as_i64().map(|n| format!("{k} {n}")))
        .collect();
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(", ")
    }
}

/// The lanes this lane shares paths with, by board name, and on hover the
/// paths with each side's source; a worktree git could not read is named,
/// so no marker never reads as no overlap.
pub fn shares(id: i64, overlap: Option<&Markers>, board: &LaneBoard) -> (String, String) {
    let markers = match overlap {
        None => return ("...".into(), "Reading the worktrees...".into()),
        Some(Err(e)) => return ("error".into(), e.clone()),
        Some(Ok(m)) => m,
    };
    let Some(m) = markers.get(&id) else {
        return ("...".into(), "Not in the last worktree read yet.".into());
    };
    let mut names: Vec<String> = m
        .others
        .iter()
        .map(
            |o| match board.lanes.iter().find(|l| l.board.item.id == *o) {
                Some(l) => board::label(&l.board.item),
                None => format!("#{o}"),
            },
        )
        .collect();
    let mut hover = m.paths.clone();
    match m.worktree.as_deref() {
        Some("no_repo") => {
            names.push("not compared: no repo".into());
            hover.push("No repo recorded: never compared.".into());
        }
        Some(w) => {
            names.push(format!("worktree {}", w.split(':').next().unwrap_or(w)));
            hover.push(format!("worktree not read: {w}"));
        }
        None => {}
    }
    if m.cut > 0 {
        names.push(format!("+{} paths not compared", m.cut));
        hover.push(format!(
            "{} worktree paths past the cap were not compared.",
            m.cut
        ));
    }
    if names.is_empty() {
        return ("-".into(), "No path shared with another open lane.".into());
    }
    (names.join(", "), hover.join("\n"))
}

/// The date field as typed, and the cohort date the stats were last read
/// for, so a new one (the default rolling over) refills the field.
#[derive(Default)]
pub struct Since {
    edit: String,
    read: String,
}

/// The cohort panel; returns the date typed, once Enter or a click away
/// applies it.
pub fn cohort(ui: &mut egui::Ui, stats: Option<&BoardStats>, since: &mut Since) -> Option<String> {
    let mut applied = None;
    if let Some(s) = stats
        && s.since != since.read
    {
        // Text typed since the last read is kept.
        if since.edit == since.read {
            since.edit.clone_from(&s.since);
        }
        since.read.clone_from(&s.since);
    }
    ui.horizontal(|ui| {
        ui.strong("Merged since");
        let field = ui.add(egui::TextEdit::singleline(&mut since.edit).desired_width(110.0));
        let typed = since.edit.trim();
        // Unchanged, the default stays the default and keeps rolling.
        if field.lost_focus() && !typed.is_empty() && typed != since.read {
            applied = Some(typed.to_string());
        }
        ui.label("(YYYY-MM-DD, UTC)");
    });
    let Some(stats) = stats else {
        ui.label("Reading the stats...");
        return applied;
    };
    let c = match &stats.cohort {
        Ok(v) => &v["cohort"],
        Err(r) => {
            let text = match r {
                Refusal::InvalidInput(m) => m.clone(),
                r => format!("{r:?}"),
            };
            ui.colored_label(ui.visuals().error_fg_color, text);
            return applied;
        }
    };
    let metric = |field: &str| match &c[field] {
        Value::Null => match c["missing"][field].as_str() {
            Some(m) => format!("missing: {m}"),
            None => "none".into(),
        },
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => count(i),
            // Medians are floats: token counts shorten, hours stay.
            (None, Some(f)) if field.contains("context") => count(f.round() as i64),
            _ => n.to_string(),
        },
        v => v.to_string(),
    };
    let skipped = |field: &str| match c[field].as_object() {
        Some(by) if !by.is_empty() => {
            let by: Vec<String> = by.iter().map(|(k, n)| format!("{k} {n}")).collect();
            format!(" (skipped: {})", by.join(", "))
        }
        _ => String::new(),
    };
    let attention: Vec<String> = ATTENTION_TYPES
        .iter()
        .map(|k| format!("{k} {}", c["attention"][k].as_i64().unwrap_or(0)))
        .collect();
    let few = if c["too_few_lanes"] == true {
        format!(" (under {TOO_FEW_LANES}: flagged, not scored)")
    } else {
        String::new()
    };
    let rows = [
        (
            "lanes",
            format!("{}, {} merged{few}", metric("lanes"), metric("merged")),
        ),
        (
            "claim to merge, median h",
            metric("claim_to_merge_h_median"),
        ),
        (
            "context per merged PR, median",
            metric("context_tokens_per_merged_pr_median"),
        ),
        ("peak context, median", metric("peak_context_median")),
        ("lanes over 500k", metric("lanes_peak_over_500k")),
        (
            "plan checked",
            metric("lanes_plan_checked") + &skipped("plan_check_skipped"),
        ),
        (
            "quality gated",
            metric("lanes_quality_gated") + &skipped("quality_gate_skipped"),
        ),
        ("verdict on head", metric("lanes_verdict_on_head")),
        ("attention", attention.join(", ")),
    ];
    egui::Grid::new("cohort").striped(true).show(ui, |ui| {
        for (name, value) in rows {
            ui.label(name);
            ui.label(value);
            ui.end_row();
        }
    });
    applied
}

/// A token count, short: `850`, `12.3k`, `1.2M`.
fn count(n: i64) -> String {
    match n {
        n if n < 1000 => n.to_string(),
        n if n < 1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        n => format!("{:.1}M", n as f64 / 1e6),
    }
}
