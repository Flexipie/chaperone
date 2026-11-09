//! The change graph: per repo, each lane's changed files, the files that
//! depend on them (`impact`) and the entry points they reach, laid out in
//! columns by depth; and one lane's own, with a column of the entry points
//! it reaches. The layout depends only on what it is built from, so it
//! stays put until a lane commits.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chaperone_graph::Impact;
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2, pos2, vec2};

/// Dependents drawn; deeper ones are counted.
const DEPTH: usize = 3;
const COLUMN: f32 = 260.0;
const ROW: f32 = 40.0;
const MARGIN: f32 = 24.0;
/// By lane id, so a lane keeps its colour while others come and go.
const PALETTE: [Color32; 8] = [
    Color32::from_rgb(66, 135, 245),
    Color32::from_rgb(230, 120, 40),
    Color32::from_rgb(60, 170, 90),
    Color32::from_rgb(200, 60, 140),
    Color32::from_rgb(150, 100, 220),
    Color32::from_rgb(200, 170, 30),
    Color32::from_rgb(40, 170, 180),
    Color32::from_rgb(170, 90, 60),
];
/// Fill strength by depth: changed files full, dependents fading.
const FADE: [f32; DEPTH + 1] = [1.0, 0.55, 0.35, 0.22];

/// Each repo's layout with its slug, in slug order.
pub type Repos = Vec<(String, Arc<Layout>)>;

/// One lane's part of the graph.
pub struct Lane {
    pub id: i64,
    /// The files it changes (overlap's paths) with lines added and removed
    /// since the merge-base, in commits only; 0 when git has no count.
    pub files: Vec<(String, u64)>,
    /// The dependents and entry points of `files`, or why there are none.
    pub impact: Result<Impact, String>,
    /// Why some of the lane may be missing.
    pub notes: Vec<String>,
}

struct Node {
    path: String,
    module: Option<String>,
    pos: Pos2,
    radius: f32,
    /// Nearest: 0 for a changed file.
    depth: usize,
    /// The lane it is coloured by: the nearest, the lowest id on a tie.
    lane: i64,
    /// Every lane changing or reaching it, by id.
    lanes: Vec<i64>,
    changed_by: Vec<i64>,
    lines: u64,
    /// `kind name` of each entry point at this file.
    entries: BTreeSet<String>,
    /// In a lane's own graph: the other open lanes changing it.
    others: Vec<i64>,
}

/// An entry point in a lane's graph, joined to its file's node.
struct Command {
    label: String,
    pos: Pos2,
    from: usize,
}

/// How a layout is drawn: the repo-wide graph uses the default.
#[derive(Default)]
pub struct Opts<'a> {
    /// Label files by path, not module.
    pub paths: bool,
    /// Ringed red: out of the lane's declared areas.
    pub out_of_scope: BTreeSet<&'a str>,
    /// Changed files the layout lacks, drawn under its changed column.
    pub extra: Vec<&'a str>,
}

/// One repo's graph, positions relative to its own top left.
pub struct Layout {
    nodes: Vec<Node>,
    /// Entry-point evidence hops, as node indexes; impact gives depths,
    /// not edges, so these are the only ones drawn.
    edges: BTreeSet<(usize, usize)>,
    /// A lane's graph only: the entry points it reaches, in a last column.
    commands: Vec<Command>,
    /// What the drawing may be missing.
    pub notes: Vec<String>,
    size: Vec2,
}

#[derive(Default)]
struct Acc {
    module: Option<String>,
    /// Lane to its nearest depth.
    depth: BTreeMap<i64, usize>,
    changed_by: Vec<i64>,
    lines: u64,
    entries: BTreeSet<String>,
}

/// `lane` reaches `path` at `depth`; the nearest depth is kept.
fn reach<'a, 'm>(
    acc: &'m mut BTreeMap<&'a str, Acc>,
    path: &'a str,
    lane: i64,
    depth: usize,
) -> &'m mut Acc {
    let a = acc.entry(path).or_default();
    let d = a.depth.entry(lane).or_insert(depth);
    *d = (*d).min(depth);
    a
}

