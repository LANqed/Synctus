//! The Synctus look: one accent colour everywhere.
//!
//! The brand colour is `#39e6e5` — the teal used for focus bars, the tray icon,
//! the WebUI and the Android app. It is defined here once and the platform layers
//! read it, so changing the theme is a one-line change instead of a hunt.

use eframe::egui;

/// The brand accent. `#39e6e5` in sRGB.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x39, 0xe6, 0xe5);

/// Apply the accent to egui's built-in widgets.
pub fn install(ctx: &egui::Context) {
    let mut visuals = ctx.global_style().visuals.clone();

    // Widgets (buttons, checkboxes, sliders) in their various states.
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    }
    visuals.widgets.hovered.weak_bg_fill = ACCENT.gamma_multiply(0.25);
    visuals.widgets.hovered.bg_fill = ACCENT.gamma_multiply(0.25);
    visuals.widgets.active.bg_fill = ACCENT.gamma_multiply(0.35);
    visuals.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.25);

    // Selection: checkmarks, highlighted rows.
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);

    // Links and progress.
    visuals.hyperlink_color = ACCENT;

    ctx.set_visuals(visuals);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accent_is_the_brand_colour() {
        assert_eq!(ACCENT, egui::Color32::from_rgb(0x39, 0xe6, 0xe5));
    }
}
