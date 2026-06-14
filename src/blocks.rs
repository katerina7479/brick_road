use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use bevy::sprite::Anchor;

use crate::{
    analysis::ScheduleAnalysis,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    db,
    model::{self, DependencyId, DependencyType, Estimate, WorkBlockId},
    schedule::{self, ViewScope},
};

const BLOCK_HEIGHT: f32 = 28.0;
/// Minimum logical block width (px) below which the inline name label is hidden.
const MIN_LABEL_WIDTH: f32 = 20.0;
/// Approximate pixel width per character at font_size 11 (used for truncation).
const LABEL_CHAR_WIDTH: f32 = 7.0;

/// ortho.scale below this → show full block name.
const LOD_CLOSE_MAX: f32 = 1.0;
/// ortho.scale above this → hide block name entirely.
const LOD_FAR_MIN: f32 = 3.0;
/// Characters shown in the medium-zoom abbreviated label.
const LOD_ABBREV_CHARS: usize = 3;

/// Inline name label rendered inside a block bar.
/// Stores the untruncated model name so `sync_block_labels` can recompute
/// the display string at any zoom level without querying the model.
#[derive(Component)]
pub struct BlockLabel {
    pub full_name: String,
}

/// HDR linear palette — one or more channels > 1.0 so the Bloom post-process fires.
const PALETTE: &[LinearRgba] = &[
    LinearRgba::new(2.0, 0.5, 0.1, 1.0), // amber
    LinearRgba::new(0.2, 1.8, 0.5, 1.0), // green
    LinearRgba::new(0.2, 0.8, 3.0, 1.0), // cyan
    LinearRgba::new(2.2, 0.3, 1.5, 1.0), // magenta
    LinearRgba::new(2.5, 1.8, 0.1, 1.0), // yellow
    LinearRgba::new(0.5, 0.5, 3.0, 1.0), // blue
];

/// HDR gold applied to every block on the critical path.
const CRITICAL_PATH_COLOR: LinearRgba = LinearRgba::new(3.0, 2.5, 0.0, 1.0);

/// Tracks the currently selected work block (if any).
#[derive(Resource, Default)]
pub struct SelectedBlock(pub Option<WorkBlockId>);

/// Tracks inline name-edit state: which block is being renamed and the live text buffer.
#[derive(Resource, Default)]
pub struct NameEditState {
    pub editing: Option<WorkBlockId>,
    pub text_buf: String,
    /// (block_id, elapsed_secs) of the most recent left-click on a block sprite.
    last_click: Option<(WorkBlockId, f32)>,
}

/// Marker: this sprite visualises one ScheduledBlock.
#[derive(Component)]
pub struct BlockSprite {
    pub work_block_id: WorkBlockId,
    pub row: usize,
}

