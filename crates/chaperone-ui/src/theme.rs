//! The look: translucent panels over a dark ground, soft shadows, round
//! corners (#109). egui has no backdrop blur, so the glass is a faint
//! white fill, a hairline edge and a shadow over the window's own ground.

use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, RichText, Shadow, Stroke, Theme, Visuals,
};

/// The window's ground, behind every panel.
pub const GROUND: Color32 = Color32::from_rgb(11, 14, 21);
pub const ACCENT: Color32 = Color32::from_rgb(10, 132, 255);
/// The needs-you badge.
pub const ALERT: Color32 = Color32::from_rgb(255, 159, 10);
const RADIUS: u8 = 14;

/// Dark always: the glass only reads over a dark ground.
pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = Color32::from_rgb(24, 28, 38);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.window_stroke = edge();
    v.faint_bg_color = Color32::from_white_alpha(6);
    v.extreme_bg_color = Color32::from_rgb(7, 9, 14);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.widgets.noninteractive.bg_stroke = edge();
    v.widgets.noninteractive.fg_stroke.color = Color32::from_gray(185);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_white_alpha(12);
    v.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(24);
    v.widgets.active.weak_bg_fill = Color32::from_white_alpha(32);
    ctx.set_theme(Theme::Dark);
    ctx.set_visuals_of(Theme::Dark, v);
}

/// The clear colour under the panels, as eframe wants it.
pub fn ground_rgba() -> [f32; 4] {
    egui::Rgba::from(GROUND).to_array()
}

fn edge() -> Stroke {
    Stroke::new(1.0, Color32::from_white_alpha(22))
}

/// A floating glass panel.
pub fn glass() -> Frame {
    Frame::new()
        .fill(Color32::from_white_alpha(9))
        .stroke(edge())
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
        .outer_margin(Margin::same(6))
        .shadow(Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
}

/// The main view: the ground itself, so its cards sit on the dark.
pub fn ground() -> Frame {
    Frame::new().inner_margin(Margin::same(14))
}

/// A small count pill; nothing for zero.
pub fn badge(ui: &mut egui::Ui, n: usize, colour: Color32) {
    if n == 0 {
        return;
    }
    Frame::new()
        .fill(colour.gamma_multiply(0.25))
        .stroke(Stroke::new(1.0, colour.gamma_multiply(0.7)))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(n.to_string()).small().strong().color(colour))
        });
}
