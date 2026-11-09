//! The lane page: a header, six tiles, the timeline, the decisions and the
//! files with their lines added and removed. The words are chaperone-ui's
//! `lane.rs` and `numbers.rs`, copied: both import egui, so they cannot be
//! compiled in the way `board.rs` and `needs.rs` are.

use chaperone_core::checks::ATTENTION_TYPES;
use chaperone_core::git::ChangedFile;
use chaperone_core::stats::PEAK_OVER;
use chaperone_core::work::{BoardLane, LanePage};
use gpui::prelude::FluentBuilder;
use gpui::{AnyElement, App, Hsla, IntoElement, ParentElement, SharedString, Styled, div, px};
use gpui_component::{ActiveTheme, h_flex, v_flex};
use serde_json::Value;

use crate::theme::{self, STEP};
use crate::watch::Page;
use crate::{board, lanes};

/// The changed files against the base, or why there are none.
pub type Files = Result<(String, Vec<ChangedFile>), String>;

/// What the page reads, each for the shown lane only.
pub struct Reads<'a> {
    pub id: i64,
    pub lane: Option<&'a BoardLane>,
    pub page: Option<&'a Page>,
    pub median: Option<&'a (String, Result<Value, String>)>,
    pub files: Option<&'a Files>,
}

/// The page, with `merge` (the confirm's trigger) in the header.
pub fn draw(r: &Reads, merge: impl IntoElement, now: i64, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let Some(lane) = r.lane else {
        // Every board lane is open: a merged or closed one has no page here.
        return line(format!("Lane {} is not open.", r.id), muted).into_any_element();
    };
    let page = r.page.map(|p| &p.read);
    let mut body = v_flex().gap(px(STEP * 4.0)).child(header(
        lane,
        page.and_then(|p| p.as_ref().ok()),
        merge,
        now,
        cx,
    ));
    match page {
        None => body = body.child(line("Reading the lane...", muted)),
        Some(Err(e)) => body = body.child(line(e.clone(), cx.theme().danger)),
        Some(Ok(p)) => {
            let stats = r.page.map(|p| p.stats.as_ref());
            body = body
                .child(tiles(lane, p, stats, r.median, now, cx))
                .child(card("Timeline", timeline(p, cx), cx))
                .child(card("Decisions", decisions(p, cx), cx));
        }
    }
    body.child(card("Files", files(r.files, cx), cx))
        .into_any_element()
}

fn line(text: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div().text_color(color).child(text.into())
}

fn card(title: &str, body: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap(px(STEP * 2.0))
        .p(px(STEP * 4.0))
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(title.to_string()),
        )
        .child(body)
}

fn header(
    lane: &BoardLane,
    page: Option<&LanePage>,
    merge: impl IntoElement,
    now: i64,
    cx: &App,
) -> impl IntoElement {
    let item = &lane.board.item;
    let (state, needs_you) = lanes::state(lane, now);
    // The newest brief's goal, else the spec slice's title, once read.
    let goal = page.map(|p| {
        let brief = p
            .detail
            .handoffs
            .first()
            .and_then(|h| h.handoff.brief.as_ref());
        let goal = brief.and_then(|b| b.goal.trim().lines().next());
        let slice = p.detail.slice.as_ref().and_then(|s| s.in_spec.as_ref());
        goal.filter(|g| !g.is_empty())
            .or(slice.map(|s| s.title.as_str()))
            .unwrap_or("No goal recorded yet.")
            .to_string()
    });
    let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
    let running = lane
        .claimed_secs
        .map_or("not started".into(), |c| board::age(c, now));
    let facts = [
        ("lane", board::label(item)),
        ("agent", text(&item.agent)),
        ("branch", text(&item.branch)),
        ("PR", text(&item.pr_ref)),
        ("pipeline", text(&item.pipeline_id)),
        ("running", running),
    ];
    let muted = cx.theme().muted_foreground;
    v_flex()
        .gap(px(STEP * 2.0))
        .child(
            h_flex()
                .gap(px(STEP * 3.0))
                .child(div().text_xl().child(item.title.clone()))
                .child(
                    div()
                        .text_color(if needs_you { theme::alert() } else { muted })
                        .child(state),
                )
                .child(div().flex_1())
                .child(merge),
        )
        .children(goal.map(|g| line(g, cx.theme().foreground)))
        .child(
            h_flex()
                .flex_wrap()
                .gap(px(STEP * 3.0))
                .children(facts.into_iter().map(|(k, v)| {
                    h_flex()
                        .gap(px(STEP))
                        .child(div().text_xs().text_color(muted).child(k))
                        .child(div().text_sm().child(v))
                })),
        )
}