/// Spawns (or re-spawns) one `Sprite` per `ScheduledBlock`.
/// Row is assigned by ascending `start_day`, then `WorkBlockId` for stability.
/// Should run once after the `Schedule` resource is first available, and
/// again whenever the schedule changes.
pub fn spawn_block_sprites(
    mut commands: Commands,
    sa: Res<ScheduleAnalysis>,
    model: Res<model::Model>,
    scope: Res<ViewScope>,
    existing: Query<Entity, With<BlockSprite>>,
) {
    if !model.is_changed() && !scope.is_changed() {
        return;
    }
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let ordered = schedule::visible_blocks(&model, &scope);

    let on_critical_path: std::collections::HashSet<WorkBlockId> =
        sa.critical_path.iter().copied().collect();

    for (row, wb) in ordered.iter().enumerate() {
        let width = wb.duration_days * PIXELS_PER_DAY;
        // Sprite origin is at its center in Bevy 2D.
        let x = wb.start_day * PIXELS_PER_DAY + width * 0.5;
        let y = -(row as f32) * ROW_HEIGHT;

        // Color hierarchy: user color > critical-path gold > palette default.
        let color = if let Some([r, g, b]) = wb.color {
            Color::from(LinearRgba::new(r, g, b, 1.0))
        } else if on_critical_path.contains(&wb.id) {
            Color::from(LinearRgba::new(3.0, 2.2, 0.1, 1.0))
        } else {
            Color::from(PALETTE[row % PALETTE.len()])
        };

        let mut block_cmd = commands.spawn((
            BlockSprite {
                work_block_id: wb.id,
                row,
            },
            Sprite {
                color,
                custom_size: Some(Vec2::new(width, BLOCK_HEIGHT)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
        ));

        // Inline name label — only when the bar is wide enough to be readable.
        if width >= MIN_LABEL_WIDTH {
            let available_chars = ((width - 4.0) / LABEL_CHAR_WIDTH) as usize;
            let display = if wb.name.chars().count() > available_chars && available_chars > 0 {
                let truncated: String =
                    wb.name.chars().take(available_chars.saturating_sub(1)).collect();
                format!("{truncated}…")
            } else {
                wb.name.clone()
            };
            block_cmd.with_children(|parent| {
                parent.spawn((
                    BlockLabel { full_name: wb.name.clone() },
                    Text2d::new(display),
                    TextFont { font_size: 11.0, ..default() },
                    TextColor(Color::srgba(1.0, 1.0, 1.0, 0.9)),
                    Anchor::CENTER_LEFT,
                    // Position from the parent center: 2px padding from the left edge.
                    Transform::from_xyz(-(width * 0.5) + 2.0, 0.0, 0.1),
                ));
            });
        }
    }
}

/// Recomputes `Transform`, `Sprite::custom_size`, and color every frame.
///
/// Color priority (highest wins):
///   1. Critical-path gold — block is on `schedule.critical_path`
///   2. Selection 2× — block is the currently selected block
///   3. Palette default
pub fn sync_block_sprites(
    sa: Res<ScheduleAnalysis>,
    model: Res<model::Model>,
    selected: Res<SelectedBlock>,
    camera_q: Query<&Projection, With<Camera2d>>,
    mut query: Query<(&BlockSprite, &mut Transform, &mut Sprite)>,
) {
    let ortho_scale = camera_q
        .single()
        .ok()
        .and_then(|p| if let Projection::Orthographic(o) = p { Some(o.scale) } else { None })
        .unwrap_or(1.0);
    let min_width = 8.0 * ortho_scale;

    let on_critical: std::collections::HashSet<WorkBlockId> =
        sa.critical_path.iter().copied().collect();

    for (block_sprite, mut transform, mut sprite) in &mut query {
        let Some(wb) = model.work_blocks.get(&block_sprite.work_block_id) else {
            continue;
        };
        let width = wb.duration_days * PIXELS_PER_DAY;
        // Expand to min_width before computing x so the sprite is always
        // left-anchored at start_day, not centered on the model midpoint.
        let visual_width = width.max(min_width);
        let x = wb.start_day * PIXELS_PER_DAY + visual_width * 0.5;
        let y = -(block_sprite.row as f32) * ROW_HEIGHT;
        transform.translation.x = x;
        transform.translation.y = y;
        sprite.custom_size = Some(Vec2::new(visual_width, BLOCK_HEIGHT));

        let base = PALETTE[block_sprite.row % PALETTE.len()];
        let id = block_sprite.work_block_id;
        // Color hierarchy: user color > critical-path gold > selected highlight > palette.
        sprite.color = if let Some([r, g, b]) = wb.color {
            Color::from(LinearRgba::new(r, g, b, 1.0))
        } else if on_critical.contains(&id) {
            Color::from(CRITICAL_PATH_COLOR)
        } else if selected.0 == Some(id) {
            Color::from(LinearRgba::new(
                base.red * 2.0,
                base.green * 2.0,
                base.blue * 2.0,
                1.0,
            ))
        } else {
            Color::from(base)
        };
    }
}

/// Updates `BlockLabel` children each frame: counter-scales the transform so
/// labels stay at constant screen-space size, and applies LOD-based text:
/// - ortho.scale < 1.0 (close): full block name
/// - 1.0 ≤ scale ≤ 3.0 (medium): first 3 characters
/// - scale > 3.0 (far): hidden
pub fn sync_block_labels(
    cam_q: Query<&Projection, With<Camera2d>>,
    mut label_q: Query<(&BlockLabel, &mut Text2d, &mut Visibility, &mut Transform)>,
) {
    let Ok(proj) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let scale = ortho.scale;

    for (label, mut text2d, mut vis, mut transform) in &mut label_q {
        transform.scale = Vec3::splat(scale);
        if scale > LOD_FAR_MIN {
            *vis = Visibility::Hidden;
        } else {
            *vis = Visibility::Inherited;
            let display: String = if scale < LOD_CLOSE_MAX {
                label.full_name.clone()
            } else {
                label.full_name.chars().take(LOD_ABBREV_CHARS).collect()
            };
            *text2d = Text2d::new(display);
        }
    }
}

/// Converts a left-click to a block selection.
///
/// Clicks that land inside egui areas (e.g. the side panel) are ignored.
/// Clicking the currently selected block deselects it; single-clicking empty
/// space deselects; double-clicking empty space (within 350 ms) creates a block.
#[allow(clippy::too_many_arguments)]
pub fn handle_block_selection(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut selected: ResMut<SelectedBlock>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
    name_edit: Res<NameEditState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
    time: Res<Time>,
    mut last_empty_click: Local<f32>,
) {
    // Yield to the inline editor while a rename is in progress.
    if name_edit.editing.is_some() {
        return;
    }

    // Guard: egui owns the pointer when the cursor is over any egui area.
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) else {
        return;
    };

    // Hit-test each block sprite against its axis-aligned bounding rect.
    let mut clicked: Option<WorkBlockId> = None;
    for (block_sprite, transform, sprite) in &block_query {
        let Some(size) = sprite.custom_size else {
            continue;
        };
        let center = transform.translation.truncate();
        let half = size * 0.5;
        if world_pos.x >= center.x - half.x
            && world_pos.x <= center.x + half.x
            && world_pos.y >= center.y - half.y
            && world_pos.y <= center.y + half.y
        {
            clicked = Some(block_sprite.work_block_id);
            break;
        }
    }

    if let Some(id) = clicked {
        // Re-clicking the selected block toggles it off; otherwise select it.
        selected.0 = if Some(id) == selected.0 { None } else { Some(id) };
    } else {
        // Empty space: single click deselects, double-click (≤350 ms) creates a block.
        let now = time.elapsed_secs();
        let is_double_click = now - *last_empty_click < 0.35;
        if is_double_click {
            // Reset so a subsequent third click doesn't trigger another creation.
            *last_empty_click = 0.0;
            let start_day = (world_pos.x / PIXELS_PER_DAY).max(0.0);
            // Snap to the nearest 0.5-day grid line.
            let start_day = (start_day * 2.0).round() / 2.0;
            let duration_days = 1.0f32;
            let est = Estimate {
                most_likely: duration_days,
                optimistic: duration_days * 0.7,
                pessimistic: duration_days * 1.5,
                confidence: 0.8,
            };
            let new_id = model.create_work_block("New Block", est);
            if let Some(wb) = model.work_blocks.get_mut(&new_id) {
                wb.start_day = start_day;
                wb.duration_days = duration_days;
            }
            if let Some(plan) = model.plans.values_mut().next() {
                plan.root_blocks.push(new_id);
            }
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
            selected.0 = Some(new_id);
        } else {
            *last_empty_click = now;
            selected.0 = None;
        }
    }
}

/// Tracks an in-progress block drag initiated by the user.
#[derive(Resource, Default)]
pub struct DragState {
    /// The block being dragged and the cursor's x-offset from the block's left edge (pixels).
    dragging: Option<(WorkBlockId, f32)>,
}

/// Tracks an in-progress right-edge resize drag.
#[derive(Resource, Default)]
pub struct ResizeDragState {
    dragging: Option<WorkBlockId>,
}

/// Pixels from the right edge of a block that count as the resize handle.
const EDGE_GRAB_PX: f32 = 8.0;

/// Drag the right edge of a block to resize its `duration_days`.
///
/// - Press: if the cursor is within `EDGE_GRAB_PX` of the right edge and inside
///   the block's Y bounds, begin resize (takes priority over the move drag).
/// - Held: update `duration_days` so the right edge tracks the cursor, snapped
///   to the nearest 0.5-day grid and clamped to ≥ 0.5.
/// - Release: cascade dependency constraints, persist to DB.
#[allow(clippy::too_many_arguments)]
pub fn handle_block_resize(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut resize: ResMut<ResizeDragState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
) {
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            resize.dragging = None;
            return;
        }
    }

    let Ok(window) = windows.single() else { return };
    let Ok((camera, camera_transform)) = camera.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else {
        resize.dragging = None;
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) else {
        return;
    };

    // Press: hit-test the right-edge handle.
    if mouse.just_pressed(MouseButton::Left) {
        resize.dragging = None;
        for (block_sprite, transform, sprite) in &block_query {
            let Some(size) = sprite.custom_size else { continue };
            let center = transform.translation.truncate();
            let half = size * 0.5;
            let right_x = center.x + half.x;
            if (world_pos.x - right_x).abs() <= EDGE_GRAB_PX
                && world_pos.y >= center.y - half.y
                && world_pos.y <= center.y + half.y
            {
                resize.dragging = Some(block_sprite.work_block_id);
                break;
            }
        }
        return;
    }

    // Held: update duration_days so the right edge follows the cursor.
    if mouse.pressed(MouseButton::Left) {
        if let Some(id) = resize.dragging {
            if let Some(wb) = model.work_blocks.get_mut(&id) {
                let raw_dur =
                    ((world_pos.x - wb.start_day * PIXELS_PER_DAY) / PIXELS_PER_DAY).max(0.5);
                wb.duration_days = (raw_dur * 2.0).round() / 2.0;
            }
        }
        return;
    }

    // Release: cascade constraints and persist.
    if mouse.just_released(MouseButton::Left) {
        if let Some(id) = resize.dragging.take() {
            schedule::cascade_dependencies(&mut model, id);
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
        }
    }
}

