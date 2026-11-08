//! The lane page (#109 slice 4): what the lane is, how it is doing against
//! its repo and what it changed. A header, six tiles, the timeline, the
//! decisions, what to check by hand, the files and the lane's change graph,
//! then the #41 panel's handoffs, findings, areas and diff, folded. On a
//! lane that needs you, the owner answers it, sends it back or opens it in
//! herdr (`writer`).

use std::collections::HashMap;

use chaperone_core::stats::{BoardStats, PEAK_OVER};
use chaperone_core::work::{BoardLane, LanePage};
use eframe::egui::{self, RichText};

use crate::areas::{self, Areas, State};
use crate::diff::{Diff, Files};
use crate::watch::{Detail, LaneGraph, Median, Selected};
use crate::writer::{Act, Confirm, Lane, Wrote};
use crate::{board, detail, graph, needs, numbers, repo, theme};
use chaperone_core::checks::NOTE_CHARS;
use chaperone_core::spec::parse::asks_about;

/// What the viewer asked for on the page.
#[derive(Default)]
pub struct Asked {
    pub back: bool,
    /// The diff's Refresh.
    pub refresh: bool,
    /// A write for the writer.
    pub act: Option<Act>,
}

/// What the owner has typed on the page, for lane `lane` only, kept
/// until the act it went with is done.
#[derive(Default)]
pub struct Typed {
    pub lane: i64,
    pub answer: String,
    pub reason: String,
    /// The question the answer is for, the oldest when typing started.
    asks: Option<i64>,
    /// Why the answer was not sent: that question was answered elsewhere,
    /// or one was asked on a lane that had none.
    gone: Option<&'static str>,
    /// The act sent and not done yet: true for a send-back.
    sent: Option<bool>,
}

/// What the page reads, each for the selected lane only.
pub struct Reads<'a> {
    pub lane: Option<&'a BoardLane>,
    pub detail: Option<&'a Detail>,
    pub diff: Option<&'a Diff>,
    pub areas: &'a HashMap<i64, Areas>,
    pub stats: Option<&'a BoardStats>,
    pub median: Option<&'a Median>,
    /// The writer is busy with an act.
    pub writing: bool,
    pub wrote: Option<&'a Wrote>,
    pub lane_graph: Option<&'a LaneGraph>,
}

pub fn draw(ui: &mut egui::Ui, sel: &Selected, r: &Reads, typed: &mut Typed, now: i64) -> Asked {
    let mut asked = Asked::default();
    if typed.lane != sel.id {
        *typed = Typed {
            lane: sel.id,
            ..Typed::default()
        };
    }
    // Each lane keeps its own scroll offset.
    let salt = ("lane", sel.id);
    egui::ScrollArea::vertical().id_salt(salt).show(ui, |ui| {
        asked.back = ui.button("< Back").clicked();
        let page = match r.detail.map(|d| &d.read) {
            Some(Ok(p)) => Some(p),
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                None
            }
            None => None,
        };
        let Some(lane) = r.lane else {
            ui.label("Reading the lane...");
            return;
        };
        header(ui, sel, lane, page, r, now);
        let Some(page) = page else {
            if r.detail.is_none() {
                ui.label("Reading the lane...");
            }
            return;
        };
        if needs::why(lane, now).is_some() {
            card(ui, "Needs you", |ui| {
                asked.act = actions(ui, lane, page, r, typed)
            });
        }
        tiles(ui, lane, page, r, now);
        card(ui, "Timeline", |ui| timeline(ui, page));
        card(ui, "Decisions", |ui| decisions(ui, page));
        card(ui, "Check by hand", |ui| check_by_hand(ui, page));
        card(ui, "Files", |ui| {
            files(ui, r.diff.and_then(|d| d.files.as_ref()))
        });
        let declared = areas::declared(lane);
        let a = r.areas.get(&sel.id);
        card(ui, "Change graph", |ui| {
            change_graph(ui, sel.id, declared, a, r.lane_graph)
        });
        let d = &page.detail;
        ui.collapsing("Handoffs", |ui| detail::handoffs(ui, d));
        ui.collapsing("Review findings", |ui| detail::findings(ui, d));
        ui.collapsing("Areas", |ui| detail::draw_areas(ui, declared, a));
        ui.collapsing("Diff against the base", |ui| {
            asked.refresh = ui.button("Refresh").clicked();
            detail::draw_diff(ui, r.diff);
        });
    });
    asked
}

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    theme::glass().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.strong(title);
        body(ui);
    });
}