/// Lays out `lanes` (one repo's). The same lanes give the same layout.
pub fn layout(lanes: &[&Lane]) -> Layout {
    let mut lanes = lanes.to_vec();
    lanes.sort_by_key(|l| l.id);
    let mut acc: BTreeMap<&str, Acc> = BTreeMap::new();
    let mut hops = Vec::new();
    let mut notes = Vec::new();
    for lane in &lanes {
        let id = lane.id;
        for (path, lines) in &lane.files {
            let a = reach(&mut acc, path, id, 0);
            a.changed_by.push(id);
            a.lines = a.lines.max(*lines);
        }
        let mut note = lane.notes.clone();
        let impact = match &lane.impact {
            Ok(i) => i,
            Err(why) => {
                note.push(format!("no impact: {why}"));
                notes.push(format!("#{id}: {}", note.join("; ")));
                continue;
            }
        };
        for t in &impact.targets {
            if let Some(a) = acc.get_mut(t.path.as_str()) {
                a.module = a.module.take().or(t.module.clone());
            }
        }
        let mut deeper = impact.files_omitted;
        for d in &impact.depths {
            if d.depth > DEPTH {
                deeper += d.files.len();
                continue;
            }
            for f in &d.files {
                let a = reach(&mut acc, &f.path, id, d.depth);
                a.module = a.module.take().or(f.module.clone());
            }
        }
        if impact.bound != "exact" {
            note.push("dependents are a lower bound".into());
        }
        if impact.stale {
            note.push("some files not read".into());
        }
        if deeper > 0 {
            note.push(format!("{deeper} dependents deeper than {DEPTH} or cut"));
        }
        match &impact.entry_points {
            Some(ep) => {
                for e in ep.reached.iter().filter(|e| e.depth <= DEPTH) {
                    reach(&mut acc, &e.path, id, e.depth)
                        .entries
                        .insert(format!("{} {}", e.kind, e.name));
                    hops.extend(e.evidence.windows(2).map(|w| (&w[0].path, &w[1].path)));
                }
                if !ep.unmatched.is_empty() || ep.omitted > 0 {
                    note.push("entry points are a lower bound".into());
                }
            }
            None => note.push("no entry points".into()),
        }
        if !note.is_empty() {
            notes.push(format!("#{id}: {}", note.join("; ")));
        }
    }

    // Columns by depth; in each, rows by colour lane, then path.
    let mut nodes: Vec<Node> = acc
        .into_iter()
        .map(|(path, a)| {
            let (lane, depth) = a
                .depth
                .iter()
                .min_by_key(|(id, d)| (**d, **id))
                .map(|(id, d)| (*id, *d))
                .unwrap_or_default();
            let radius = match depth {
                0 => (6.0 + 2.0 * (a.lines as f32).sqrt()).min(14.0),
                _ => 5.0,
            };
            Node {
                path: path.to_string(),
                module: a.module,
                pos: Pos2::ZERO,
                radius,
                depth,
                lane,
                lanes: a.depth.keys().copied().collect(),
                changed_by: a.changed_by,
                lines: a.lines,
                entries: a.entries,
                others: Vec::new(),
            }
        })
        .collect();
    nodes.sort_by(|a, b| (a.depth, a.lane, &a.path).cmp(&(b.depth, b.lane, &b.path)));
    let mut rows = [0usize; DEPTH + 1];
    for n in &mut nodes {
        let row = &mut rows[n.depth];
        n.pos = pos2(MARGIN + n.depth as f32 * COLUMN, MARGIN + *row as f32 * ROW);
        *row += 1;
    }
    let index: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.path.as_str(), i))
        .collect();
    let edges = hops
        .into_iter()
        .filter_map(|(a, b)| Some((*index.get(a.as_str())?, *index.get(b.as_str())?)))
        .collect();
    let tallest = rows.iter().copied().max().unwrap_or(0);
    let columns = rows.iter().filter(|r| **r > 0).count();
    Layout {
        nodes,
        edges,
        commands: Vec::new(),
        notes,
        size: vec2(
            2.0 * MARGIN + columns as f32 * COLUMN,
            2.0 * MARGIN + tallest as f32 * ROW,
        ),
    }
}