/// Center-drag a block left or right to reposition its `start_day`.
///
/// - Press: hit-test blocks, record offset from left edge, set selection.
///   Skipped if a resize drag is already in progress.
/// - Held: slide `start_day` to follow the cursor (clamped to ≥ 0).
/// - Release: cascade dependency constraints, persist to DB.
///
/// Clicks that land inside egui areas are ignored (same guard as selection).
#[allow(clippy::too_many_arguments)]
pub fn handle_block_drag(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut drag: ResMut<DragState>,
    mut selected: ResMut<SelectedBlock>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
    resize: Res<ResizeDragState>,
) {
    // Guard: egui owns the pointer when the cursor is over any egui area.
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            drag.dragging = None;
            return;
        }
    }

    let Ok(window) = windows.single() else { return };
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        drag.dragging = None;
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) else {
        return;
    };

    // Press: hit-test and start drag. Skip if a resize is already in progress.
    if mouse.just_pressed(MouseButton::Left) {
        drag.dragging = None;
        if resize.dragging.is_some() {
            return;
        }
        for (block_sprite, transform, sprite) in &block_query {
            let Some(size) = sprite.custom_size else { continue };
            let center = transform.translation.truncate();
            let half = size * 0.5;
            if world_pos.x >= center.x - half.x
                && world_pos.x <= center.x + half.x
                && world_pos.y >= center.y - half.y
                && world_pos.y <= center.y + half.y
            {
                let id = block_sprite.work_block_id;
                let start_px = model
                    .work_blocks
                    .get(&id)
                    .map(|wb| wb.start_day * PIXELS_PER_DAY)
                    .unwrap_or(0.0);
                // Offset preserves where within the block the user grabbed.
                drag.dragging = Some((id, world_pos.x - start_px));
                selected.0 = Some(id);
                break;
            }
        }
        return;
    }

    // Held: slide start_day to follow cursor.
    if mouse.pressed(MouseButton::Left) {
        if let Some((id, offset_px)) = drag.dragging {
            let new_start = ((world_pos.x - offset_px) / PIXELS_PER_DAY).max(0.0);
            if let Some(wb) = model.work_blocks.get_mut(&id) {
                wb.start_day = new_start;
            }
        }
        return;
    }

    // Release: cascade dependencies and persist.
    if mouse.just_released(MouseButton::Left) {
        if let Some((id, _)) = drag.dragging.take() {
            schedule::cascade_dependencies(&mut model, id);
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
        }
    }
}

