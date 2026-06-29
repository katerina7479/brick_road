//! Aurora-styled calendar date picker widget — single-date and date-range modes.
//!
//! Call [`aurora_date_picker`] inside an `egui::Area` or `egui::Window`; the
//! function renders the popup contents and returns `Some(result)` when the user
//! commits a selection.
//!
//! The pure calendar kernel ([`month_grid`]) is unit-tested independently of
//! the rendering.

use bevy_egui::egui;
use chrono::{Datelike, Duration, NaiveDate};
use egui::{Color32, Rangef, RichText, Stroke, Ui, Vec2};

use crate::theme;

// ── Layout constants ──────────────────────────────────────────────────────────
const CELL_W: f32 = 30.0;
const CELL_H: f32 = 24.0;
const BRACKET: f32 = 8.0; // arm length of decorative corner brackets

// ── Pure calendar kernel ──────────────────────────────────────────────────────

/// 6 × 7 calendar grid (Monday-first) for `(year, month)`.
///
/// The 42 cells span from the Monday on or before the 1st of the month through
/// the Sunday that closes the sixth week, including leading and trailing spill
/// days from adjacent months. Every cell is a valid `NaiveDate`.
pub fn month_grid(year: i32, month: u32) -> [[NaiveDate; 7]; 6] {
    let first = NaiveDate::from_ymd_opt(year, month, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 1, 1).unwrap());
    let days_from_monday = first.weekday().num_days_from_monday() as i64;
    let grid_start = first - Duration::days(days_from_monday);

    let mut grid = [[first; 7]; 6];
    let mut cur = grid_start;
    for row in grid.iter_mut() {
        for cell in row.iter_mut() {
            *cell = cur;
            cur += Duration::days(1);
        }
    }
    grid
}

/// Advance (positive) or retreat (negative) the displayed month by `delta`.
pub fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + month as i32 - 1 + delta;
    let y = total.div_euclid(12);
    let m = (total.rem_euclid(12) + 1) as u32;
    (y, m)
}

// ── Widget state & result ─────────────────────────────────────────────────────

/// Persistent state threaded through [`aurora_date_picker`] across frames.
pub struct DatePickerState {
    /// Year and month currently displayed in the grid.
    pub shown_month: (i32, u32),
    /// `false` = single-date mode, `true` = range mode.
    pub mode_range: bool,
    /// Selected date (single) or range start.
    pub start: NaiveDate,
    /// Range end — `None` while awaiting the second click.
    pub end: Option<NaiveDate>,
    /// Range mode: `true` when the next click sets `end`.
    pub picking_end: bool,
}

impl DatePickerState {
    pub fn single(date: NaiveDate) -> Self {
        Self {
            shown_month: (date.year(), date.month()),
            mode_range: false,
            start: date,
            end: None,
            picking_end: false,
        }
    }

    pub fn range(start: NaiveDate, end: Option<NaiveDate>) -> Self {
        Self {
            shown_month: (start.year(), start.month()),
            mode_range: true,
            start,
            end,
            picking_end: end.is_some(),
        }
    }
}

/// Committed result from [`aurora_date_picker`].
#[derive(Debug, Clone, PartialEq)]
pub enum DatePickerResult {
    Single(NaiveDate),
    Range(NaiveDate, NaiveDate),
}

// ── Rendering ─────────────────────────────────────────────────────────────────