/// One lane's graph: `layout` of it, the files `others` (path to other
/// lanes' ids) change too, and a last column of the entry points reached
/// at the depths drawn, each joined to its file.
pub fn lane_layout(lane: &Lane, others: &BTreeMap<String, Vec<i64>>) -> Layout {
    let mut l = layout(&[lane]);
    for n in &mut l.nodes {
        n.others = others.get(&n.path).cloned().unwrap_or_default();
    }
    let reached = lane.impact.iter().filter_map(|i| i.entry_points.as_ref());
    let deepest = l.nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    let x = MARGIN + (deepest + 1) as f32 * COLUMN;
    for e in reached
        .flat_map(|ep| &ep.reached)
        .filter(|e| e.depth <= DEPTH)
    {
        if let Some(from) = l.nodes.iter().position(|n| n.path == e.path) {
            let y = MARGIN + l.commands.len() as f32 * ROW;
            l.commands.push(Command {
                label: format!("{} {}", e.kind, e.name),
                pos: pos2(x, y),
                from,
            });
        }
    }
    if !l.commands.is_empty() {
        l.size.x = l.size.x.max(x + COLUMN + MARGIN);
        l.size.y = l.size.y.max(2.0 * MARGIN + l.commands.len() as f32 * ROW);
    }
    l
}

impl Layout {
    /// `path` is a changed file (depth 0), its dependents read.
    pub fn changes(&self, path: &str) -> bool {
        self.nodes.iter().any(|n| n.depth == 0 && n.path == path)
    }
}

fn colour(lane: i64) -> Color32 {
    PALETTE[lane.rem_euclid(PALETTE.len() as i64) as usize]
}

/// Every repo's graph, one below the other.
pub fn draw(ui: &mut egui::Ui, repos: &Repos) {
    if repos.is_empty() {
        ui.label("No open lanes with changes.");
        return;
    }
    egui::ScrollArea::both().id_salt("graph").show(ui, |ui| {
        for (slug, layout) in repos {
            ui.heading(slug);
            ui.label("Changed files at each lane's head, then their dependents by depth.");
            for note in &layout.notes {
                ui.colored_label(ui.visuals().warn_fg_color, note);
            }
            draw_one(ui, layout, &Opts::default());
            ui.separator();
        }
    });
}