fn header(
    ui: &mut egui::Ui,
    sel: &Selected,
    lane: &BoardLane,
    page: Option<&LanePage>,
    r: &Reads,
    now: i64,
) {
    let item = &lane.board.item;
    let row = repo::row(lane, r.stats, r.areas, now);
    ui.horizontal_wrapped(|ui| {
        ui.heading(&item.title);
        match row.why {
            Some(_) => ui.label(theme::alert(row.state)),
            None => ui.label(RichText::new(row.state).strong()),
        };
    });
    // The newest brief's goal, else the spec slice's title, once read.
    if let Some(p) = page {
        let goal = p
            .detail
            .handoffs
            .first()
            .and_then(|h| h.handoff.brief.as_ref())
            .and_then(|b| b.goal.trim().lines().next())
            .filter(|g| !g.is_empty());
        let slice = p.detail.slice.as_ref().and_then(|s| s.in_spec.as_ref());
        let slice = slice.map(|s| s.title.as_str());
        ui.label(goal.or(slice).unwrap_or("No goal recorded yet."));
    }
    ui.horizontal_wrapped(|ui| {
        detail::issue(ui, sel.key.as_deref());
        let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
        for (k, v) in [
            ("lane", row.label.clone()),
            ("agent", text(&item.agent)),
            ("branch", text(&item.branch)),
            ("PR", text(&item.pr_ref)),
            ("pipeline", text(&item.pipeline_id)),
            ("running", row.running.clone()),
        ] {
            ui.label(RichText::new(k).small().weak());
            ui.label(v);
        }
    });
}

/// Answer, send back and open in herdr, each one act; a button waits
/// while the writer is busy.
fn actions(
    ui: &mut egui::Ui,
    lane: &BoardLane,
    page: &LanePage,
    r: &Reads,
    typed: &mut Typed,
) -> Option<Act> {
    let item = &lane.board.item;
    let mut act = None;
    let free = !r.writing;
    let wrote = r.wrote.filter(|w| w.work == item.id).map(|w| &w.said);
    if !r.writing
        && let Some(back) = typed.sent.take()
        && let Some(Ok(_)) = wrote
    {
        match back {
            true => typed.reason.clear(),
            false => typed.answer.clear(),
        }
    }
    // What the lane asks: its open questions, else its blocker or next
    // step, which an answer records as the question.
    let blocker = item.blocker.clone().or_else(|| item.next_action.clone());
    for q in &page.questions {
        ui.label(format!(
            "Asks: {}",
            q.note.as_deref().unwrap_or("(no text)")
        ));
    }
    if page.questions.is_empty()
        && let Some(b) = &blocker
    {
        ui.label(format!("Says: {b}"));
    }
    let Some(agent) = item.agent.clone() else {
        ui.label("No herdr agent recorded for the lane: it cannot be answered from here.");
        return None;
    };
    let target = Lane {
        work: item.id,
        agent,
        key: item
            .source_key
            .clone()
            .unwrap_or_else(|| board::label(item)),
    };
    let answer = |asks, text: String, confirm| Act::Answer {
        lane: target.clone(),
        asks,
        blocker: blocker.clone(),
        text,
        confirm,
    };
    let in_spec = page.detail.slice.as_ref().and_then(|s| s.in_spec.as_ref());
    for d in in_spec.iter().flat_map(|s| &s.provisional) {
        let (Some(q), Some(a), Some(spec)) = (&d.question, &d.answer, in_spec) else {
            continue;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Provisional: {q} -> {a}"));
            if ui.add_enabled(free, egui::Button::new("Accept")).clicked() {
                // A spec-wide decision is confirmed spec-wide.
                let confirm = Confirm {
                    spec: spec.parent_key.clone(),
                    slice: d.slice,
                    question: q.clone(),
                    answer: a.clone(),
                };
                // Only a question about this decision is answered by it.
                let asks = page
                    .questions
                    .iter()
                    .find(|x| x.note.as_deref().is_some_and(|n| asks_about(n, q)));
                let text = format!("confirmed: {q} -> {a}");
                act = Some(answer(asks.map(|x| x.id), text, Some(confirm)));
            }
        });
    }
    if page.questions.len() > 1 {
        ui.label(RichText::new("An answer goes to the oldest question.").small());
    }
    let line = |ui: &mut egui::Ui, text: &mut String, hint: &str, button: &str| {
        ui.horizontal(|ui| {
            let edit = egui::TextEdit::singleline(text)
                .hint_text(hint)
                .char_limit(NOTE_CHARS)
                .desired_width(420.0);
            ui.add(edit);
            let ok = free && !text.trim().is_empty();
            ui.add_enabled(ok, egui::Button::new(button)).clicked()
        })
        .inner
    };
    // A typed answer goes to an open question, or asks one for a lane
    // that blocked itself; a ready lane has nothing to answer.
    if !page.questions.is_empty() || item.status == "blocked" {
        if typed.answer.trim().is_empty() {
            typed.asks = page.questions.first().map(|q| q.id);
        }
        if line(ui, &mut typed.answer, "Your answer", "Answer") {
            let open = |id| page.questions.iter().any(|q| q.id == id);
            typed.gone = match typed.asks {
                Some(id) if !open(id) => Some("that question was answered elsewhere"),
                None if !page.questions.is_empty() => {
                    Some("the lane asked a question since you started typing")
                }
                _ => None,
            };
            if typed.gone.is_some() {
                typed.asks = page.questions.first().map(|q| q.id);
            } else {
                act = Some(answer(typed.asks, typed.answer.clone(), None));
                typed.sent = Some(false);
            }
        }
        if let Some(why) = typed.gone {
            let e = format!("Not sent: {why}. Answer again to send it to what the lane asks now.");
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }
    if line(ui, &mut typed.reason, "Why it goes back", "Send back") {
        act = Some(Act::SendBack {
            lane: target.clone(),
            round: page.sent_back as u64 + 1,
            reason: typed.reason.clone(),
        });
        typed.sent = Some(true);
    }
    match &item.session {
        Some(ws) => {
            if ui
                .add_enabled(free, egui::Button::new("Open in herdr"))
                .clicked()
            {
                act = Some(Act::Focus {
                    work: item.id,
                    workspace: ws.clone(),
                });
            }
        }
        None => {
            ui.label("No herdr workspace recorded for the lane.");
        }
    }
    match (r.writing, wrote) {
        (true, _) => {
            ui.label("Working...");
        }
        (false, Some(Ok(m))) => {
            ui.label(m);
        }
        (false, Some(Err(e))) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        (false, None) => {}
    }
    act
}