/// Marker: this sprite visualises one `ResourceConflict` window.
#[derive(Component)]
pub struct ConflictOverlay;

/// Translucent red used for conflict overlays (behind blocks at z = −0.5).
const CONFLICT_COLOR: Color = Color::srgba(1.0, 0.12, 0.05, 0.38);

/// Vertical padding above/below the block height when sizing conflict overlays.
const CONFLICT_PADDING: f32 = 5.0;

/// Despawns all existing `ConflictOverlay` entities and re-spawns one per
/// `ResourceConflict` in `ScheduleAnalysis`. Each overlay is a translucent red
/// sprite placed behind blocks (z = −0.5) that spans the conflict time window
/// in x and the contributing blocks' row range in y.
pub fn sync_conflict_overlays(
    mut commands: Commands,
    sa: Res<ScheduleAnalysis>,
    model: Res<model::Model>,
    scope: Res<ViewScope>,
    existing: Query<Entity, With<ConflictOverlay>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    if sa.resource_conflicts.is_empty() {
        return;
    }

    // Row lookup: WorkBlockId → row index (same ordering as block sprites).
    let ordered = schedule::visible_blocks(&model, &scope);
    let row_of: HashMap<WorkBlockId, usize> =
        ordered.iter().enumerate().map(|(i, wb)| (wb.id, i)).collect();
    let total_rows = ordered.len().max(1);

    for conflict in &sa.resource_conflicts {
        let width = (conflict.window_end - conflict.window_start) * PIXELS_PER_DAY;
        if width <= 0.0 {
            continue;
        }

        // Compute the y-center and height to cover contributing block rows.
        let rows: Vec<usize> = conflict
            .contributing_blocks
            .iter()
            .filter_map(|id| row_of.get(id).copied())
            .collect();

        let (y_center, height) = if rows.is_empty() {
            // Fall back to covering all rows.
            let h = (total_rows as f32) * ROW_HEIGHT + CONFLICT_PADDING * 2.0;
            (-(total_rows as f32 - 1.0) * 0.5 * ROW_HEIGHT, h)
        } else {
            let min_row = *rows.iter().min().unwrap() as f32;
            let max_row = *rows.iter().max().unwrap() as f32;
            let y_top = -min_row * ROW_HEIGHT + BLOCK_HEIGHT * 0.5 + CONFLICT_PADDING;
            let y_bot = -max_row * ROW_HEIGHT - BLOCK_HEIGHT * 0.5 - CONFLICT_PADDING;
            let h = (y_top - y_bot).abs().max(BLOCK_HEIGHT + CONFLICT_PADDING * 2.0);
            ((y_top + y_bot) * 0.5, h)
        };

        let x_center = conflict.window_start * PIXELS_PER_DAY + width * 0.5;

        commands.spawn((
            ConflictOverlay,
            Sprite {
                color: CONFLICT_COLOR,
                custom_size: Some(Vec2::new(width, height)),
                ..default()
            },
            Transform::from_xyz(x_center, y_center, -0.5),
        ));
    }
}