pub fn draw_one(ui: &mut egui::Ui, layout: &Layout, opts: &Opts) {
    // The extra rows go below the tallest column, clear of every label.
    let below = ((layout.size.y - 2.0 * MARGIN) / ROW).round() as usize;
    let mut size = layout.size;
    size.y += opts.extra.len() as f32 * ROW;
    let text = ui.visuals().text_color();
    let font = egui::TextStyle::Small.resolve(ui.style());
    let label = |n: &Node| {
        // The module names a file across crates; `src/lib.rs` would not.
        let name = match opts.paths {
            true => &n.path,
            false => n.module.as_deref().unwrap_or(&n.path),
        };
        match n.entries.first() {
            Some(e) => format!("{name}  [{e}]"),
            None => name.to_string(),
        }
    };
    let extra = |p: &str| format!("{p}  (not in the graph yet, dependents not read)");
    // As wide as the widest label: the painter clips to its rect.
    let width = |s: String| ui.painter().layout_no_wrap(s, font.clone(), text).size().x;
    let right = layout
        .nodes
        .iter()
        .map(|n| n.pos.x + n.radius + 8.0 + width(label(n)))
        .chain(
            layout
                .commands
                .iter()
                .map(|c| c.pos.x + 14.0 + width(c.label.clone())),
        )
        .chain(opts.extra.iter().map(|p| MARGIN + 22.0 + width(extra(p))))
        .fold(0.0, f32::max);
    size.x = size.x.max(right + MARGIN);
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    let origin = response.rect.min.to_vec2();
    let faint = ui.visuals().weak_text_color();
    let red = Stroke::new(2.0, ui.visuals().error_fg_color);
    let diamond = |at: Pos2, r: f32| {
        let corners = [vec2(0.0, -r), vec2(r, 0.0), vec2(0.0, r), vec2(-r, 0.0)];
        let points = corners.iter().map(|c| at + *c).collect();
        egui::Shape::closed_line(points, Stroke::new(1.5, text))
    };
    for &(a, b) in &layout.edges {
        let (a, b) = (&layout.nodes[a], &layout.nodes[b]);
        painter.line_segment([a.pos + origin, b.pos + origin], Stroke::new(1.0, faint));
    }
    for c in &layout.commands {
        let (from, at) = (layout.nodes[c.from].pos + origin, c.pos + origin);
        painter.line_segment([from, at], Stroke::new(1.0, faint));
        painter.add(diamond(at, 6.0));
        let at = at + vec2(14.0, 0.0);
        painter.text(at, egui::Align2::LEFT_CENTER, &c.label, font.clone(), text);
    }
    for (row, path) in (below..).zip(&opts.extra) {
        let at = pos2(MARGIN, MARGIN + row as f32 * ROW) + origin;
        painter.circle_stroke(at, 6.0, Stroke::new(1.5, faint));
        painter.circle_stroke(at, 14.0, red);
        painter.text(
            at + vec2(22.0, 0.0),
            egui::Align2::LEFT_CENTER,
            extra(path),
            font.clone(),
            text,
        );
    }
    let mut hovered = None;
    for (i, n) in layout.nodes.iter().enumerate() {
        let at = n.pos + origin;
        let fill = colour(n.lane).gamma_multiply(FADE[n.depth]);
        painter.circle_filled(at, n.radius, fill);
        if n.lanes.len() > 1 || !n.others.is_empty() {
            let width = if n.changed_by.len() > 1 { 3.0 } else { 1.5 };
            painter.circle_stroke(at, n.radius + 3.0, Stroke::new(width, text));
        }
        if opts.out_of_scope.contains(n.path.as_str()) {
            painter.circle_stroke(at, n.radius + 8.0, red);
        }
        if !n.entries.is_empty() {
            painter.add(diamond(at, n.radius + 6.0));
        }
        let colour = if n.depth == 0 { text } else { faint };
        painter.text(
            at + vec2(n.radius + 8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label(n),
            font.clone(),
            colour,
        );
        if response
            .hover_pos()
            .is_some_and(|p| p.distance(at) <= n.radius + 4.0)
        {
            hovered = Some(i);
        }
    }
    if let Some(n) = hovered.map(|i| &layout.nodes[i]) {
        response.on_hover_ui_at_pointer(|ui| {
            ui.strong(&n.path);
            if let Some(m) = &n.module {
                ui.monospace(m);
            }
            let ids = |l: &[i64]| l.iter().map(|i| format!("#{i}")).collect::<Vec<_>>();
            if !n.changed_by.is_empty() {
                ui.label(format!(
                    "changed by {}, {} lines",
                    ids(&n.changed_by).join(", "),
                    n.lines
                ));
            }
            ui.label(format!(
                "depth {} from {}",
                n.depth,
                ids(&n.lanes).join(", ")
            ));
            for e in &n.entries {
                ui.label(format!("entry point: {e}"));
            }
            if !n.others.is_empty() {
                ui.label(format!("also changed by {}", ids(&n.others).join(", ")));
            }
            if opts.out_of_scope.contains(n.path.as_str()) {
                ui.label("out of the declared areas");
            }
        });
    }
}