/// One stat: a name, a value and a line under it.
fn tile(ui: &mut egui::Ui, name: &str, value: &str, under: &str) {
    theme::glass().show(ui, |ui| {
        ui.set_width(200.0);
        ui.label(RichText::new(name).small().weak());
        ui.label(RichText::new(value).heading().strong());
        ui.label(RichText::new(under).small());
    });
}

fn tiles(ui: &mut egui::Ui, lane: &BoardLane, page: &LanePage, r: &Reads, now: i64) {
    let stats = numbers::lane(r.stats, lane.board.item.id);
    ui.horizontal_wrapped(|ui| {
        let (value, under) = time(lane, r.median, now);
        tile(ui, "Time running", &value, &under);
        let [io, cache] = numbers::tokens(stats);
        let under = match cache.as_str() {
            "-" | "..." => "cache share: -".into(),
            c => format!("{c} read from the cache"),
        };
        tile(ui, "Tokens in / out", &io, &under);
        let peak = numbers::peak(stats);
        let under = match stats.and_then(|s| s["peak_context"].as_i64()) {
            Some(p) => format!(
                "{:.0}% of {}k",
                p as f64 / PEAK_OVER as f64 * 100.0,
                PEAK_OVER / 1000
            ),
            None => format!("of {}k", PEAK_OVER / 1000),
        };
        tile(ui, "Peak context", &peak, &under);
        let (value, under) = review(page);
        tile(ui, "Review", &value, &under);
        let attention = numbers::attention(stats);
        let open = match lane.open_questions {
            0 => "no open question".to_string(),
            n => format!("{n} open"),
        };
        tile(ui, "People needed", &attention, &open);
        let (value, under) = gates(page);
        tile(ui, "Gates", &value, &under);
    });
}