// ── Estimate uncertainty overlays ────────────────────────────────────────────

/// Marker for estimate uncertainty overlay sprites.
#[derive(Component)]
pub struct UncertaintyOverlay;

/// Spawns two visual cues per visible block that encode estimate uncertainty:
///
/// - **Pessimistic tail**: a translucent warm-glow sprite extending rightward
///   from the block's right edge to `(start_day + pessimistic) * PPD`.
///   Opacity scales with `(1 − confidence)` so high-confidence blocks have
///   barely-visible tails.
///
/// - **Optimistic marker**: a narrow white vertical bar inside the block at
///   `(start_day + optimistic) * PPD`, only drawn when the optimistic end
///   falls meaningfully inside the block (i.e. `optimistic < duration_days`).
///
/// Re-spawns whenever the model or view scope changes.
pub fn sync_uncertainty_overlays(
    mut commands: Commands,
    model: Res<model::Model>,
    scope: Res<ViewScope>,
    existing: Query<Entity, With<UncertaintyOverlay>>,
) {
    if !model.is_changed() && !scope.is_changed() {
        return;
    }
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let ordered = schedule::visible_blocks(&model, &scope);

    for (row, wb) in ordered.iter().enumerate() {
        let y = -(row as f32) * ROW_HEIGHT;
        let confidence = wb.estimate.confidence.clamp(0.0, 1.0);
        let tail_alpha = (1.0 - confidence) * 0.55;

        // Pessimistic tail — extends past the right edge of the main block.
        let x_block_right = (wb.start_day + wb.duration_days) * PIXELS_PER_DAY;
        let x_pes_right = (wb.start_day + wb.estimate.pessimistic) * PIXELS_PER_DAY;
        let tail_w = (x_pes_right - x_block_right).max(0.0);

        if tail_w > 0.5 && tail_alpha > 0.01 {
            commands.spawn((
                UncertaintyOverlay,
                Sprite {
                    color: Color::from(LinearRgba::new(1.8, 1.3, 0.5, tail_alpha)),
                    custom_size: Some(Vec2::new(tail_w, BLOCK_HEIGHT * 0.65)),
                    ..default()
                },
                Transform::from_xyz(x_block_right + tail_w * 0.5, y, -0.1),
            ));
        }

        // Optimistic marker — narrow bar inside the block at the optimistic end.
        let x_block_left = wb.start_day * PIXELS_PER_DAY;
        let x_opt_end = (wb.start_day + wb.estimate.optimistic) * PIXELS_PER_DAY;
        if x_opt_end > x_block_left + 1.0 && x_opt_end < x_block_right - 1.0 {
            commands.spawn((
                UncertaintyOverlay,
                Sprite {
                    color: Color::from(LinearRgba::new(1.4, 1.4, 1.4, 0.55)),
                    custom_size: Some(Vec2::new(2.0, BLOCK_HEIGHT * 0.9)),
                    ..default()
                },
                Transform::from_xyz(x_opt_end, y, 0.3),
            ));
        }
    }
}

// ── Dependency edges ──────────────────────────────────────────────────────────

/// Persistent state for the right-click drag-to-create-dependency gesture.
#[derive(Resource, Default)]
pub struct DepDragState {
    /// Source block the user started dragging from, `None` when idle.
    pub from: Option<WorkBlockId>,
}

/// Screen-space geometry for one work block, computed once per frame.
struct BlockGeom {
    xl: f32,
    xr: f32,
    y: f32,
}