/// Aurora calendar date picker. Renders into `ui` (caller wraps in popup).
///
/// Returns `Some(result)` when the user commits:
/// - single mode: any day click
/// - range mode: end date chosen (second click)
pub fn aurora_date_picker(ui: &mut Ui, state: &mut DatePickerState) -> Option<DatePickerResult> {
    let mut result = None;
    let (year, month) = state.shown_month;
    let today = today_date();

    let popup = egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::STROKE))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(CELL_W * 7.0 + 4.0);
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);

            // Title
            let title = if state.mode_range {
                "Select range"
            } else {
                "Pick a date"
            };
            ui.label(RichText::new(title).size(11.0).color(theme::TEXT_MUTED));
            ui.add_space(4.0);

            // Chip header
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if state.mode_range {
                    date_chip_active(ui, state.start, !state.picking_end);
                    ui.label(RichText::new("→").size(12.0).color(theme::TEXT_MUTED));
                    if let Some(end) = state.end {
                        date_chip_active(ui, end, state.picking_end);
                    } else {
                        ui.label(RichText::new("···").size(12.0).color(theme::TEXT_MUTED));
                    }
                } else {
                    date_chip_active(ui, state.start, true);
                }
            });
            ui.add_space(8.0);

            // Month navigation
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if nav_arrow(ui, "‹") {
                    state.shown_month = shift_month(year, month, -1);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if nav_arrow(ui, "›") {
                        state.shown_month = shift_month(year, month, 1);
                    }
                    let label = NaiveDate::from_ymd_opt(year, month, 1)
                        .unwrap_or_default()
                        .format("%B %Y")
                        .to_string();
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        ui.label(RichText::new(label).size(12.0).color(theme::TEXT));
                    });
                });
            });
            ui.add_space(4.0);

            // Weekday headers
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for hdr in ["M", "T", "W", "T", "F", "S", "S"] {
                    ui.add_sized(
                        Vec2::new(CELL_W, 16.0),
                        egui::Label::new(RichText::new(hdr).size(10.0).color(theme::TEXT_MUTED)),
                    );
                }
            });
            ui.add_space(2.0);

            // Day grid
            let grid = month_grid(year, month);
            for row in &grid {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for &day in row {
                        let in_month = day.month() == month && day.year() == year;
                        let is_start = day == state.start;
                        let is_end = state.end.is_some_and(|e| day == e);
                        let is_today = day == today;

                        let in_range = if state.mode_range {
                            if let Some(end) = state.end {
                                let (lo, hi) = if state.start <= end {
                                    (state.start, end)
                                } else {
                                    (end, state.start)
                                };
                                day > lo && day < hi
                            } else {
                                false
                            }
                        } else {
                            false
                        };

                        let is_selected = is_start || is_end;

                        let fill = if is_selected {
                            theme::ACCENT.gamma_multiply(0.22)
                        } else if in_range {
                            theme::ACCENT.gamma_multiply(0.08)
                        } else {
                            Color32::TRANSPARENT
                        };

                        let (stroke_w, stroke_col) = if is_selected {
                            (1.0, theme::ACCENT)
                        } else if is_today {
                            (1.0, theme::STROKE_HI)
                        } else {
                            (0.0, Color32::TRANSPARENT)
                        };

                        let text_color = if is_selected {
                            theme::ACCENT_GLOW
                        } else if !in_month {
                            theme::TEXT_MUTED
                        } else if is_today {
                            theme::ACCENT
                        } else {
                            theme::TEXT
                        };

                        let resp = ui.add_sized(
                            Vec2::new(CELL_W, CELL_H),
                            egui::Button::new(
                                RichText::new(day.day().to_string())
                                    .size(12.0)
                                    .color(text_color),
                            )
                            .fill(fill)
                            .stroke(Stroke::new(stroke_w, stroke_col))
                            .corner_radius(4.0),
                        );

                        if resp.clicked() {
                            if let Some(r) = handle_click(state, day) {
                                result = Some(r);
                            }
                        }
                    }
                });
            }

            ui.add_space(8.0);

            // TODAY button
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if theme::pill_button(ui, "TODAY", false).clicked() {
                    state.shown_month = (today.year(), today.month());
                    if !state.mode_range {
                        state.start = today;
                        result = Some(DatePickerResult::Single(today));
                    }
                }
            });
        });

    // Corner brackets drawn on the outer frame rect after layout is finalized.
    let frame_rect = popup.response.rect;
    draw_corner_brackets(ui.painter(), frame_rect.shrink(3.0));

    result
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Date chip with an active/inactive highlight — active gets `ACCENT` border.
fn date_chip_active(ui: &mut Ui, date: NaiveDate, active: bool) -> egui::Response {
    let (stroke_w, stroke_col, text_col) = if active {
        (1.5, theme::ACCENT, theme::ACCENT_GLOW)
    } else {
        (1.0, theme::STROKE, theme::TEXT_MUTED)
    };
    ui.add(
        egui::Button::new(
            RichText::new(date.format("%Y · %m · %d").to_string())
                .font(egui::FontId::monospace(11.0))
                .color(text_col),
        )
        .fill(theme::PANEL_HI)
        .stroke(Stroke::new(stroke_w, stroke_col))
        .corner_radius(4.0),
    )
}

/// Compact nav arrow button (‹ or ›).
fn nav_arrow(ui: &mut Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(16.0).color(theme::ACCENT))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(4.0),
    )
    .clicked()
}

/// Process a day-cell click, updating `state` and returning a committed result
/// if the selection is complete.
fn handle_click(state: &mut DatePickerState, day: NaiveDate) -> Option<DatePickerResult> {
    if !state.mode_range {
        state.start = day;
        state.shown_month = (day.year(), day.month());
        return Some(DatePickerResult::Single(day));
    }
    if !state.picking_end {
        state.start = day;
        state.end = None;
        state.picking_end = true;
    } else {
        let (start, end) = if day >= state.start {
            (state.start, day)
        } else {
            (day, state.start)
        };
        state.start = start;
        state.end = Some(end);
        state.picking_end = false;
        return Some(DatePickerResult::Range(start, end));
    }
    None
}

/// Draw four L-shaped corner brackets on `rect` in `STROKE_HI`.
fn draw_corner_brackets(painter: &egui::Painter, rect: egui::Rect) {
    let s = Stroke::new(1.5, theme::STROKE_HI);
    let b = BRACKET;
    let min = rect.min;
    let max = rect.max;
    // Top-left
    painter.hline(Rangef::new(min.x, min.x + b), min.y, s);
    painter.vline(min.x, Rangef::new(min.y, min.y + b), s);
    // Top-right
    painter.hline(Rangef::new(max.x - b, max.x), min.y, s);
    painter.vline(max.x, Rangef::new(min.y, min.y + b), s);
    // Bottom-left
    painter.hline(Rangef::new(min.x, min.x + b), max.y, s);
    painter.vline(min.x, Rangef::new(max.y - b, max.y), s);
    // Bottom-right
    painter.hline(Rangef::new(max.x - b, max.x), max.y, s);
    painter.vline(max.x, Rangef::new(max.y - b, max.y), s);
}