/// Time running, and the repo's median claim to merge over 30 days.
fn time(lane: &BoardLane, median: Option<&Median>, now: i64) -> (String, String) {
    let Some(claimed) = lane.claimed_secs else {
        return ("not started".into(), String::new());
    };
    let value = board::age(claimed, now);
    let Some(repo) = lane.board.item.repo_slug.as_deref() else {
        return (value, "no repo to compare with".into());
    };
    let under = match median.filter(|m| m.repo == repo).map(|m| &m.read) {
        None => "repo median: ...".into(),
        Some(Err(e)) => format!("repo median not read: {e}"),
        Some(Ok(v)) => {
            let lanes = v["lanes"].as_i64().unwrap_or(0);
            let few = if v["too_few_lanes"] == true {
                ", too few to score"
            } else {
                ""
            };
            match v["claim_to_merge_h_median"].as_f64() {
                Some(h) => {
                    format!("vs repo median {h:.1}h ({lanes} merged in 30 days{few})")
                }
                None => "repo median: no merges in 30 days".into(),
            }
        }
    };
    (value, under)
}

/// The newest scores, the engine's rounds and whether its review head is
/// the newest handoff's commit (what `head_eq` compares).
fn review(page: &LanePage) -> (String, String) {
    let scores: Vec<String> = page
        .reviews
        .iter()
        .filter(|r| r.outcome == "ran")
        .filter_map(|r| r.score)
        .take(3)
        .map(|s| format!("{s}"))
        .collect();
    let value = match scores.is_empty() {
        true => "no score".into(),
        // The rows are newest first: shown oldest first, the newest last.
        false => scores.into_iter().rev().collect::<Vec<_>>().join(", "),
    };
    let head = page
        .detail
        .handoffs
        .first()
        .and_then(|h| h.handoff.commit.as_deref());
    let on = match (page.review_head.as_deref(), head) {
        (None, _) => "no local review".into(),
        (Some(_), None) => "no handoff commit".into(),
        (Some(r), Some(h)) if r == h => "on head".into(),
        (Some(r), Some(_)) => format!("not on head (reviewed {})", board::short(r)),
    };
    let rounds = match page.review_rounds {
        1 => "1 round".to_string(),
        n => format!("{n} rounds"),
    };
    (value, format!("{rounds}, {on}"))
}

fn gates(page: &LanePage) -> (String, String) {
    let Some(g) = &page.gates else {
        return (
            "-".into(),
            "no pipeline document for the lane's stamp".into(),
        );
    };
    let passed = g.gates.iter().filter(|g| g.passed).count();
    let value = format!("{passed} of {}", g.gates.len());
    let failing: Vec<&str> = g
        .gates
        .iter()
        .filter(|g| !g.passed)
        .map(|g| g.gate.as_str())
        .collect();
    let to = format!("{:?}", g.to).to_lowercase();
    let under = match failing.is_empty() {
        true => format!("all pass to {to}"),
        false => format!("to {to}, not yet: {}", failing.join(", ")),
    };
    (value, under)
}

fn timeline(ui: &mut egui::Ui, page: &LanePage) {
    if page.timeline.is_empty() {
        ui.label("No events.");
        return;
    }
    detail::capped(ui, page.timeline.len());
    egui::Grid::new("timeline").striped(true).show(ui, |ui| {
        for t in &page.timeline {
            ui.label(RichText::new(&t.at).small().weak());
            ui.label(t.minutes.map_or("-".into(), |m| format!("+{m}m")));
            ui.label(&t.kind);
            ui.label(t.what.as_deref().unwrap_or(""));
            ui.label(RichText::new(t.agent.as_deref().unwrap_or("")).small());
            ui.end_row();
        }
    });
}

fn decisions(ui: &mut egui::Ui, page: &LanePage) {
    if page.decisions.is_empty() {
        ui.label("No decisions recorded.");
    }
    for d in &page.decisions {
        ui.horizontal_wrapped(|ui| {
            // A spec line's who and when are in its text.
            let who = d.by.as_deref().unwrap_or("spec");
            ui.label(RichText::new(who).small().weak());
            ui.label(&d.text);
        });
    }
    // The dossier keeps the spec's newest 20.
    let slice = page.detail.slice.as_ref();
    if let Some(n) = slice.and_then(|s| s.omitted.get("slice_decisions")) {
        ui.label(format!("Left out: the {n} oldest decisions."));
    }
}

fn check_by_hand(ui: &mut egui::Ui, page: &LanePage) {
    let Some(h) = page.detail.handoffs.first() else {
        ui.label("No handoff yet.");
        return;
    };
    let items = h.handoff.brief.as_ref().map(|b| &b.check_by_hand);
    match items.filter(|i| !i.is_empty()) {
        None => {
            ui.label("Nothing named in the newest handoff.");
        }
        Some(items) => {
            for i in items {
                ui.label(format!("- {i}"));
            }
        }
    }
}