/// Draw all model dependency edges as arrows, plus the in-progress drag line.
///
/// Colors:
///   Violated   — red/orange
///   Satisfied  — dim cyan
///   In-progress drag — white
pub fn draw_dependency_edges(
    mut gizmos: Gizmos,
    model: Res<model::Model>,
    sa: Res<ScheduleAnalysis>,
    drag: Res<DepDragState>,
    scope: Res<ViewScope>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
) {
    let ordered = schedule::visible_blocks(&model, &scope);
    let geom: HashMap<WorkBlockId, BlockGeom> = ordered
        .iter()
        .enumerate()
        .map(|(row, wb)| {
            (
                wb.id,
                BlockGeom {
                    xl: wb.start_day * PIXELS_PER_DAY,
                    xr: (wb.start_day + wb.duration_days) * PIXELS_PER_DAY,
                    y: -(row as f32) * ROW_HEIGHT,
                },
            )
        })
        .collect();

    let violated: std::collections::HashSet<DependencyId> =
        sa.violations.iter().map(|v| v.dependency_id).collect();

    for dep in model.dependencies.values() {
        let (Some(pg), Some(sg)) = (geom.get(&dep.predecessor), geom.get(&dep.successor)) else {
            continue;
        };

        let (src, dst) = match dep.dependency_type {
            DependencyType::FinishToStart => (Vec2::new(pg.xr, pg.y), Vec2::new(sg.xl, sg.y)),
            DependencyType::StartToStart => (Vec2::new(pg.xl, pg.y), Vec2::new(sg.xl, sg.y)),
            DependencyType::FinishToFinish => (Vec2::new(pg.xr, pg.y), Vec2::new(sg.xr, sg.y)),
            DependencyType::StartToFinish => (Vec2::new(pg.xl, pg.y), Vec2::new(sg.xr, sg.y)),
        };

        let color = if violated.contains(&dep.id) {
            Color::srgba(1.0, 0.25, 0.1, 0.9)
        } else {
            Color::srgba(0.35, 0.85, 0.85, 0.65)
        };

        gizmos.line_2d(src, dst, color);
        draw_arrowhead(&mut gizmos, src, dst, color);
    }

    // In-progress drag line.
    if let Some(from_id) = drag.from {
        if let Some(fg) = geom.get(&from_id) {
            let Ok(window) = windows.single() else {
                return;
            };
            let Ok((cam, cam_tr)) = camera.single() else {
                return;
            };
            let Some(cursor) = window.cursor_position() else {
                return;
            };
            let Ok(world_pos) = cam.viewport_to_world_2d(cam_tr, cursor) else {
                return;
            };
            let src = Vec2::new(fg.xr, fg.y);
            gizmos.line_2d(src, world_pos, Color::WHITE);
            draw_arrowhead(&mut gizmos, src, world_pos, Color::WHITE);
        }
    }
}

fn draw_arrowhead(gizmos: &mut Gizmos, src: Vec2, dst: Vec2, color: Color) {
    let dir = (dst - src).normalize_or_zero();
    if dir == Vec2::ZERO {
        return;
    }
    let perp = Vec2::new(-dir.y, dir.x);
    gizmos.line_2d(dst, dst - dir * 8.0 + perp * 4.0, color);
    gizmos.line_2d(dst, dst - dir * 8.0 - perp * 4.0, color);
}

fn dep_type_from_modifiers(keys: &ButtonInput<KeyCode>) -> DependencyType {
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        DependencyType::StartToStart
    } else if keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
        DependencyType::FinishToFinish
    } else if keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]) {
        DependencyType::StartToFinish
    } else {
        DependencyType::FinishToStart
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_dep_drag(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut drag: ResMut<DepDragState>,
    mut model: ResMut<model::Model>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
    conn: NonSend<rusqlite::Connection>,
) {
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            drag.from = None;
            return;
        }
    }

    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_tr)) = camera.single() else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_tr, cursor) else { return };

    let block_at = |pos: Vec2| -> Option<WorkBlockId> {
        for (bs, tr, sp) in &block_query {
            let Some(size) = sp.custom_size else { continue };
            let center = tr.translation.truncate();
            let half = size * 0.5;
            if pos.x >= center.x - half.x
                && pos.x <= center.x + half.x
                && pos.y >= center.y - half.y
                && pos.y <= center.y + half.y
            {
                return Some(bs.work_block_id);
            }
        }
        None
    };

    if mouse.just_pressed(MouseButton::Right) {
        drag.from = block_at(world_pos);
    }

    if mouse.just_released(MouseButton::Right) {
        if let Some(from_id) = drag.from.take() {
            if let Some(to_id) = block_at(world_pos) {
                if to_id != from_id {
                    let dep_type = dep_type_from_modifiers(&keyboard);
                    let already = model.dependencies.values().any(|d| {
                        d.predecessor == from_id
                            && d.successor == to_id
                            && d.dependency_type == dep_type
                    });
                    if !already {
                        model.create_dependency(from_id, to_id, dep_type);
                        if let Err(e) = crate::db::save_model(&conn, &model) {
                            error!("save_model failed: {e}");
                        }
                    }
                }
            }
        }
    }
}