fn tile(name: &str, value: String, under: String, cx: &App) -> impl IntoElement {
    v_flex()
        .w(px(200.0))
        .gap(px(STEP))
        .p(px(STEP * 3.0))
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(name.to_string()),
        )
        .child(div().text_lg().child(value))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(under),
        )
}

fn tiles(
    lane: &BoardLane,
    page: &LanePage,
    stats: Option<Result<&Value, &String>>,
    median: Option<&(String, Result<Value, String>)>,
    now: i64,
    cx: &App,
) -> impl IntoElement {
    // A refused or failed stats read is said on every tile it feeds.
    let row = match stats {
        Some(Err(e)) => Err(e.clone()),
        Some(Ok(v)) => Ok(Some(v)),
        None => Ok(None),
    };
    let from_stats = |f: &dyn Fn(Option<&Value>) -> (String, String)| match &row {
        Ok(r) => f(*r),
        Err(e) => ("-".into(), format!("stats not read: {e}")),
    };
    let (time, time_under) = time(lane, median, now);
    let (io, cache) = from_stats(&|r| {
        let [io, cache] = tokens(r);
        let under = match cache.as_str() {
            "-" | "..." => "cache share: -".into(),
            c => format!("{c} read from the cache"),
        };
        (io, under)
    });
    let (peak, peak_under) = from_stats(&|r| {
        let under = match r.and_then(|s| s["peak_context"].as_i64()) {
            Some(p) => format!(
                "{:.0}% of {}k",
                p as f64 / PEAK_OVER as f64 * 100.0,
                PEAK_OVER / 1000
            ),
            None => format!("of {}k", PEAK_OVER / 1000),
        };
        (peak(r), under)
    });
    let (review, review_under) = review(page);
    let (people, _) = from_stats(&|r| (attention(r), String::new()));
    let open = match lane.open_questions {
        0 => "no open question".to_string(),
        n => format!("{n} open"),
    };
    let (gates, gates_under) = gates(page);
    h_flex()
        .flex_wrap()
        .gap(px(STEP * 3.0))
        .child(tile("Time running", time, time_under, cx))
        .child(tile("Tokens in / out", io, cache, cx))
        .child(tile("Peak context", peak, peak_under, cx))
        .child(tile("Review", review, review_under, cx))
        .child(tile("People needed", people, open, cx))
        .child(tile("Gates", gates, gates_under, cx))
}

// Copied from crates/chaperone-ui/src/lane.rs `time`.
/// Time running, and the repo's median claim to merge over 30 days.
fn time(
    lane: &BoardLane,
    median: Option<&(String, Result<Value, String>)>,
    now: i64,
) -> (String, String) {
    let Some(claimed) = lane.claimed_secs else {
        return ("not started".into(), String::new());
    };
    let value = board::age(claimed, now);
    let Some(repo) = lane.board.item.repo_slug.as_deref() else {
        return (value, "no repo to compare with".into());
    };
    let under = match median.filter(|m| m.0 == repo).map(|m| &m.1) {
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
                Some(h) => format!("vs repo median {h:.1}h ({lanes} merged in 30 days{few})"),
                None => "repo median: no merges in 30 days".into(),
            }
        }
    };
    (value, under)
}

// Copied from crates/chaperone-ui/src/numbers.rs `tokens`.
/// Tokens in and out over every source, and the share of input read from
/// the cache.
fn tokens(row: Option<&Value>) -> [String; 2] {
    let Some(row) = row else {
        return ["...".into(), "...".into()];
    };
    let Some(sources) = row["tokens"].as_object() else {
        return ["no usage.reported".into(), "-".into()];
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
        (Some(read), Some(input)) if input > 0 && read <= input => {
            format!("{:.0}%", read as f64 / input as f64 * 100.0)
        }
        _ => "-".into(),
    };
    [format!("{} / {}", n(input), n(sum("output_total"))), share]
}