fn files(ui: &mut egui::Ui, files: Option<&Result<Files, String>>) {
    let f = match files {
        None => {
            ui.label("Reading the files...");
            return;
        }
        Some(Err(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
        Some(Ok(f)) => f,
    };
    if let Some(why) = &f.no_areas {
        ui.colored_label(ui.visuals().warn_fg_color, format!("No areas: {why}"));
    }
    if f.files.is_empty() {
        ui.label(format!("No changes against {}.", f.base));
        return;
    }
    ui.label(format!(
        "Against {}: committed, uncommitted and untracked.",
        f.base
    ));
    let n = |v: Option<u64>| v.map_or("-".into(), |v| v.to_string());
    egui::Grid::new("files").striped(true).show(ui, |ui| {
        for c in ["file", "area", "+", "-"] {
            ui.label(RichText::new(c).small().weak());
        }
        ui.end_row();
        for c in &f.files {
            let path = match c.untracked {
                true => format!("{} (new, untracked)", c.path),
                false => c.path.clone(),
            };
            ui.monospace(path);
            let area = match (&c.area, &f.no_areas) {
                (Some(a), _) => a.as_str(),
                (None, None) => "in no area",
                (None, Some(_)) => "-",
            };
            ui.label(area);
            ui.label(n(c.added));
            ui.label(n(c.removed));
            ui.end_row();
        }
    });
}

/// One line (declared areas against the change, entry points reached of
/// the repo's, exact or a lower bound), then the graph, its out-of-scope
/// files ringed red as the areas check reads them.
fn change_graph(
    ui: &mut egui::Ui,
    id: i64,
    declared: Option<&[String]>,
    a: Option<&Areas>,
    g: Option<&LaneGraph>,
) {
    let snap = match g.map(|g| &g.read) {
        None => {
            ui.label("Reading the change graph...");
            return;
        }
        Some(Err(why)) => {
            ui.label(why);
            return;
        }
        Some(Ok(s)) => s,
    };
    let done = a.filter(|a| declared.is_some_and(|d| a.is_for(d)));
    let mut opts = graph::Opts {
        paths: true,
        ..Default::default()
    };
    let mut not_ringed = 0;
    if let Some(State::Done(x)) = done.map(|a| &a.state) {
        for u in &x.undeclared {
            opts.out_of_scope.extend(u.files.iter().map(String::as_str));
            not_ringed += u.files_omitted;
        }
        opts.out_of_scope
            .extend(x.unmapped.iter().map(String::as_str));
        not_ringed += x.unmapped_omitted;
    }
    let layout = &snap.layout;
    opts.extra = opts
        .out_of_scope
        .iter()
        .copied()
        .filter(|p| !layout.has(p))
        .collect();
    let reached = match snap.reached {
        Some((n, total)) => format!("entry points reached {n} of {total}"),
        None => "entry points not read".into(),
    };
    let exact = snap.exact && snap.notes.is_empty() && snap.files_omitted == 0;
    let bound = match exact && opts.extra.is_empty() {
        true => "exact",
        false => "lower bound",
    };
    let scope = areas::card(declared, a);
    ui.label(RichText::new(format!("Areas {scope}; {reached}; {bound}.")).strong());
    let warn = ui.visuals().warn_fg_color;
    for n in &snap.notes {
        ui.colored_label(warn, format!("Overlap: {n}"));
    }
    for n in &layout.notes {
        ui.colored_label(warn, n);
    }
    if snap.files_omitted > 0 {
        let n = snap.files_omitted;
        ui.colored_label(warn, format!("Overlap left out {n} files."));
    }
    if not_ringed > 0 {
        ui.colored_label(warn, format!("{not_ringed} more out of scope, not ringed."));
    }
    if !opts.extra.is_empty() {
        let n = opts.extra.len();
        ui.colored_label(
            warn,
            format!("Out of scope and uncommitted, dependents not read: {n}."),
        );
    }
    ui.label(
        RichText::new(
            "Read at each commit: the files changed, what depends on them by depth, then the \
             entry points reached. Ringed: another open lane changes it too; red: out of the \
             declared areas. Uncommitted edits show at the next commit.",
        )
        .small()
        .weak(),
    );
    egui::ScrollArea::horizontal()
        .id_salt(("lane graph", id))
        .show(ui, |ui| graph::draw_one(ui, layout, &opts));
}
