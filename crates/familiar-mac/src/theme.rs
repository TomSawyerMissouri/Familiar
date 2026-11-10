//! Colours and text styles: a dark, restrained "instrument panel" look.

use eframe::egui::{self, Color32, FontId, Stroke};

pub const BACKGROUND: Color32 = Color32::from_rgb(13, 16, 19);
pub const PANEL: Color32 = Color32::from_rgb(22, 27, 32);
pub const OUTLINE: Color32 = Color32::from_rgb(44, 52, 60);
pub const TEXT: Color32 = Color32::from_rgb(214, 221, 226);
pub const DIM: Color32 = Color32::from_rgb(110, 122, 132);
pub const ACCENT: Color32 = Color32::from_rgb(232, 163, 61);
pub const GOOD: Color32 = Color32::from_rgb(61, 220, 132);
pub const WARN: Color32 = Color32::from_rgb(235, 200, 70);
pub const BAD: Color32 = Color32::from_rgb(215, 38, 61);

/// Monospace font for readouts.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Proportional font for labels.
pub fn label(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Colour for an accuracy value: green from 85%, yellow from 65%, red below.
pub fn accuracy_color(accuracy: f64) -> Color32 {
    if accuracy >= 0.85 {
        GOOD
    } else if accuracy >= 0.65 {
        WARN
    } else {
        BAD
    }
}

/// Applies the theme to the whole app.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = PANEL;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.inactive.weak_bg_fill = PANEL;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, OUTLINE);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(32, 39, 46);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(40, 48, 56);
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
    });
}
