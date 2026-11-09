//! The look, from three tokens the way Linear builds its themes: a base
//! (the ground), an accent (selection, focus, the primary action) and a
//! contrast (how far text and edges step away from the base). Every other
//! colour is derived here and written onto GPUI Kit's theme, so views read
//! `cx.theme()` and never name a colour.

use gpui::{App, Hsla, px};
use gpui_component::{Theme, ThemeMode, oklch};

/// An OKLCH colour: lightness 0 to 1, chroma, hue in degrees.
#[derive(Clone, Copy)]
pub struct Lch(pub f32, pub f32, pub f32);

pub struct Tokens {
    pub base: Lch,
    pub accent: Lch,
    /// 0 is no contrast, 1 the most the base allows.
    pub contrast: f32,
}

/// Dark, a cool grey ground and an indigo accent.
pub const DARK: Tokens = Tokens {
    base: Lch(0.18, 0.006, 275.0),
    accent: Lch(0.62, 0.16, 277.0),
    contrast: 0.7,
};

/// The spacing step: every gap and padding is a multiple of it.
pub const STEP: f32 = 4.0;

/// The needs-you badge and its words.
pub fn alert() -> Hsla {
    oklch(0.78, 0.15, 70.0)
}

/// Writes `tokens` onto GPUI Kit's theme, dark mode.
pub fn apply(tokens: &Tokens, cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let Lch(l, c, h) = tokens.base;
    let k = tokens.contrast;
    // A surface `n` steps above the base.
    let up = |n: f32| oklch(l + 0.025 * n * k, c, h);
    let text = oklch(l + (0.97 - l) * (0.6 + 0.4 * k), c * 0.5, h);
    let muted_text = oklch(l + (0.97 - l) * (0.35 + 0.3 * k), c, h);
    let edge = oklch(l + 0.09 * k, c, h);
    let Lch(al, ac, ah) = tokens.accent;
    let accent = oklch(al, ac, ah);
    Theme::update(cx, |t| {
        t.font_family = ".SystemUIFont".into();
        t.font_size = px(13.0);
        t.radius = px(STEP * 1.5);
        t.radius_lg = px(STEP * 2.0);
        t.background = up(0.0);
        t.foreground = text;
        t.border = edge;
        t.muted = up(1.0);
        t.muted_foreground = muted_text;
        // GPUI Kit's `accent` is the hover fill, not a brand colour.
        t.accent = up(2.0);
        t.accent_foreground = text;
        t.primary = accent;
        t.primary_foreground = oklch(0.99, 0.0, 0.0);
        t.ring = accent;
        t.list_active = accent.opacity(0.2);
        t.sidebar = up(-0.6);
        t.sidebar_border = edge;
        t.sidebar_foreground = muted_text;
        t.sidebar_accent = up(2.0);
        t.sidebar_accent_foreground = text;
        t.sidebar_primary = accent;
        t.table = up(0.0);
        t.table_even = up(0.0);
        t.table_head = up(0.0);
        t.table_head_foreground = muted_text;
        t.table_hover = up(1.0);
        t.table_active = accent.opacity(0.18);
        t.table_row_border = edge.opacity(0.6);
        t.title_bar = up(-0.6);
        t.title_bar_border = edge;
        t.warning = alert();
    });
}