/// Current date from the system clock (no chrono `clock` feature required).
fn today_date() -> NaiveDate {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::calendar::unix_secs_to_date(secs)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_grid_first_cell_is_monday_on_or_before_first() {
        // January 2025: the 1st is a Wednesday, so grid starts on Mon Dec 30 2024.
        let grid = month_grid(2025, 1);
        assert_eq!(
            grid[0][0],
            NaiveDate::from_ymd_opt(2024, 12, 30).unwrap(),
            "Jan 2025 grid must start Mon 2024-12-30"
        );
    }

    #[test]
    fn month_grid_last_cell_is_sunday() {
        let grid = month_grid(2025, 1);
        let last = grid[5][6];
        use chrono::Weekday;
        assert_eq!(
            last.weekday(),
            Weekday::Sun,
            "last cell must be a Sunday, got {last}"
        );
    }

    #[test]
    fn month_grid_contains_42_consecutive_days() {
        let grid = month_grid(2025, 3);
        let flat: Vec<NaiveDate> = grid.iter().flat_map(|r| r.iter().copied()).collect();
        for w in flat.windows(2) {
            assert_eq!(
                w[1] - w[0],
                Duration::days(1),
                "cells must be consecutive days"
            );
        }
        assert_eq!(flat.len(), 42);
    }

    #[test]
    fn month_grid_leap_february_2000() {
        // Feb 2000 is a leap year: 29 days. 1st is a Tuesday → grid starts Mon Jan 31.
        let grid = month_grid(2000, 2);
        assert_eq!(grid[0][0], NaiveDate::from_ymd_opt(2000, 1, 31).unwrap());
        // Feb 29 must appear.
        let flat: Vec<NaiveDate> = grid.iter().flat_map(|r| r.iter().copied()).collect();
        assert!(
            flat.contains(&NaiveDate::from_ymd_opt(2000, 2, 29).unwrap()),
            "leap day 2000-02-29 must be in grid"
        );
    }

    #[test]
    fn month_grid_year_boundary_december() {
        // Dec 2024: 1st is a Sunday → grid starts Mon Nov 25.
        let grid = month_grid(2024, 12);
        assert_eq!(grid[0][0], NaiveDate::from_ymd_opt(2024, 11, 25).unwrap());
        // Last cell: Dec 31 is Tuesday → grid fills to Sun Jan 5 2025.
        let last = grid[5][6];
        assert_eq!(last, NaiveDate::from_ymd_opt(2025, 1, 5).unwrap());
    }

    #[test]
    fn shift_month_forward_wraps_december() {
        assert_eq!(shift_month(2024, 12, 1), (2025, 1));
    }

    #[test]
    fn shift_month_backward_wraps_january() {
        assert_eq!(shift_month(2025, 1, -1), (2024, 12));
    }

    #[test]
    fn shift_month_multi_step() {
        assert_eq!(shift_month(2025, 11, 3), (2026, 2));
    }

    #[test]
    fn handle_click_single_returns_result() {
        let date = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let mut state = DatePickerState::single(date);
        let new_date = NaiveDate::from_ymd_opt(2025, 6, 20).unwrap();
        let result = handle_click(&mut state, new_date);
        assert_eq!(result, Some(DatePickerResult::Single(new_date)));
        assert_eq!(state.start, new_date);
    }

    #[test]
    fn handle_click_range_first_click_sets_start_no_result() {
        let date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        let mut state = DatePickerState::range(date, None);
        state.picking_end = false;
        let clicked = NaiveDate::from_ymd_opt(2025, 6, 10).unwrap();
        let result = handle_click(&mut state, clicked);
        assert!(result.is_none(), "first click must not commit");
        assert_eq!(state.start, clicked);
        assert!(state.picking_end);
    }

    #[test]
    fn handle_click_range_second_click_commits() {
        let start = NaiveDate::from_ymd_opt(2025, 6, 5).unwrap();
        let mut state = DatePickerState::range(start, None);
        state.picking_end = true;
        let end = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let result = handle_click(&mut state, end);
        assert_eq!(result, Some(DatePickerResult::Range(start, end)));
    }

    #[test]
    fn handle_click_range_swaps_if_end_before_start() {
        let start = NaiveDate::from_ymd_opt(2025, 6, 20).unwrap();
        let mut state = DatePickerState::range(start, None);
        state.picking_end = true;
        let earlier = NaiveDate::from_ymd_opt(2025, 6, 10).unwrap();
        let result = handle_click(&mut state, earlier);
        assert_eq!(
            result,
            Some(DatePickerResult::Range(earlier, start)),
            "end < start must swap so start <= end"
        );
    }
}