/// Detects double-click on a block sprite and enters inline name-edit mode
/// by populating `NameEditState`.
///
/// Must run before `handle_block_selection` so the guard there sees the updated
/// `editing` flag on the same frame the double-click fires.
pub fn handle_name_edit(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    model: Res<model::Model>,
    mut name_edit: ResMut<NameEditState>,
    mut scope: ResMut<ViewScope>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
) {
    if name_edit.editing.is_some() {
        return;
    }
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }

    let Ok(window) = windows.single() else { return };
    let Ok((camera, camera_transform)) = camera.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };
    let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) else {
        return;
    };

    let now = time.elapsed_secs();

    // Double-click on a block sprite.
    // If the block has variants → drill into its children.
    // If the block has no variants → enter inline name-edit mode.
    for (block_sprite, transform, sprite) in &block_query {
        let Some(size) = sprite.custom_size else { continue };
        let center = transform.translation.truncate();
        let half = size * 0.5;
        if world_pos.x >= center.x - half.x
            && world_pos.x <= center.x + half.x
            && world_pos.y >= center.y - half.y
            && world_pos.y <= center.y + half.y
        {
            let id = block_sprite.work_block_id;
            if let Some((last_id, last_time)) = name_edit.last_click {
                if last_id == id && now - last_time < 0.4 {
                    name_edit.last_click = None;
                    if let Some(wb) = model.work_blocks.get(&id) {
                        if !wb.variants.is_empty() {
                            // Push onto the stack to drill into this block's children.
                            scope.scope_stack.push(id);
                        } else {
                            // Rename the block inline.
                            name_edit.editing = Some(id);
                            name_edit.text_buf = wb.name.clone();
                        }
                    }
                    return;
                }
            }
            name_edit.last_click = Some((id, now));
            return;
        }
    }

    name_edit.last_click = None;
}

/// Renders an egui `TextEdit` overlay anchored to the editing block's screen
/// position while `NameEditState::editing` is `Some`. Commits on Enter or
/// focus-loss; cancels on Escape. On commit, persists to model + DB; the model
/// change triggers `spawn_block_sprites` which re-creates the `BlockLabel` with
/// the updated name automatically.
pub fn draw_name_edit_overlay(
    mut contexts: EguiContexts,
    mut name_edit: ResMut<NameEditState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    block_query: Query<(&BlockSprite, &Transform)>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Some(edit_id) = name_edit.editing else { return };

    let Ok(_window) = windows.single() else { return };
    let Ok((camera, camera_transform)) = camera.single() else { return };

    // Locate the block sprite's screen position to anchor the overlay.
    let mut screen_pos = egui::pos2(50.0, 200.0);
    for (bs, transform) in &block_query {
        if bs.work_block_id == edit_id {
            if let Ok(vp) = camera.world_to_viewport(camera_transform, transform.translation) {
                screen_pos = egui::pos2(vp.x, vp.y - 10.0);
            }
            break;
        }
    }

    let Ok(ctx) = contexts.ctx_mut() else { return };

    let escaped = keys.just_pressed(KeyCode::Escape);
    // Check Enter via Bevy's key state — egui's TextEdit::singleline does not
    // reliably fire lost_focus() on Enter in bevy_egui, so we handle it
    // explicitly here in parallel with the Escape path.
    let entered = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);
    let mut commit = false;

    if !escaped && !entered {
        egui::Area::new(egui::Id::new("name_edit_overlay"))
            .fixed_pos(screen_pos)
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut name_edit.text_buf)
                        .min_size(egui::Vec2::new(120.0, 20.0)),
                );
                response.request_focus();
                // Fallback: commit if focus is lost through any other means
                // (Tab, clicking outside the overlay, etc.).
                if response.lost_focus() {
                    commit = true;
                }
            });
    } else if entered {
        commit = true;
    }

    if escaped {
        name_edit.editing = None;
        name_edit.text_buf.clear();
    } else if commit {
        let new_name = name_edit.text_buf.trim().to_string();
        if !new_name.is_empty() {
            if let Some(wb) = model.work_blocks.get_mut(&edit_id) {
                wb.name = new_name;
            }
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
        }
        name_edit.editing = None;
        name_edit.text_buf.clear();
    }
}

/// Tracks a pending block deletion waiting for user confirmation.
#[derive(Resource, Default)]
pub struct DeleteConfirmState {
    pub pending: Option<WorkBlockId>,
}