// Copied from crates/chaperone-ui/src/numbers.rs `peak`.
fn peak(row: Option<&Value>) -> String {
    match row.map(|r| &r["peak_context"]) {
        None => "...".into(),
        Some(Value::Number(n)) => n.as_i64().map_or("-".into(), count),
        Some(_) => "-".into(),
    }
}

// Copied from crates/chaperone-ui/src/numbers.rs `attention`.
/// The attention types the lane has, in their order: `asked 2, nudged 1`.
fn attention(row: Option<&Value>) -> String {
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

// Copied from crates/chaperone-ui/src/numbers.rs `count`.
/// A token count, short: `850`, `12.3k`, `1.2M`.
fn count(n: i64) -> String {
    match n {
        n if n < 1000 => n.to_string(),
        n if n < 1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        n => format!("{:.1}M", n as f64 / 1e6),
    }
}

// Copied from crates/chaperone-ui/src/lane.rs `review`.
/// The newest scores, the engine's rounds and whether its review head is
/// the newest handoff's commit.
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

// Copied from crates/chaperone-ui/src/lane.rs `gates`.
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

fn timeline(page: &LanePage, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    if page.timeline.is_empty() {
        return line("No events.", muted).into_any_element();
    }
    v_flex()
        .children(page.timeline.iter().map(|t| {
            h_flex()
                .gap(px(STEP * 3.0))
                .py(px(STEP * 0.5))
                .text_sm()
                .child(div().w(px(190.0)).text_color(muted).child(t.at.clone()))
                .child(
                    div()
                        .w(px(56.0))
                        .text_color(muted)
                        .child(t.minutes.map_or("-".into(), |m| format!("+{m}m"))),
                )
                .child(div().w(px(150.0)).child(t.kind.clone()))
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .child(t.what.clone().unwrap_or_default()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t.agent.clone().unwrap_or_default()),
                )
        }))
        .into_any_element()
}

fn decisions(page: &LanePage, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    // The dossier keeps the spec's newest 20.
    let omitted = page.detail.slice.as_ref();
    let omitted = omitted.and_then(|s| s.omitted.get("slice_decisions"));
    v_flex()
        .gap(px(STEP))
        .when(page.decisions.is_empty(), |d| {
            d.child(line("No decisions recorded.", muted))
        })
        .children(page.decisions.iter().map(|d| {
            // A spec line's who and when are in its text.
            let who = d.by.clone().unwrap_or_else(|| "spec".into());
            h_flex()
                .gap(px(STEP * 3.0))
                .items_start()
                .text_sm()
                .child(div().w(px(90.0)).text_xs().text_color(muted).child(who))
                .child(div().flex_1().child(d.text.clone()))
        }))
        .children(omitted.map(|n| line(format!("Left out: the {n} oldest decisions."), muted)))
        .into_any_element()
}

fn files(files: Option<&Files>, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let (base, files) = match files {
        None => return line("Reading the files...", muted).into_any_element(),
        Some(Err(e)) => return line(e.clone(), cx.theme().danger).into_any_element(),
        Some(Ok(f)) => f,
    };
    if files.is_empty() {
        return line(format!("No changes against {base}."), muted).into_any_element();
    }
    let n = |v: Option<u64>, sign: &str| v.map_or("-".into(), |v| format!("{sign}{v}"));
    v_flex()
        .child(line(
            format!("Against {base}: committed, uncommitted and untracked."),
            muted,
        ))
        .children(files.iter().map(|c| {
            let path = match c.untracked {
                true => format!("{} (new, untracked)", c.path),
                false => c.path.clone(),
            };
            h_flex()
                .gap(px(STEP * 3.0))
                .py(px(STEP * 0.5))
                .text_sm()
                .child(div().flex_1().truncate().font_family("Menlo").child(path))
                .child(
                    div()
                        .w(px(64.0))
                        .text_color(cx.theme().success)
                        .child(n(c.added, "+")),
                )
                .child(
                    div()
                        .w(px(64.0))
                        .text_color(cx.theme().danger)
                        .child(n(c.removed, "-")),
                )
        }))
        .into_any_element()
}
