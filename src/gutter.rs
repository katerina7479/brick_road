//! The left resource gutter: row labels that track the camera, in-place row
//! rename, the resource picker popup, and drag-to-reorder of resource rows.
//! Extracted from main.rs (#340) — a pure move, no behavior change.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::{
    bands, constants, db, flow,
    model::{self, default_row_label},
    schedule, theme, ViewKind, ViewMode,
};

/// Width of the resource-name gutter, in logical pixels.
const GUTTER_WIDTH: f32 = 116.0;
/// Left gutter naming the resource rows of the current view. It carries only a
/// faint background — just enough to be a click target (so clicks don't fall
/// through and create blocks) without reading as a heavy panel. Each row a
/// block sits on gets a label that tracks the row vertically as the camera pans
/// (like the calendar ruler tracks days). Double-click a name to edit it in
/// place; typing an existing row's name merges this row's work onto it.
///
/// Names resolve through the active plan at the current drill scope, with
/// branches inheriting main's names by default (`Model::resolved_row_name`).
#[allow(clippy::too_many_arguments)]
pub fn resource_gutter_ui(
    mut contexts: EguiContexts,
    mut model: ResMut<model::Model>,
    drill: Res<schedule::DrillScope>,
    visible: Res<schedule::VisibleBlocks>,
    view: Res<ViewMode>,
    person_view: Res<schedule::PersonViewCache>,
    flow_cache: Res<flow::FlowCache>,
    mut rename: ResMut<RowRename>,
    mut save: ResMut<db::SaveRequest>,
    keys: Res<ButtonInput<KeyCode>>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Ok((cam_t, proj)) = cam_q.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let Ok(window) = windows.single() else { return };
    let rh = constants::ROW_HEIGHT;
    let scope = drill.path.last().copied();

    // In By-Person the gutter is read-only: no rename, no resource picker. Clear
    // any pending one (e.g. a rename started in By-Plan, then toggled to
    // By-Person) up front. Both the editable field and the Enter/lost-focus
    // commit key off `rename.editing`, so leaving it set is a read-only escape
    // that could `commit_row_name` on a main lane.
    if view.kind != ViewKind::Plan
        && (rename.editing.is_some() || rename.picker_open.is_some() || rename.drag.is_some())
    {
        rename.editing = None;
        rename.picker_open = None;
        rename.drag = None;
        rename.buf.clear();
    }

    // One labelled row in the gutter. Each plan — main plus every forked band —
    // contributes its own rows at its own world-Y, so the gutter is plan-aware
    // rather than locked to a single "active" plan.
    struct GutterRow {
        plan_id: model::PlanId,
        scope: Option<model::WorkBlockId>,
        row: i32,
        world_y: f32,
        name: Option<String>,
        kind: Option<model::ResourceType>,
    }
    // Resolve a row's display name (with branch→main inheritance) and resource
    // type up front so the egui closure borrows no model state while it may
    // mutate on commit.
    let resolve = |model: &model::Model,
                   plan_id: model::PlanId,
                   scope: Option<model::WorkBlockId>,
                   row: i32,
                   world_y: f32| {
        let name = model.resolved_row_name(plan_id, scope, row);
        let kind = name.and_then(|n| model.resource_kind(n));
        GutterRow {
            plan_id,
            scope,
            row,
            world_y,
            name: name.map(|s| s.to_string()),
            kind,
        }
    };

    // The gutter only labels rows that carry a visible block, so it stays empty
    // until there's real work on a row.
    let mut entries: Vec<GutterRow> = Vec::new();

    if view.kind == ViewKind::Flow {
        // Flow: one read-only label at each stream's first ribbon level.
        let dummy_plan = model.main_plan_id().unwrap_or(model::PlanId(0));
        for (i, lane) in flow_cache.0.lanes.iter().enumerate() {
            entries.push(GutterRow {
                plan_id: dummy_plan,
                scope: None,
                row: i as i32,
                world_y: flow::flow_level_y(&flow_cache.0.lanes, i, 0),
                name: Some(lane.name.clone()),
                kind: None,
            });
        }
    } else if view.kind == ViewKind::Resource {
        // By-resource: one read-only label at each group's first row. A group
        // spanning extra sub-rows (concurrent work) is labelled only once.
        let dummy_plan = model.main_plan_id().unwrap_or(model::PlanId(0));
        for (name, kind, base_row) in person_view.0.rows.iter() {
            entries.push(GutterRow {
                plan_id: dummy_plan,
                scope: None,
                row: *base_row,
                world_y: -(*base_row as f32 * rh),
                name: Some(name.clone()),
                kind: *kind,
            });
        }
    } else {
        // Main plan occupies world rows 0,1,2… at y = -row * ROW_HEIGHT, respecting
        // the active drill scope.
        if let Some(main_id) = model.main_plan_id() {
            let mut rows: Vec<i32> = visible
                .ids
                .iter()
                .map(|id| model.block_row(main_id, *id))
                .collect();
            rows.sort_unstable();
            rows.dedup();
            for r in rows {
                entries.push(resolve(&model, main_id, scope, r, -(r as f32 * rh)));
            }
        }

        // Each forked-plan band labels its own rows, anchored at that lane's row0_y.
        // Bands are hidden while drilled into a block, so skip them then.
        if drill.path.is_empty() {
            for band in bands::layout_bands(&model) {
                let mut rows: Vec<i32> = schedule::visible_blocks(&model, band.plan_id, None)
                    .iter()
                    .map(|wb| model.block_row(band.plan_id, wb.id))
                    .collect();
                rows.sort_unstable();
                rows.dedup();
                for r in rows {
                    entries.push(resolve(
                        &model,
                        band.plan_id,
                        None,
                        r,
                        band.row0_y - r as f32 * rh,
                    ));
                }
            }
        }

        // A brand-new resource lane is edited before it has a block. Give the edited
        // row an entry (synthesizing its world-Y from the row index in its plan) so
        // the rename field below actually renders on that row.
        if let Some((pid, sc, r)) = rename.editing {
            if !entries
                .iter()
                .any(|e| e.plan_id == pid && e.scope == sc && e.row == r)
            {
                let world_y = if Some(pid) == model.main_plan_id() {
                    -(r as f32 * rh)
                } else {
                    bands::layout_bands(&model)
                        .into_iter()
                        .find(|b| b.plan_id == pid)
                        .map(|b| b.row0_y - r as f32 * rh)
                        .unwrap_or(-(r as f32 * rh))
                };
                entries.push(resolve(&model, pid, sc, r, world_y));
            }
        }
    }

    if entries.is_empty() && rename.editing.is_none() {
        return;
    }

    let scale = ortho.scale;
    let cam_y = cam_t.translation.y;
    let win_h = window.height();
    let world_to_screen_y = |wy: f32| win_h * 0.5 + (cam_y - wy) / scale;
    let editing = rename.editing;
    let picker_open = rename.picker_open;
    let dragging = rename.drag;

    let known_resources = model.named_resources();
    let resource_kinds: Vec<Option<model::ResourceType>> = known_resources
        .iter()
        .map(|n| model.resource_kind(n))
        .collect();

    let Ok(ctx) = contexts.ctx_mut() else { return };

    enum Act {
        OpenPicker(model::PlanId, Option<model::WorkBlockId>, i32),
        ClosePicker,
        SelectResource(model::PlanId, Option<model::WorkBlockId>, i32, String),
        StartNew(model::PlanId, Option<model::WorkBlockId>, i32),
        CommitNew,
        CancelNew,
        StartDrag(model::PlanId, Option<model::WorkBlockId>, i32),
        DropRow(model::PlanId, Option<model::WorkBlockId>, i32, i32),
        CancelDrag,
    }
    let mut act: Option<Act> = if keys.just_pressed(KeyCode::Escape) {
        if dragging.is_some() {
            Some(Act::CancelDrag)
        } else if editing.is_some() {
            Some(Act::CancelNew)
        } else if picker_open.is_some() {
            Some(Act::ClosePicker)
        } else {
            None
        }
    } else if editing.is_some()
        && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
    {
        Some(Act::CommitNew)
    } else {
        None
    };

    egui::SidePanel::left("resource_gutter")
        .exact_width(GUTTER_WIDTH)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(14, 20, 24, 96))
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            let half = (rh / scale * 0.5).clamp(9.0, 22.0);

            for e in &entries {
                let cy = world_to_screen_y(e.world_y);
                if cy < rect.top() - half || cy > rect.bottom() + half {
                    continue;
                }

                let key = (e.plan_id, e.scope, e.row);
                let name = &e.name;
                let kind = &e.kind;
                let editing_this = editing == Some(key);
                let picker_this = picker_open == Some(key);

                if editing_this {
                    let field = egui::Rect::from_min_max(
                        egui::pos2(rect.left() + 6.0, cy - 9.0),
                        egui::pos2(rect.right() - 4.0, cy + 9.0),
                    );
                    let resp = ui
                        .scope(|ui| {
                            theme::style_inputs(ui);
                            ui.put(
                                field,
                                egui::TextEdit::singleline(&mut rename.buf)
                                    .id(egui::Id::new(("gutter_rename", e.plan_id.0, e.row)))
                                    .font(egui::FontId::proportional(13.0))
                                    .text_color(theme::TEXT),
                            )
                        })
                        .inner;
                    if !resp.has_focus() {
                        resp.request_focus();
                    }
                    if resp.lost_focus() && act.is_none() {
                        act = Some(Act::CommitNew);
                    }
                } else if view.kind != ViewKind::Plan {
                    // By-person: read-only label + dot, no click interaction.
                    let (text, color) = match name {
                        Some(n) => (n.clone(), egui::Color32::from_rgb(206, 190, 164)),
                        None => (
                            default_row_label(e.row),
                            egui::Color32::from_rgb(138, 128, 114),
                        ),
                    };
                    let mut text_x = rect.left() + 10.0;
                    if let Some(k) = kind {
                        theme::draw_resource_dot(
                            ui.painter(),
                            egui::pos2(rect.left() + 8.0, cy),
                            *k,
                        );
                        text_x = rect.left() + 18.0;
                    }
                    ui.painter().text(
                        egui::pos2(text_x, cy),
                        egui::Align2::LEFT_CENTER,
                        &text,
                        egui::FontId::proportional(13.0),
                        color,
                    );
                } else if e.row < 0 {
                    // The Events row: a fixed label, not a resource — no rename,
                    // no resource picker, no drag-reorder.
                    ui.painter().text(
                        egui::pos2(rect.left() + 10.0, cy),
                        egui::Align2::LEFT_CENTER,
                        default_row_label(e.row),
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_rgb(206, 190, 164),
                    );
                } else {
                    let hot = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), cy - half),
                        egui::pos2(rect.right(), cy + half),
                    );
                    let resp = ui.interact(
                        hot,
                        ui.id().with(("gutter_row", e.plan_id.0, e.row)),
                        egui::Sense::click_and_drag(),
                    );
                    if resp.drag_started() && act.is_none() && dragging.is_none() {
                        act = Some(Act::StartDrag(e.plan_id, e.scope, e.row));
                    }
                    if resp.hovered() && dragging.is_none() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    let (text, color) = match name {
                        Some(n) => (n.clone(), egui::Color32::from_rgb(206, 190, 164)),
                        None => (
                            default_row_label(e.row),
                            egui::Color32::from_rgb(138, 128, 114),
                        ),
                    };
                    let mut text_x = rect.left() + 10.0;
                    if let Some(k) = kind {
                        theme::draw_resource_dot(
                            ui.painter(),
                            egui::pos2(rect.left() + 8.0, cy),
                            *k,
                        );
                        text_x = rect.left() + 18.0;
                    }
                    let hovered = resp.hovered();
                    ui.painter().text(
                        egui::pos2(text_x, cy),
                        egui::Align2::LEFT_CENTER,
                        &text,
                        egui::FontId::proportional(13.0),
                        if hovered {
                            egui::Color32::from_rgb(236, 224, 204)
                        } else {
                            color
                        },
                    );
                    if resp.clicked() && act.is_none() {
                        act = Some(Act::OpenPicker(e.plan_id, e.scope, e.row));
                    }

                    if picker_this {
                        let popup_id = ui.id().with(("gutter_picker", e.plan_id.0, e.row));
                        let popup_pos = egui::pos2(rect.right() + 2.0, cy - 4.0);
                        let area_resp = egui::Area::new(popup_id)
                            .fixed_pos(popup_pos)
                            .order(egui::Order::Foreground)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::new()
                                    .fill(theme::PANEL_HI)
                                    .stroke(egui::Stroke::new(1.0, theme::STROKE))
                                    .corner_radius(egui::CornerRadius::same(4))
                                    .inner_margin(egui::Margin::same(6))
                                    .show(ui, |ui| {
                                        ui.set_min_width(130.0);
                                        for (i, res_name) in known_resources.iter().enumerate() {
                                            let is_current = name
                                                .as_ref()
                                                .is_some_and(|n| n.eq_ignore_ascii_case(res_name));
                                            ui.horizontal(|ui| {
                                                if let Some(k) = resource_kinds[i] {
                                                    let (_, dot_rect) =
                                                        ui.allocate_space(egui::vec2(10.0, 16.0));
                                                    theme::draw_resource_dot(
                                                        ui.painter(),
                                                        dot_rect.center(),
                                                        k,
                                                    );
                                                } else {
                                                    ui.allocate_space(egui::vec2(10.0, 16.0));
                                                }
                                                let label_color = if is_current {
                                                    egui::Color32::from_rgb(255, 220, 160)
                                                } else {
                                                    egui::Color32::from_rgb(206, 190, 164)
                                                };
                                                let btn = ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(res_name)
                                                            .color(label_color)
                                                            .size(13.0),
                                                    )
                                                    .selectable(false)
                                                    .sense(egui::Sense::click()),
                                                );
                                                if btn.clicked() {
                                                    act = Some(Act::SelectResource(
                                                        e.plan_id,
                                                        e.scope,
                                                        e.row,
                                                        res_name.clone(),
                                                    ));
                                                }
                                            });
                                        }
                                        ui.add_space(4.0);
                                        ui.separator();
                                        ui.add_space(2.0);
                                        let add_btn = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new("+ Add New")
                                                    .color(egui::Color32::from_rgb(140, 180, 220))
                                                    .size(13.0),
                                            )
                                            .selectable(false)
                                            .sense(egui::Sense::click()),
                                        );
                                        if add_btn.clicked() {
                                            act = Some(Act::StartNew(e.plan_id, e.scope, e.row));
                                        }
                                        if name.is_some() {
                                            ui.add_space(2.0);
                                            let clear_btn = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new("Clear")
                                                        .color(egui::Color32::from_rgb(
                                                            180, 120, 100,
                                                        ))
                                                        .size(12.0),
                                                )
                                                .selectable(false)
                                                .sense(egui::Sense::click()),
                                            );
                                            if clear_btn.clicked() {
                                                act = Some(Act::SelectResource(
                                                    e.plan_id,
                                                    e.scope,
                                                    e.row,
                                                    String::new(),
                                                ));
                                            }
                                        }
                                    });
                            });
                        if ui.ctx().input(|i| i.pointer.any_pressed())
                            && !area_resp.response.rect.contains(
                                ui.ctx()
                                    .input(|i| i.pointer.interact_pos().unwrap_or_default()),
                            )
                            && act.is_none()
                        {
                            act = Some(Act::ClosePicker);
                        }
                    }
                }
            }

            // An active drag-reorder: track the pointer with a ghost of the
            // dragged name, mark the candidate slot, and drop on release. The
            // target comes from the pointer's world-Y so the drop works even
            // when the source row has scrolled out of the culled label range.
            if let Some((pid, sc, from)) = dragging {
                let released = !ui.ctx().input(|i| i.pointer.primary_down());
                let ptr = ui.ctx().input(|i| i.pointer.latest_pos());
                let lane_rows = entries
                    .iter()
                    .filter(|e| e.plan_id == pid && e.scope == sc)
                    .collect::<Vec<_>>();
                let lane = lane_rows.first().map(|e| {
                    let row0_y = e.world_y + e.row as f32 * rh;
                    // Floor at 0: the Events row (−1) is never a reorder target,
                    // and clamp(0, max) needs max ≥ 0 even in an Events-only lane.
                    let max_row = lane_rows.iter().map(|e| e.row).max().unwrap_or(0).max(0);
                    (row0_y, max_row)
                });
                if let (Some((row0_y, max_row)), Some(p)) = (lane, ptr) {
                    let world_y = cam_y - (p.y - win_h * 0.5) * scale;
                    let target = (((row0_y - world_y) / rh).round() as i32).clamp(0, max_row);
                    if released {
                        if act.is_none() {
                            act = Some(Act::DropRow(pid, sc, from, target));
                        }
                    } else {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        let ty = world_to_screen_y(row0_y - target as f32 * rh);
                        ui.painter().line_segment(
                            [
                                egui::pos2(rect.left() + 2.0, ty),
                                egui::pos2(rect.right() - 2.0, ty),
                            ],
                            egui::Stroke::new(1.5, theme::ACCENT),
                        );
                        let name = lane_rows
                            .iter()
                            .find(|e| e.row == from)
                            .and_then(|e| e.name.clone())
                            .unwrap_or_else(|| default_row_label(from));
                        ui.painter().text(
                            egui::pos2(rect.left() + 10.0, p.y),
                            egui::Align2::LEFT_CENTER,
                            name,
                            egui::FontId::proportional(13.0),
                            egui::Color32::from_rgba_unmultiplied(236, 224, 204, 200),
                        );
                    }
                } else if released && act.is_none() {
                    act = Some(Act::CancelDrag);
                }
            }
        });

    match act {
        Some(Act::OpenPicker(pid, sc, r)) => {
            rename.picker_open = Some((pid, sc, r));
            rename.editing = None;
            rename.buf.clear();
        }
        Some(Act::ClosePicker) => {
            rename.picker_open = None;
        }
        Some(Act::SelectResource(pid, sc, r, name)) => {
            commit_row_name(&mut model, &mut save, pid, sc, r, &name);
            rename.picker_open = None;
        }
        Some(Act::StartNew(pid, sc, r)) => {
            rename.picker_open = None;
            rename.editing = Some((pid, sc, r));
            rename.buf.clear();
        }
        Some(Act::CommitNew) => {
            if let Some((pid, sc, r)) = rename.editing {
                let raw = rename.buf.trim().to_string();
                commit_row_name(&mut model, &mut save, pid, sc, r, &raw);
            }
            rename.editing = None;
            rename.buf.clear();
        }
        Some(Act::CancelNew) => {
            rename.editing = None;
            rename.buf.clear();
        }
        Some(Act::StartDrag(pid, sc, r)) => {
            rename.drag = Some((pid, sc, r));
            rename.picker_open = None;
        }
        Some(Act::DropRow(pid, sc, from, to)) => {
            if from != to {
                apply_row_reorder(&mut model, &mut save, pid, sc, from, to);
            }
            rename.drag = None;
        }
        Some(Act::CancelDrag) => {
            rename.drag = None;
        }
        None => {}
    }
}
/// Applies a resource-row rename. Empty clears the name. If `raw` matches
/// another row's name in the same scope (case-insensitively), the two are the
/// same resource: this row's blocks move onto that row and no separate name is
/// kept. Otherwise the name is stored as this plan's override for the row.
fn commit_row_name(
    model: &mut model::Model,
    save: &mut db::SaveRequest,
    plan_id: model::PlanId,
    scope: Option<model::WorkBlockId>,
    row: i32,
    raw: &str,
) {
    let name = raw.trim().to_string();

    // A matching name on another row (resolved through inheritance) means the
    // user is pointing this row at an existing resource.
    let merge_target = if name.is_empty() {
        None
    } else {
        // Bound the search by the actual number of allocated rows so a named
        // resource on a high-numbered row is never silently missed.
        let named_row_count = model
            .plans
            .get(&plan_id)
            .and_then(|p| p.row_names.get(&scope))
            .map(|v| v.len() as i32)
            .unwrap_or(0);
        (0..named_row_count).find(|&other| {
            other != row
                && model
                    .resolved_row_name(plan_id, scope, other)
                    .is_some_and(|n| n.eq_ignore_ascii_case(&name))
        })
    };

    if let Some(target) = merge_target {
        let move_ids: Vec<model::WorkBlockId> = schedule::visible_blocks(model, plan_id, scope)
            .iter()
            .filter(|wb| model.block_row(plan_id, wb.id) == row)
            .map(|wb| wb.id)
            .collect();
        for id in move_ids {
            model.set_block_row(plan_id, id, target);
        }
        if let Some(plan) = model.plans.get_mut(&plan_id) {
            plan.set_row_name(scope, row, String::new());
        }
    } else {
        // Renaming this row may be renaming the resource itself: when the old
        // name's registry entry (type + time-off) has no other row using it
        // and the new name is unregistered, carry the entry to the new name
        // instead of orphaning it (#339). A shared name (other rows still use
        // it) or an already-registered new name keeps the old behavior — the
        // row forks off / points at the existing resource.
        let old = model
            .plans
            .get(&plan_id)
            .and_then(|p| p.row_name(scope, row))
            .map(|s| s.to_string());
        if let Some(old) = old {
            if !name.is_empty()
                && !old.eq_ignore_ascii_case(&name)
                && model.resource_by_name(&name).is_none()
                && model.resource_by_name(&old).is_some()
                && model.row_name_references(&old) == 1
            {
                model.rename_resource(&old, &name);
            }
        }
        if let Some(plan) = model.plans.get_mut(&plan_id) {
            plan.set_row_name(scope, row, name);
        }
    }

    save.mark();
}
/// New index for `row` after a drag-reorder that moves row `from` to `to`
/// (list-insert semantics: the rows between them shift one step toward
/// `from`; rows outside the span are untouched).
fn reordered_row(row: i32, from: i32, to: i32) -> i32 {
    if row == from {
        to
    } else if from < to && row > from && row <= to {
        row - 1
    } else if to < from && row >= to && row < from {
        row + 1
    } else {
        row
    }
}
/// Permutes a scope's row-name list for a `from`→`to` drag, growing it with
/// empty placeholders so both indices are addressable and trimming trailing
/// empties afterwards (mirroring `reordered_row`'s mapping).
fn reorder_row_names(names: &mut Vec<String>, from: i32, to: i32) {
    let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
        return;
    };
    if from == to {
        return;
    }
    let need = from.max(to) + 1;
    if names.len() < need {
        names.resize(need, String::new());
    }
    let moved = names.remove(from);
    names.insert(to, moved);
    while names.last().is_some_and(|s| s.is_empty()) {
        names.pop();
    }
}
/// Applies a gutter drag-reorder within `(plan, scope)`: row `from` moves to
/// `to` and the rows between shift one step. The blocks visible at that scope
/// follow their rows, the row-name list is permuted to match, and the result
/// autosaves. Per-plan by construction — a branch reorder never touches main.
fn apply_row_reorder(
    model: &mut model::Model,
    save: &mut db::SaveRequest,
    plan_id: model::PlanId,
    scope: Option<model::WorkBlockId>,
    from: i32,
    to: i32,
) {
    if from == to {
        return;
    }
    let moves: Vec<(model::WorkBlockId, i32)> = schedule::visible_blocks(model, plan_id, scope)
        .iter()
        .map(|wb| (wb.id, model.block_row(plan_id, wb.id)))
        .collect();
    for (id, row) in moves {
        let new_row = reordered_row(row, from, to);
        if new_row != row {
            model.set_block_row(plan_id, id, new_row);
        }
    }
    if let Some(names) = model
        .plans
        .get_mut(&plan_id)
        .and_then(|p| p.row_names.get_mut(&scope))
    {
        reorder_row_names(names, from, to);
    }
    save.mark();
}
/// Resource-gutter state: which row has an open picker popup and, when editing,
/// the text buffer for the row name.
#[derive(Resource, Default)]
pub struct RowRename {
    pub editing: Option<(model::PlanId, Option<model::WorkBlockId>, i32)>,
    pub buf: String,
    pub picker_open: Option<(model::PlanId, Option<model::WorkBlockId>, i32)>,
    /// An in-progress gutter drag-reorder: (plan, scope, source row).
    pub drag: Option<(model::PlanId, Option<model::WorkBlockId>, i32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_rename_carries_sole_use_resource_registration() {
        // Renaming the only row using "Jefff" to "Jeff" carries the registry
        // entry (type + time-off) instead of orphaning it (#339).
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();
        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        let scope: Option<model::WorkBlockId> = None;
        m.set_resource_kind("Jefff", model::ResourceType::Engineer);
        if let Some(rb) = m.resource_blocks.values_mut().find(|r| r.name == "Jefff") {
            rb.non_working_dates.push(model::NonWorkingDate {
                date: chrono::NaiveDate::from_ymd_opt(2026, 8, 3).unwrap(),
                description: "PTO".to_string(),
            });
        }
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(scope, 0, "Jefff".to_string());
        db::save_model(&conn, &m).unwrap();

        let mut save = db::SaveRequest::default();
        commit_row_name(&mut m, &mut save, plan_id, scope, 0, "Jeff");
        assert!(save.0, "rename marks the deferred save");

        let rb = m.resource_by_name("Jeff").expect("registration carried");
        assert_eq!(rb.resource_type, model::ResourceType::Engineer);
        assert_eq!(rb.non_working_dates.len(), 1, "PTO carried");
        assert!(
            m.resource_by_name("Jefff").is_none(),
            "no orphaned entry under the old name"
        );
        assert_eq!(m.plans[&plan_id].row_name(scope, 0), Some("Jeff"));
    }
    #[test]
    fn commit_rename_leaves_shared_resource_registration_alone() {
        // Two rows staff "Team A"; renaming ONE of them must not steal the
        // registry entry from the other.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();
        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        m.set_resource_kind("Team A", model::ResourceType::Team);
        {
            let p = m.plans.get_mut(&plan_id).unwrap();
            p.set_row_name(None, 0, "Team A".to_string());
            // Same name in a drilled scope elsewhere still counts as a user.
            p.set_row_name(Some(model::WorkBlockId(999)), 0, "Team A".to_string());
        }
        db::save_model(&conn, &m).unwrap();

        let mut save = db::SaveRequest::default();
        commit_row_name(&mut m, &mut save, plan_id, None, 0, "Team Alpha");

        assert!(
            m.resource_by_name("Team A").is_some(),
            "shared registration stays with the remaining rows"
        );
        assert!(m.resource_by_name("Team Alpha").is_none());
        assert_eq!(m.plans[&plan_id].row_name(None, 0), Some("Team Alpha"));
    }
    #[test]
    fn commit_rename_onto_registered_name_does_not_rename_registry() {
        // Pointing a row at an already-registered resource keeps both entries
        // intact — that's an assignment, not a rename.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();
        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        m.set_resource_kind("Old", model::ResourceType::Engineer);
        m.set_resource_kind("Existing", model::ResourceType::Team);
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(None, 0, "Old".to_string());
        db::save_model(&conn, &m).unwrap();

        let mut save = db::SaveRequest::default();
        commit_row_name(&mut m, &mut save, plan_id, None, 0, "Existing");

        assert!(m.resource_by_name("Old").is_some(), "Old entry untouched");
        assert_eq!(
            m.resource_kind("Existing"),
            Some(model::ResourceType::Team),
            "Existing keeps its own registration"
        );
    }
    #[test]
    fn commit_row_name_merges_blocks_onto_existing_row() {
        // Merge semantics: naming row A the same as row B moves A's placed blocks to B.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();

        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        let scope: Option<model::WorkBlockId> = None;

        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(scope, 3, "Backend".to_string());
        let block = m.create_work_block("Task");
        // duration_days must be > 0 so visible_blocks includes this block.
        m.work_blocks.get_mut(&block).unwrap().duration_days = 5;
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(block);
        m.set_block_row(plan_id, block, 7);
        db::save_model(&conn, &m).unwrap();

        let mut save = db::SaveRequest::default();
        commit_row_name(&mut m, &mut save, plan_id, scope, 7, "Backend");

        assert_eq!(
            m.block_row(plan_id, block),
            3,
            "block should move to the target row"
        );
        assert_eq!(
            m.plans[&plan_id].row_name(scope, 7),
            None,
            "source row name cleared"
        );
    }
    #[test]
    fn commit_row_name_merges_onto_row_beyond_old_64_cap() {
        // Regression: the old `0..64` scan missed named rows ≥ 64, silently
        // creating a duplicate instead of merging. Verified by the fix that
        // derives the bound from the actual `row_names` Vec length.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();

        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        let scope: Option<model::WorkBlockId> = None;

        // Row 100 — beyond the old 0..64 cap — is already named "Infra".
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(scope, 100, "Infra".to_string());
        let block = m.create_work_block("Deploy");
        // duration_days must be > 0 so visible_blocks includes this block.
        m.work_blocks.get_mut(&block).unwrap().duration_days = 3;
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(block);
        m.set_block_row(plan_id, block, 1);
        db::save_model(&conn, &m).unwrap();

        // Naming row 1 "Infra" must find the target at row 100.
        let mut save = db::SaveRequest::default();
        commit_row_name(&mut m, &mut save, plan_id, scope, 1, "Infra");

        assert_eq!(
            m.block_row(plan_id, block),
            100,
            "block must merge onto the high-numbered row, not duplicate"
        );
        assert_eq!(
            m.plans[&plan_id].row_name(scope, 1),
            None,
            "source row name cleared"
        );
    }
    #[test]
    fn reordered_row_insert_semantics() {
        // Moving row 0 down to 2: rows 1..=2 shift up one; outside untouched.
        assert_eq!(reordered_row(0, 0, 2), 2);
        assert_eq!(reordered_row(1, 0, 2), 0);
        assert_eq!(reordered_row(2, 0, 2), 1);
        assert_eq!(reordered_row(3, 0, 2), 3);
        // Moving row 3 up to 1: rows 1..=2 shift down one.
        assert_eq!(reordered_row(3, 3, 1), 1);
        assert_eq!(reordered_row(1, 3, 1), 2);
        assert_eq!(reordered_row(2, 3, 1), 3);
        assert_eq!(reordered_row(0, 3, 1), 0);
        assert_eq!(reordered_row(4, 3, 1), 4);
        // A no-move drag maps every row to itself.
        assert_eq!(reordered_row(5, 2, 2), 5);
        assert_eq!(reordered_row(2, 2, 2), 2);
    }
    #[test]
    fn reorder_row_names_moves_grows_and_trims() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Down-move permutes like reordered_row.
        let mut v = names(&["A", "B", "C"]);
        reorder_row_names(&mut v, 0, 2);
        assert_eq!(v, names(&["B", "C", "A"]));
        // Up-move.
        let mut v = names(&["A", "B", "C"]);
        reorder_row_names(&mut v, 2, 0);
        assert_eq!(v, names(&["C", "A", "B"]));
        // A short list grows so the drag is addressable, then trailing
        // empties are trimmed: only "A" moving to index 2 survives.
        let mut v = names(&["A"]);
        reorder_row_names(&mut v, 0, 2);
        assert_eq!(v, names(&["", "", "A"]));
        // Moving an unnamed high row up shifts names down and trims the tail.
        let mut v = names(&["A", "B"]);
        reorder_row_names(&mut v, 3, 0);
        assert_eq!(v, names(&["", "A", "B"]));
    }
    #[test]
    fn apply_row_reorder_moves_blocks_and_names_together() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();

        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        let scope: Option<model::WorkBlockId> = None;