/// Detects Delete/Backspace key press and queues the selected block for
/// confirmation. Skipped while a name edit or egui text input is active.
pub fn handle_block_delete(
    mut egui_ctx: EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Res<SelectedBlock>,
    name_edit: Res<NameEditState>,
    mut delete_confirm: ResMut<DeleteConfirmState>,
) {
    if name_edit.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            return;
        }
    }
    if keyboard.just_pressed(KeyCode::Delete) || keyboard.just_pressed(KeyCode::Backspace) {
        if let Some(id) = selected.0 {
            delete_confirm.pending = Some(id);
        }
    }
}

/// Shows a confirmation dialog while `DeleteConfirmState::pending` is set.
/// On confirm: removes the block and all references from the model, persists to
/// DB, and clears the selection. On cancel: dismisses without deleting.
pub fn draw_delete_confirm_overlay(
    mut contexts: EguiContexts,
    mut delete_confirm: ResMut<DeleteConfirmState>,
    mut model: ResMut<model::Model>,
    mut selected: ResMut<SelectedBlock>,
    conn: NonSend<rusqlite::Connection>,
) {
    let Some(pending_id) = delete_confirm.pending else { return };

    let block_name = model
        .work_blocks
        .get(&pending_id)
        .map(|wb| wb.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut confirmed = false;
    let mut cancelled = false;

    egui::Window::new("Confirm Delete")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("Delete \"{}\"?", block_name));
            ui.label("This will also remove all its dependencies.");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });

    if confirmed {
        model.work_blocks.remove(&pending_id);
        model
            .dependencies
            .retain(|_, dep| dep.predecessor != pending_id && dep.successor != pending_id);
        for plan in model.plans.values_mut() {
            plan.root_blocks.retain(|&bid| bid != pending_id);
            plan.selected_variants.remove(&pending_id);
            plan.allocations.retain(|a| a.work_block_id != pending_id);
        }
        for variant in model.variants.values_mut() {
            variant.children.retain(|&bid| bid != pending_id);
        }
        // Remove variants owned by the deleted block to avoid orphans.
        model.variants.retain(|_, v| v.parent != pending_id);

        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
        selected.0 = None;
        delete_confirm.pending = None;
    } else if cancelled {
        delete_confirm.pending = None;
    }
}

/// State for rapid block creation mode (activated with `N`).
#[derive(Resource, Default)]
pub struct CreateModeState {
    pub active: bool,
    pub text_buf: String,
}

/// Toggles create mode with the `N` key. Skipped while a name edit or any
/// egui text input is active so `N` can be typed freely in those contexts.
pub fn handle_create_mode_toggle(
    mut egui_ctx: EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    name_edit: Res<NameEditState>,
    mut state: ResMut<CreateModeState>,
) {
    if name_edit.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            return;
        }
    }
    if keyboard.just_pressed(KeyCode::KeyN) {
        state.active = !state.active;
        if !state.active {
            state.text_buf.clear();
        }
    }
}

/// Exits create mode when the user left-clicks on the timeline (outside egui).
pub fn handle_create_mode_click_exit(
    mut egui_ctx: EguiContexts,
    mouse: Res<ButtonInput<MouseButton>>,
    mut state: ResMut<CreateModeState>,
) {
    if !state.active {
        return;
    }
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if !ctx.is_pointer_over_area() {
            state.active = false;
            state.text_buf.clear();
        }
    }
}

/// Renders the quick-create overlay while create mode is active.
///
/// - Enter: creates a block with the typed name, clears the buffer, stays in
///   create mode so the user can immediately type the next name.
/// - Escape: exits create mode.
///
/// New blocks are placed at day 0 with a 1-day default duration; the user can
/// drag and resize them after bulk entry.
pub fn draw_create_mode_overlay(
    mut contexts: EguiContexts,
    mut state: ResMut<CreateModeState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
) {
    if !state.active {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut create_block = false;
    let mut exit_mode = false;

    egui::Window::new("Quick Create  [N]")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
        .show(ctx, |ui| {
            ui.label("↵ to create  ·  Esc to exit");
            let response = ui.add(
                egui::TextEdit::singleline(&mut state.text_buf)
                    .hint_text("Block name…")
                    .desired_width(240.0),
            );
            response.request_focus();
            if response.has_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    create_block = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    exit_mode = true;
                }
            }
        });

    if create_block {
        let name = state.text_buf.trim().to_string();
        if !name.is_empty() {
            let est = Estimate {
                most_likely: 1.0,
                optimistic: 0.7,
                pessimistic: 1.5,
                confidence: 0.8,
            };
            let new_id = model.create_work_block(name, est);
            if let Some(plan) = model.plans.values_mut().next() {
                plan.root_blocks.push(new_id);
            }
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
        }
        state.text_buf.clear();
        // Stay in create mode — ready for the next block name.
    }

    if exit_mode {
        state.active = false;
        state.text_buf.clear();
    }
}
