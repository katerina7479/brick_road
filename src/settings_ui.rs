//! The right-side settings fly-out: calendar, holidays, per-resource time-off,
//! and t-shirt sizes — dense grid, in-place editing, Aurora date pickers.
//! Extracted from main.rs (#340) — a pure move, no behavior change.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::{datepicker, db, model, theme};

// ── Settings fly-out layout constants ────────────────────────────────────────
const SETTINGS_SECTION_GAP: f32 = 16.0; // between sections
const SETTINGS_ROW_GAP: f32 = 6.0; // between rows within a section
const SETTINGS_LABEL_COL: f32 = 116.0; // fixed left label column width

/// One config row: fixed-width left label + right-aligned control.
fn settings_row(ui: &mut egui::Ui, label: &str, add_control: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [SETTINGS_LABEL_COL, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(label).color(theme::TEXT_MUTED)),
        );
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            add_control,
        );
    });
}
/// A run of consecutive calendar days that share a label — one logical holiday.
struct HolidayGroup {
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
    description: String,
    dates: Vec<chrono::NaiveDate>,
}
/// Groups non-working dates into runs of consecutive calendar days that share a
/// description, so a multi-day holiday stored as N daily rows shows and removes
/// as one entry. Result is ordered by start date.
fn group_holidays(dates: &[model::NonWorkingDate]) -> Vec<HolidayGroup> {
    let mut sorted = dates.to_vec();
    sorted.sort_by_key(|nwd| nwd.date);
    let mut groups: Vec<HolidayGroup> = Vec::new();
    for nwd in sorted {
        if let Some(g) = groups.last_mut() {
            if g.description == nwd.description && g.end.succ_opt() == Some(nwd.date) {
                g.end = nwd.date;
                g.dates.push(nwd.date);
                continue;
            }
        }
        groups.push(HolidayGroup {
            start: nwd.date,
            end: nwd.date,
            description: nwd.description.clone(),
            dates: vec![nwd.date],
        });
    }
    groups
}
/// All calendar days in `[start, end]` inclusive, with `end` clamped to `>= start`
/// and to at most a year past `start` (so a mis-picked span can't insert thousands
/// of daily rows). This is the expansion behind a multi-day holiday / time-off span.
fn expand_date_range(start: chrono::NaiveDate, end: chrono::NaiveDate) -> Vec<chrono::NaiveDate> {
    let end = end.max(start).min(start + chrono::Duration::days(366));
    let mut out = Vec::new();
    let mut d = start;
    loop {
        out.push(d);
        if d >= end {
            break;
        }
        match d.succ_opt() {
            Some(n) => d = n,
            None => break,
        }
    }
    out
}
/// Re-anchor a date group: drop its current `old_dates` from `dates` and insert
/// the expanded `[start, end]` span, all carrying `desc`, de-duplicated against
/// the survivors. Shared by the holiday and per-resource time-off editors.
fn set_date_range(
    dates: &mut Vec<model::NonWorkingDate>,
    old_dates: &[chrono::NaiveDate],
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
    desc: &str,
) {
    dates.retain(|x| !old_dates.contains(&x.date));
    for d in expand_date_range(start, end) {
        if !dates.iter().any(|x| x.date == d) {
            dates.push(model::NonWorkingDate {
                date: d,
                description: desc.to_string(),
            });
        }
    }
}
/// Apply a committed date-picker result to `dates`, re-anchoring the group that
/// currently starts at `old_start`. A `Single` collapses to a one-day span; a
/// `Range` expands to the inclusive span. Returns whether anything was applied.
fn apply_date_pick(
    dates: &mut Vec<model::NonWorkingDate>,
    old_start: chrono::NaiveDate,
    result: &datepicker::DatePickerResult,
) -> bool {
    let (old_dates, desc) = {
        let groups = group_holidays(dates);
        match groups.iter().find(|g| g.start == old_start) {
            Some(g) => (g.dates.clone(), g.description.clone()),
            None => return false,
        }
    };
    let (start, end) = match *result {
        datepicker::DatePickerResult::Single(d) => (d, d),
        datepicker::DatePickerResult::Range(s, e) => (s, e),
    };
    set_date_range(dates, &old_dates, start, end, &desc);
    true
}
/// Display order for the settings SIZES list: indices into `sizes` sorted by
/// `days` ascending (stable, so ties keep insertion order). While `frozen` and
/// the cached order still covers the current list, the cache is returned
/// unchanged so rows don't re-sort (and steal focus) mid-edit; any add or
/// remove invalidates the cache and falls back to a fresh sort.
fn sizes_display_order(sizes: &[model::TShirtSize], cached: &[usize], frozen: bool) -> Vec<usize> {
    if frozen && cached.len() == sizes.len() {
        return cached.to_vec();
    }
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| sizes[i].days);
    order
}
/// Which date group an open Aurora picker is editing — a calendar holiday or a
/// specific resource's time-off, keyed by the group's (current) start date.
#[derive(Clone, PartialEq)]
enum DateTarget {
    Holiday(chrono::NaiveDate),
    Resource(String, chrono::NaiveDate),
}
/// An open settings date-range picker popup: what it edits, its calendar state,
/// and the screen anchor to draw it at (kept fresh from the row each frame).
struct OpenDatePicker {
    target: DateTarget,
    state: datepicker::DatePickerState,
    anchor: egui::Pos2,
    /// `false` on the frame it opened (so the chip-click that spawned it isn't
    /// counted as a click-outside dismiss); becomes `true` after one frame.
    armed: bool,
}
/// Right-side settings fly-out. Toggled by the top-bar gear. Holds general
/// settings; the first section is the calendar (working days per week, the
/// holiday list, and the start date). Edits write straight to `model.calendar`
/// and autosave.
pub fn settings_flyout_ui(
    mut contexts: EguiContexts,
    mut settings: ResMut<SettingsState>,
    mut model: ResMut<model::Model>,
    mut save: ResMut<db::SaveRequest>,
) {
    if !settings.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut changed = false;
    let mut close = false;

    egui::SidePanel::right("settings_flyout")
        .resizable(false)
        .exact_width(272.0)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::STROKE))
                .inner_margin(egui::Margin::same(14)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Settings")
                        .size(16.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(egui::RichText::new("✕").size(14.0)).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(10.0);

            // auto_shrink([false,false]) pins body to the full panel width so
            // desired_width fields and RTL buttons stay on-screen.
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Constrain content to leave room for the scrollbar gutter and
                    // right breathing room so the bar never overlays controls.
                    let gutter =
                        ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
                    ui.set_max_width(ui.available_width() - gutter - 6.0);

                    // ── CALENDAR ─────────────────────────────────────────────
                    theme::section_header(ui, "CALENDAR", None);

                    settings_row(ui, "Days / week", |ui| {
                        let mut wdpw = model.calendar.working_days_per_week as i32;
                        if ui
                            .add(egui::DragValue::new(&mut wdpw).range(1..=7).speed(0.05))
                            .changed()
                        {
                            model.calendar.working_days_per_week = wdpw.clamp(1, 7) as u8;
                            changed = true;
                        }
                        theme::chip(ui, &format!("{} / 7", model.calendar.working_days_per_week));
                    });

                    ui.add_space(SETTINGS_ROW_GAP);
                    // Start date: click the chip to edit in place; no separate TextEdit + SET.
                    settings_row(ui, "Start date", |ui| {
                        if matches!(&settings.editing, Some(SettingsEdit::StartDate)) {
                            let r = ui
                                .scope(|ui| {
                                    theme::style_inputs(ui);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut settings.edit_buf)
                                            .hint_text("YYYY-MM-DD")
                                            .desired_width(100.0),
                                    )
                                })
                                .inner;
                            r.request_focus();
                            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if esc {
                                settings.editing = None;
                            } else if r.lost_focus()
                                || ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                if let Ok(d) = chrono::NaiveDate::parse_from_str(
                                    settings.edit_buf.trim(),
                                    "%Y-%m-%d",
                                ) {
                                    model.calendar.start_date = d;
                                    changed = true;
                                }
                                settings.editing = None;
                            }
                        } else {
                            let resp = theme::date_chip(ui, model.calendar.start_date);
                            if resp.clicked() {
                                settings.editing = Some(SettingsEdit::StartDate);
                                settings.edit_buf =
                                    model.calendar.start_date.format("%Y-%m-%d").to_string();
                            }
                        }
                    });

                    // ── HOLIDAYS ──────────────────────────────────────────────
                    // Header inlined to accommodate the + add button on the right.
                    ui.add_space(SETTINGS_SECTION_GAP);
                    let holiday_groups = group_holidays(&model.calendar.non_working_dates);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("HOLIDAYS")
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::add_button(ui).on_hover_text("Add holiday").clicked() {
                                // Find a date not already in the list as a unique key.
                                let mut new_date = model.calendar.start_date;
                                for _ in 0..1000 {
                                    if !model
                                        .calendar
                                        .non_working_dates
                                        .iter()
                                        .any(|x| x.date == new_date)
                                    {
                                        break;
                                    }
                                    new_date = new_date.succ_opt().unwrap_or(new_date);
                                }
                                model
                                    .calendar
                                    .non_working_dates
                                    .push(model::NonWorkingDate {
                                        date: new_date,
                                        description: String::new(),
                                    });
                                changed = true;
                                settings.editing = Some(SettingsEdit::HolidayLabel(new_date));
                                settings.edit_buf = String::new();
                            }
                            if !holiday_groups.is_empty() {
                                ui.label(
                                    egui::RichText::new(holiday_groups.len().to_string())
                                        .size(10.0)
                                        .color(theme::ACCENT),
                                );
                            }
                        });
                    });
                    // Section rule (same as theme::section_header draws).
                    {
                        let avail = ui.available_width();
                        if avail > 0.0 {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(avail, 1.0), egui::Sense::hover());
                            ui.painter().hline(
                                rect.x_range(),
                                rect.center().y,
                                egui::Stroke::new(1.0, theme::STROKE),
                            );
                        }
                    }
                    ui.add_space(4.0);

                    if holiday_groups.is_empty() {
                        ui.label(
                            egui::RichText::new("None set")
                                .italics()
                                .color(theme::TEXT_MUTED),
                        );
                    }

                    let mut remove_hol: Option<Vec<chrono::NaiveDate>> = None;
                    for g in &holiday_groups {
                        let ed_label = matches!(&settings.editing,
                            Some(SettingsEdit::HolidayLabel(d)) if *d == g.start);

                        let mut row_rm_rect = egui::Rect::NOTHING;
                        let mut row_rm_clicked = false;
                        let mut row_rm_hovered = false;

                        let row = theme::list_row(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Date span: a clickable chip for the start (plus
                                // an end chip for a multi-day span). Clicking either
                                // opens the Aurora calendar range picker popup.
                                let cr = theme::date_chip(ui, g.start);
                                let anchor = cr.rect.left_bottom();
                                let mut open_picker = cr.clicked();
                                if g.start != g.end {
                                    ui.label(egui::RichText::new("–").color(theme::TEXT_MUTED));
                                    open_picker |= theme::date_chip(ui, g.end).clicked();
                                }
                                if open_picker {
                                    settings.editing = None;
                                    settings.picker = Some(OpenDatePicker {
                                        target: DateTarget::Holiday(g.start),
                                        state: datepicker::DatePickerState::range(g.start, None),
                                        anchor,
                                        armed: false,
                                    });
                                }
                                // Keep an already-open picker pinned under this row.
                                if let Some(p) = &mut settings.picker {
                                    if p.target == DateTarget::Holiday(g.start) {
                                        p.anchor = anchor;
                                    }
                                }

                                ui.add_space(4.0);

                                // Label: clickable text or in-place TextEdit.
                                // Reserve remove-slot width so the label doesn't overlap it.
                                let remove_w = 20.0;
                                let label_avail =
                                    (ui.available_width() - remove_w - ui.spacing().item_spacing.x)
                                        .max(20.0);
                                if ed_label {
                                    let r = ui
                                        .scope(|ui| {
                                            theme::style_inputs(ui);
                                            ui.add(
                                                egui::TextEdit::singleline(&mut settings.edit_buf)
                                                    .desired_width(label_avail)
                                                    .hint_text("Label"),
                                            )
                                        })
                                        .inner;
                                    r.request_focus();
                                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    if esc {
                                        settings.editing = None;
                                    } else if r.lost_focus()
                                        || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    {
                                        let new_desc = settings.edit_buf.trim().to_string();
                                        let grp_dates = g.dates.clone();
                                        for nwd in &mut model.calendar.non_working_dates {
                                            if grp_dates.contains(&nwd.date) {
                                                nwd.description = new_desc.clone();
                                                changed = true;
                                            }
                                        }
                                        settings.editing = None;
                                    }
                                } else {
                                    let desc = if g.description.is_empty() {
                                        "—"
                                    } else {
                                        g.description.as_str()
                                    };
                                    let lr = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(desc).color(theme::TEXT_MUTED),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    if lr.clicked() {
                                        settings.editing =
                                            Some(SettingsEdit::HolidayLabel(g.start));
                                        settings.edit_buf = g.description.clone();
                                    }
                                }

                                // Always allocate the × slot to prevent layout shift on hover.
                                let (rr, rresp) = ui
                                    .with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.allocate_exact_size(
                                                egui::vec2(remove_w, 14.0),
                                                egui::Sense::click(),
                                            )
                                        },
                                    )
                                    .inner;
                                row_rm_rect = rr;
                                row_rm_clicked = rresp.clicked();
                                row_rm_hovered = rresp.hovered();
                            });
                        });

                        // Paint × only when row hovered; click detection is always registered.
                        let ptr = ui.ctx().input(|i| i.pointer.hover_pos());
                        if ptr.is_some_and(|p| row.response.rect.contains(p)) {
                            ui.painter().text(
                                row_rm_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "×",
                                egui::FontId::proportional(14.0),
                                if row_rm_hovered {
                                    theme::DANGER
                                } else {
                                    theme::TEXT_MUTED
                                },
                            );
                            if row_rm_clicked {
                                remove_hol = Some(g.dates.clone());
                            }
                        }
                        ui.add_space(2.0);
                    }
                    if let Some(dts) = remove_hol {
                        model
                            .calendar
                            .non_working_dates
                            .retain(|x| !dts.contains(&x.date));
                        changed = true;
                        let stale = matches!(&settings.editing,
                            Some(SettingsEdit::HolidayLabel(d)) if dts.contains(d));
                        if stale {
                            settings.editing = None;
                        }
                        // Drop an open picker that targeted a removed group.
                        if matches!(&settings.picker,
                            Some(p) if matches!(&p.target, DateTarget::Holiday(d) if dts.contains(d)))
                        {
                            settings.picker = None;
                        }
                    }

                    // ── RESOURCES ─────────────────────────────────────────────
                    ui.add_space(SETTINGS_SECTION_GAP);
                    let resource_count = model.named_resources().len();
                    theme::section_header(ui, "RESOURCES", Some(resource_count));

                    let names = model.named_resources();
                    if names.is_empty() {
                        ui.label(
                            egui::RichText::new("Name rows in the gutter to add resources")
                                .italics()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                    for name in &names {
                        ui.horizontal(|ui| {
                            let kind = model.resource_kind(name);
                            if let Some(k) = kind {
                                let dot = ui.allocate_space(egui::vec2(9.0, 9.0)).1;
                                theme::draw_resource_dot(ui.painter(), dot.center(), k);
                            }
                            ui.label(egui::RichText::new(name).color(theme::TEXT));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    egui::ComboBox::from_id_salt(format!("restype:{name}"))
                                        .selected_text(kind.map(|k| k.label()).unwrap_or("—"))
                                        .width(96.0)
                                        .show_ui(ui, |ui| {
                                            for k in model::ResourceType::ALL {
                                                if ui
                                                    .selectable_label(kind == Some(k), k.label())
                                                    .clicked()
                                                {
                                                    model.set_resource_kind(name, k);
                                                    changed = true;
                                                }
                                            }
                                        });
                                },
                            );
                        });

                        // Per-resource time-off — in-place edit, hover-only × remove.
                        let rb_id = model
                            .resource_blocks
                            .values()
                            .find(|r| r.name.eq_ignore_ascii_case(name))
                            .map(|r| r.id);
                        if let Some(rb_id) = rb_id {
                            let sorted = model.resource_blocks[&rb_id].non_working_dates.to_vec();
                            let to_groups = group_holidays(&sorted);

                            let mut remove_to: Option<Vec<chrono::NaiveDate>> = None;
                            for g in &to_groups {
                                let ed_lbl = matches!(&settings.editing,
                                    Some(SettingsEdit::ResourceLabel(rn, d))
                                    if rn.as_str() == name.as_str() && *d == g.start);

                                let mut row_rm_rect = egui::Rect::NOTHING;
                                let mut row_rm_clicked = false;
                                let mut row_rm_hovered = false;

                                ui.add_space(2.0);
                                let row = theme::list_row(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add_space(4.0);

                                        // Date span: clickable chip(s) opening the
                                        // Aurora calendar range picker popup.
                                        let cr = theme::date_chip(ui, g.start);
                                        let anchor = cr.rect.left_bottom();
                                        let mut open_picker = cr.clicked();
                                        if g.start != g.end {
                                            ui.label(
                                                egui::RichText::new("–").color(theme::TEXT_MUTED),
                                            );
                                            open_picker |= theme::date_chip(ui, g.end).clicked();
                                        }
                                        if open_picker {
                                            settings.editing = None;
                                            settings.picker = Some(OpenDatePicker {
                                                target: DateTarget::Resource(
                                                    name.clone(),
                                                    g.start,
                                                ),
                                                state: datepicker::DatePickerState::range(
                                                    g.start, None,
                                                ),
                                                anchor,
                                                armed: false,
                                            });
                                        }
                                        if let Some(p) = &mut settings.picker {
                                            if p.target
                                                == DateTarget::Resource(name.clone(), g.start)
                                            {
                                                p.anchor = anchor;
                                            }
                                        }

                                        ui.add_space(4.0);

                                        let remove_w = 20.0;
                                        let label_avail = (ui.available_width()
                                            - remove_w
                                            - ui.spacing().item_spacing.x)
                                            .max(20.0);

                                        // Label or in-place label editor.
                                        if ed_lbl {
                                            let r = ui
                                                .scope(|ui| {
                                                    theme::style_inputs(ui);
                                                    ui.add(
                                                        egui::TextEdit::singleline(
                                                            &mut settings.edit_buf,
                                                        )
                                                        .desired_width(label_avail)
                                                        .hint_text("Reason"),
                                                    )
                                                })
                                                .inner;
                                            r.request_focus();
                                            let esc =
                                                ui.input(|i| i.key_pressed(egui::Key::Escape));
                                            if esc {
                                                settings.editing = None;
                                            } else if r.lost_focus()
                                                || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                            {
                                                let new_desc = settings.edit_buf.trim().to_string();
                                                let grp_dates = g.dates.clone();
                                                if let Some(rb) =
                                                    model.resource_blocks.get_mut(&rb_id)
                                                {
                                                    for nwd in &mut rb.non_working_dates {
                                                        if grp_dates.contains(&nwd.date) {
                                                            nwd.description = new_desc.clone();
                                                            changed = true;
                                                        }
                                                    }
                                                }
                                                settings.editing = None;
                                            }
                                        } else {
                                            let desc = if g.description.is_empty() {
                                                "—"
                                            } else {
                                                g.description.as_str()
                                            };
                                            let lr = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(desc)
                                                        .size(11.0)
                                                        .color(theme::TEXT_MUTED),
                                                )
                                                .sense(egui::Sense::click()),
                                            );
                                            if lr.clicked() {
                                                settings.editing =
                                                    Some(SettingsEdit::ResourceLabel(
                                                        name.clone(),
                                                        g.start,
                                                    ));
                                                settings.edit_buf = g.description.clone();
                                            }
                                        }

                                        // Always allocate the × slot.
                                        let (rr, rresp) = ui
                                            .with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.allocate_exact_size(
                                                        egui::vec2(remove_w, 14.0),
                                                        egui::Sense::click(),
                                                    )
                                                },
                                            )
                                            .inner;
                                        row_rm_rect = rr;
                                        row_rm_clicked = rresp.clicked();
                                        row_rm_hovered = rresp.hovered();
                                    });
                                });

                                let ptr = ui.ctx().input(|i| i.pointer.hover_pos());
                                if ptr.is_some_and(|p| row.response.rect.contains(p)) {
                                    ui.painter().text(
                                        row_rm_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "×",
                                        egui::FontId::proportional(14.0),
                                        if row_rm_hovered {
                                            theme::DANGER
                                        } else {
                                            theme::TEXT_MUTED
                                        },
                                    );
                                    if row_rm_clicked {
                                        remove_to = Some(g.dates.clone());
                                    }
                                }
                            }
                            if let Some(dts) = remove_to {
                                if let Some(rb) = model.resource_blocks.get_mut(&rb_id) {
                                    rb.non_working_dates.retain(|x| !dts.contains(&x.date));
                                    changed = true;
                                }
                                let stale = matches!(&settings.editing,
                                    Some(SettingsEdit::ResourceLabel(rn, d))
                                    if rn.as_str() == name.as_str() && dts.contains(d));
                                if stale {
                                    settings.editing = None;
                                }
                                if matches!(&settings.picker, Some(p) if matches!(&p.target,
                                    DateTarget::Resource(rn, d)
                                    if rn.as_str() == name.as_str() && dts.contains(d)))
                                {
                                    settings.picker = None;
                                }
                            }

                            // + Time off: append a blank entry and enter label-edit mode.
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                if ui
                                    .small_button(
                                        egui::RichText::new("+ Time off")
                                            .size(11.0)
                                            .color(theme::ACCENT),
                                    )
                                    .clicked()
                                {
                                    let existing: Vec<_> = model
                                        .resource_blocks
                                        .get(&rb_id)
                                        .map(|rb| {
                                            rb.non_working_dates.iter().map(|x| x.date).collect()
                                        })
                                        .unwrap_or_default();
                                    let mut new_date = model.calendar.start_date;
                                    for _ in 0..1000 {
                                        if !existing.contains(&new_date) {
                                            break;
                                        }
                                        new_date = new_date.succ_opt().unwrap_or(new_date);
                                    }
                                    if let Some(rb) = model.resource_blocks.get_mut(&rb_id) {
                                        rb.non_working_dates.push(model::NonWorkingDate {
                                            date: new_date,
                                            description: String::new(),
                                        });
                                        changed = true;
                                    }
                                    settings.editing =
                                        Some(SettingsEdit::ResourceLabel(name.clone(), new_date));
                                    settings.edit_buf = String::new();
                                }
                            });
                        }
                        ui.add_space(4.0);
                    }

                    // ── SIZES ─────────────────────────────────────────────────
                    ui.add_space(SETTINGS_SECTION_GAP);
                    theme::section_header(ui, "SIZES", Some(model.t_shirt_sizes.len()));

                    // Re-sorting is frozen while a row is being edited so rows
                    // don't jump (and drop focus) as the days value changes.
                    let order = sizes_display_order(
                        &model.t_shirt_sizes,
                        &settings.sizes_order,
                        settings.sizes_editing,
                    );

                    let mut sizes_editing_now = false;
                    let mut remove_size: Option<usize> = None;
                    for &idx in &order {
                        let mut trash_rect = egui::Rect::NOTHING;
                        let mut trash_clicked = false;
                        let mut trash_hovered = false;

                        let row = ui.horizontal(|ui| {
                            let trash_w = 22.0;
                            let right_reserve = 58.0 + trash_w + ui.spacing().item_spacing.x * 2.0;
                            let label_w = (ui.available_width() - right_reserve).max(40.0);
                            let label_resp = ui.add(
                                egui::TextEdit::singleline(&mut model.t_shirt_sizes[idx].label)
                                    .desired_width(label_w),
                            );
                            if label_resp.changed() {
                                changed = true;
                            }
                            if settings.sizes_focus == Some(idx) {
                                label_resp.request_focus();
                                settings.sizes_focus = None;
                                sizes_editing_now = true;
                            }
                            let days_resp = ui.add(
                                egui::DragValue::new(&mut model.t_shirt_sizes[idx].days)
                                    .range(1..=400)
                                    .suffix(" d"),
                            );
                            if days_resp.changed() {
                                changed = true;
                            }
                            sizes_editing_now |= label_resp.has_focus()
                                || days_resp.has_focus()
                                || days_resp.dragged();
                            // Always allocate the 🗑 slot; only paint and act on hover.
                            let (tr, tresp) = ui.allocate_exact_size(
                                egui::vec2(trash_w, 16.0),
                                egui::Sense::click(),
                            );
                            trash_rect = tr;
                            trash_clicked = tresp.clicked();
                            trash_hovered = tresp.hovered();
                        });

                        let ptr = ui.ctx().input(|i| i.pointer.hover_pos());
                        if ptr.is_some_and(|p| row.response.rect.contains(p)) {
                            ui.painter().text(
                                trash_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "🗑",
                                egui::FontId::proportional(14.0),
                                if trash_hovered {
                                    theme::DANGER
                                } else {
                                    theme::TEXT_MUTED
                                },
                            );
                            if trash_clicked {
                                remove_size = Some(idx);
                            }
                        }
                        ui.add_space(2.0);
                    }
                    settings.sizes_order = order;
                    settings.sizes_editing = sizes_editing_now;
                    if let Some(i) = remove_size {
                        if i < model.t_shirt_sizes.len() {
                            model.t_shirt_sizes.remove(i);
                            changed = true;
                        }
                    }
                    ui.add_space(SETTINGS_ROW_GAP);
                    ui.horizontal(|ui| {
                        if theme::add_button(ui).clicked() {
                            model.t_shirt_sizes.push(model::TShirtSize {
                                label: "New".to_string(),
                                days: 5,
                            });
                            settings.sizes_focus = Some(model.t_shirt_sizes.len() - 1);
                            changed = true;
                        }
                        ui.label(
                            egui::RichText::new("Add size")
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                        );
                    });
                }); // ScrollArea::vertical
        });

    // ── Date-range picker popup ───────────────────────────────────────────────
    // Drawn over the panel as a foreground Area anchored under the clicked row.
    // Committing a date re-anchors that group's span; Esc or a click outside the
    // popup dismisses it without change.
    if let Some(mut open) = settings.picker.take() {
        let area = egui::Area::new(egui::Id::new("settings_date_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(open.anchor)
            .show(ctx, |ui| {
                datepicker::aurora_date_picker(ui, &mut open.state)
            });
        let committed = area.inner;
        let area_rect = area.response.rect;
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let click_outside = open.armed
            && ctx.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !area_rect.contains(p))
            });
        if let Some(result) = committed {
            let applied = match &open.target {
                DateTarget::Holiday(start) => {
                    apply_date_pick(&mut model.calendar.non_working_dates, *start, &result)
                }
                DateTarget::Resource(rn, start) => {
                    let rb_id = model
                        .resource_blocks
                        .values()
                        .find(|r| r.name.eq_ignore_ascii_case(rn))
                        .map(|r| r.id);
                    match rb_id.and_then(|id| model.resource_blocks.get_mut(&id)) {
                        Some(rb) => apply_date_pick(&mut rb.non_working_dates, *start, &result),
                        None => false,
                    }
                }
            };
            changed |= applied;
            // Committed → leave `settings.picker` cleared (closed).
        } else if esc || click_outside {
            // Dismissed → leave cleared.
        } else {
            open.armed = true;
            settings.picker = Some(open); // still open next frame
        }
    }

    if close {
        settings.open = false;
    }
    if changed {
        save.mark();
    }
}
/// Which settings field is currently being edited in-place (one at a time).
#[derive(Clone, PartialEq)]
enum SettingsEdit {
    StartDate,
    /// Holiday group's label — keyed by the group's start date (unique).
    HolidayLabel(chrono::NaiveDate),
    /// Per-resource time-off label — (resource name, group start date).
    ResourceLabel(String, chrono::NaiveDate),
}
/// State for the right-side settings fly-out: which field (if any) is being
/// edited in-place, plus the shared edit buffer.
#[derive(Resource, Default)]
pub struct SettingsState {
    pub open: bool,
    /// The field currently being edited in-place (one at a time).
    editing: Option<SettingsEdit>,
    /// Shared text buffer for whichever field is in `editing`.
    edit_buf: String,
    /// Open Aurora date-range picker popup, if any (one at a time).
    picker: Option<OpenDatePicker>,
    /// Cached SIZES display order (indices into `model.t_shirt_sizes`), kept
    /// frozen while a size row is being edited (see `sizes_display_order`).
    sizes_order: Vec<usize>,
    /// True while a SIZES row widget had focus last frame — freezes re-sorting.
    sizes_editing: bool,
    /// Model index of a just-added size whose label should grab focus.
    sizes_focus: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn group_holidays_merges_consecutive_same_label() {
        use chrono::NaiveDate;
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        let nwd = |date, desc: &str| model::NonWorkingDate {
            date,
            description: desc.to_string(),
        };
        let dates = vec![
            nwd(d(12, 26), "Christmas"),
            nwd(d(12, 24), "Christmas"),
            nwd(d(12, 25), "Christmas"),
            nwd(d(7, 4), "July 4"),
            nwd(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), "Christmas"),
        ];
        let groups = group_holidays(&dates);
        assert_eq!(
            groups.len(),
            3,
            "xmas run + july4 + far new-year = 3 groups"
        );
        let xmas = groups
            .iter()
            .find(|g| g.start == d(12, 24))
            .expect("christmas group");
        assert_eq!(xmas.end, d(12, 26));
        assert_eq!(xmas.dates.len(), 3);
        assert_eq!(xmas.description, "Christmas");
    }
    #[test]
    fn expand_date_range_inclusive_and_clamped() {
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        // Inclusive multi-day span.
        assert_eq!(
            expand_date_range(d(7, 1), d(7, 3)),
            vec![d(7, 1), d(7, 2), d(7, 3)]
        );
        // end == start is a single day.
        assert_eq!(expand_date_range(d(7, 1), d(7, 1)), vec![d(7, 1)]);
        // end < start collapses to a single day at start.
        assert_eq!(expand_date_range(d(7, 5), d(7, 1)), vec![d(7, 5)]);
        // A wildly large span is clamped to start + 366 days inclusive.
        let huge = expand_date_range(d(1, 1), NaiveDate::from_ymd_opt(2030, 1, 1).unwrap());
        assert_eq!(huge.len(), 367, "clamped to start + 366 days inclusive");
    }
    #[test]
    fn set_date_range_replaces_group_and_dedups() {
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        let nwd = |date, desc: &str| model::NonWorkingDate {
            date,
            description: desc.to_string(),
        };
        // A 2-day "Trip" group plus an unrelated holiday.
        let mut dates = vec![
            nwd(d(7, 1), "Trip"),
            nwd(d(7, 2), "Trip"),
            nwd(d(1, 1), "NY"),
        ];
        // Re-anchor the Trip group to a 3-day span starting one day later.
        set_date_range(&mut dates, &[d(7, 1), d(7, 2)], d(7, 2), d(7, 4), "Trip");
        let mut trip: Vec<_> = dates
            .iter()
            .filter(|x| x.description == "Trip")
            .map(|x| x.date)
            .collect();
        trip.sort();
        assert_eq!(trip, vec![d(7, 2), d(7, 3), d(7, 4)]);
        // Unrelated holiday is untouched; no duplicate dates.
        assert!(dates
            .iter()
            .any(|x| x.date == d(1, 1) && x.description == "NY"));
        assert_eq!(dates.len(), 4);
    }
    #[test]
    fn apply_date_pick_range_reanchors_group() {
        use crate::datepicker::DatePickerResult;
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        let nwd = |date, desc: &str| model::NonWorkingDate {
            date,
            description: desc.to_string(),
        };
        let mut dates = vec![nwd(d(7, 1), "PTO")];
        let applied = apply_date_pick(
            &mut dates,
            d(7, 1),
            &DatePickerResult::Range(d(7, 1), d(7, 3)),
        );
        assert!(applied);
        let mut got: Vec<_> = dates.iter().map(|x| x.date).collect();
        got.sort();
        assert_eq!(got, vec![d(7, 1), d(7, 2), d(7, 3)]);
        assert!(dates.iter().all(|x| x.description == "PTO"));
        // A start that matches no group applies nothing.
        assert!(!apply_date_pick(
            &mut dates,
            d(12, 25),
            &DatePickerResult::Single(d(12, 25))
        ));
    }
    #[test]
    fn sizes_display_order_sorts_and_freezes() {
        let s = |label: &str, days| model::TShirtSize {
            label: label.to_string(),
            days,
        };
        let sizes = vec![s("M", 15), s("XS", 5), s("L", 25)];
        // Unfrozen: sorted by days ascending.
        assert_eq!(sizes_display_order(&sizes, &[], false), vec![1, 0, 2]);
        // Frozen with a still-valid cache: cache wins even though it no longer
        // matches the sorted order (a mid-edit days change must not re-sort).
        assert_eq!(sizes_display_order(&sizes, &[0, 1, 2], true), vec![0, 1, 2]);
        // Frozen but the cache is stale (a size was added/removed): re-sort.
        assert_eq!(sizes_display_order(&sizes, &[0, 1], true), vec![1, 0, 2]);
        // Ties keep insertion order (stable sort).
        let tied = vec![s("A", 5), s("B", 5)];
        assert_eq!(sizes_display_order(&tied, &[], false), vec![0, 1]);
    }
}