        let block_on_row = |m: &mut model::Model, name: &str, row: i32| {
            let id = m.create_work_block(name);
            m.work_blocks.get_mut(&id).unwrap().duration_days = 5;
            m.plans.get_mut(&plan_id).unwrap().root_blocks.push(id);
            m.set_block_row(plan_id, id, row);
            id
        };
        let a = block_on_row(&mut m, "A", 0);
        let b = block_on_row(&mut m, "B", 1);
        let c = block_on_row(&mut m, "C", 2);
        for (row, name) in [(0, "Ann"), (1, "Bob"), (2, "Cat")] {
            m.plans
                .get_mut(&plan_id)
                .unwrap()
                .set_row_name(scope, row, name.to_string());
        }
        // A child block (visible only when drilled into A) sits on row 1 of
        // its own scope and must not move with a top-level reorder.
        let child = m.create_work_block("A.1");
        m.work_blocks.get_mut(&child).unwrap().duration_days = 2;
        m.work_blocks.get_mut(&child).unwrap().parent = Some(a);
        m.set_block_row(plan_id, child, 1);
        db::save_model(&conn, &m).unwrap();

        // Drag Ann's row (0) below Cat's (2).
        let mut save = db::SaveRequest::default();
        apply_row_reorder(&mut m, &mut save, plan_id, scope, 0, 2);
        assert!(save.0, "reorder marks the deferred save");

        assert_eq!(m.block_row(plan_id, a), 2, "dragged row's block lands at 2");
        assert_eq!(m.block_row(plan_id, b), 0, "rows between shift up");
        assert_eq!(m.block_row(plan_id, c), 1, "rows between shift up");
        assert_eq!(
            m.block_row(plan_id, child),
            1,
            "drilled-scope block is not visible at top level and stays put"
        );
        assert_eq!(m.plans[&plan_id].row_name(scope, 0), Some("Bob"));
        assert_eq!(m.plans[&plan_id].row_name(scope, 1), Some("Cat"));
        assert_eq!(m.plans[&plan_id].row_name(scope, 2), Some("Ann"));
    }
}
