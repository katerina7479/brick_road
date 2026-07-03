//! Aurora cyan-HUD design-system: palette constants + shared widget helpers.
//!
//! Import with `use crate::theme`. Color constants are used immediately by the
//! base chrome flip; helpers are consumed by the per-surface restyle tickets
//! (br-256 settings, br-257 top bar, br-258 inspector).

use bevy_egui::egui;
use chrono::NaiveDate;
use egui::{Color32, Response, RichText, Stroke, Ui};

// ── Aurora palette ────────────────────────────────────────────────────────────
pub const BG: Color32 = Color32::from_rgb(10, 14, 16); // page / behind panels, near-black cool
pub const PANEL: Color32 = Color32::from_rgb(14, 20, 24); // panel fill (settings, inspector, ruler)
pub const PANEL_HI: Color32 = Color32::from_rgb(18, 28, 32); // raised row / chip background
pub const STROKE: Color32 = Color32::from_rgb(40, 70, 80); // dim cyan borders / separators
pub const STROKE_HI: Color32 = Color32::from_rgb(70, 120, 135); // brighter border (hover / focus)
pub const ACCENT: Color32 = Color32::from_rgb(90, 225, 220); // primary cyan — active text, borders
pub const ACCENT_GLOW: Color32 = Color32::from_rgb(130, 245, 235); // lit/active (TODAY, selected)
pub const TEXT: Color32 = Color32::from_rgb(226, 236, 238); // primary text, cool near-white
pub const TEXT_MUTED: Color32 = Color32::from_rgb(120, 140, 148); // section headers, secondary
pub const DANGER: Color32 = Color32::from_rgb(220, 92, 80); // destructive (Clear), coral-red

// ── Shared HUD widget helpers ─────────────────────────────────────────────────

/// Section header: caps label in TEXT_MUTED + optional right-aligned count in
/// ACCENT + a thin STROKE rule spanning the full row below.
pub fn section_header(ui: &mut Ui, title: &str, count: Option<usize>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(11.0).color(TEXT_MUTED));
        if let Some(n) = count {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(n.to_string()).size(10.0).color(ACCENT));
            });
        }
    });
    let avail = ui.available_width();
    if avail > 0.0 {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, 1.0), egui::Sense::hover());
        ui.painter()
            .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, STROKE));
    }
    ui.add_space(4.0);
}

/// Small monospace chip: PANEL_HI background, STROKE border, ACCENT text.
/// Used for compact data labels (`5 / 7`, date chips `07·04`).
pub fn chip(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .font(egui::FontId::monospace(11.0))
                .color(ACCENT),
        )
        .fill(PANEL_HI)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(4.0),
    )
}

/// Bordered pill button for the top bar (TODAY, Fit, Home…).
/// `active` → ACCENT_GLOW border + glow fill; inactive → dim STROKE border.
pub fn pill_button(ui: &mut Ui, label: &str, active: bool) -> Response {
    let text = RichText::new(label)
        .size(12.5)
        .color(if active { ACCENT_GLOW } else { TEXT });
    ui.add(
        egui::Button::new(text)
            .fill(if active {
                PANEL_HI
            } else {
                Color32::TRANSPARENT
            })
            .stroke(Stroke::new(
                if active { 1.5 } else { 1.0 },
                if active { ACCENT_GLOW } else { STROKE },
            ))
            .corner_radius(10.0),
    )
}

/// Compact cyan-bordered `+` add-button for add-rows (holidays, sizes, resources).
pub fn add_button(ui: &mut Ui) -> Response {
    ui.add(
        egui::Button::new(RichText::new("+").size(14.0).color(ACCENT))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, STROKE))
            .corner_radius(4.0),
    )
}

/// Bordered PANEL_HI container row for list items (holidays, sizes, resources).
/// Caller layouts content inside; returns `InnerResponse` for response chaining.
pub fn list_row<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(PANEL_HI)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, add_contents)
}

/// Monospace date chip in `YYYY · MM · DD` style — the canonical date display
/// used for start date, holidays, and per-resource time-off. Returns the chip
/// `Response` so callers can check `.clicked()` for in-place edit entry.
pub fn date_chip(ui: &mut Ui, date: NaiveDate) -> Response {
    chip(ui, &date.format("%Y · %m · %d").to_string())
}

/// Apply Aurora input styling to the current `ui` scope so `TextEdit` widgets
/// rendered within it get the PANEL_HI fill + STROKE border + ACCENT focus look.
/// Call inside `ui.scope(|ui| { style_inputs(ui); ... })` to limit the effect.
pub fn style_inputs(ui: &mut Ui) {
    let v = ui.visuals_mut();
    v.extreme_bg_color = PANEL_HI;
    v.widgets.inactive.bg_fill = PANEL_HI;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, STROKE);
    v.widgets.hovered.bg_fill = PANEL_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, STROKE_HI);
    v.widgets.active.bg_fill = PANEL_HI;
    v.widgets.active.bg_stroke = Stroke::new(1.5, ACCENT);
    v.selection.stroke = Stroke::new(1.5, ACCENT);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.25);
}

/// The accent colour marking a resource's type in the gutter and settings.
pub fn resource_type_rgb(kind: crate::model::ResourceType) -> (u8, u8, u8) {
    match kind {
        crate::model::ResourceType::Engineer => (98, 154, 224), // blue
        crate::model::ResourceType::NewHire => (140, 200, 230), // light cyan
        crate::model::ResourceType::Team => (120, 196, 140),    // green
        crate::model::ResourceType::Equipment => (224, 176, 92), // amber
        crate::model::ResourceType::Budget => (180, 150, 222),  // violet
    }
}
/// Paint the small resource-type indicator dot at `pos`.
pub fn draw_resource_dot(
    painter: &egui::Painter,
    pos: egui::Pos2,
    kind: crate::model::ResourceType,
) {
    let (r, g, b) = resource_type_rgb(kind);
    painter.circle_filled(pos, 3.5, egui::Color32::from_rgb(r, g, b));
}
