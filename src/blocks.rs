use std::collections::{HashMap, HashSet};

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

const BLOCK_HEIGHT: f32 = 44.0;
/// Minimum logical block width (px) below which the inline name label is hidden.
const MIN_LABEL_WIDTH: f32 = 20.0;
/// Approximate pixel width per character at font_size 13 (used for truncation).
const LABEL_CHAR_WIDTH: f32 = 8.0;

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
    pub work_block_id: WorkBlockId,
}

/// Dark shadow layer rendered behind `BlockLabel` to ensure legibility on any
/// block color. Offset by 1 screen pixel (updated each frame to match zoom).
#[derive(Component)]
pub struct BlockLabelShadow {
    pub full_name: String,
    pub work_block_id: WorkBlockId,
}

/// Marker for the description-dot indicator at a block's top-right corner.
/// Carries `work_block_id` so `sync_description_dots` can locate and manage it
/// without traversing the parent–child hierarchy.
#[derive(Component)]
pub struct DescriptionDot {
    pub work_block_id: WorkBlockId,
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

/// Maps each currently-visible `WorkBlockId` to its `BlockSprite` entity.
///
/// Maintained by `reconcile_block_sprites` to allow incremental ECS updates:
/// only newly visible blocks are spawned; only removed blocks are despawned.
#[derive(Resource, Default)]
pub struct BlockSpriteMap {
    pub entities: HashMap<WorkBlockId, Entity>,
}

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

/// Reconciles `BlockSprite` entities against the current `VisibleBlocks` cache.
///
/// Fires only when the visible block set or order actually changes (not on every
/// model mutation such as drag or resize). Despawns entities for blocks that left
/// the visible set, spawns entities for newly visible blocks, and updates
/// `BlockSprite.row` in place for blocks that stayed but changed row position.
///
/// Transform, size, and color are kept current every frame by `sync_block_sprites`;
/// this system only manages entity lifetime and row order. Label `full_name` is
/// kept current by `sync_block_label_names`; description-dot presence is kept
/// current by `sync_description_dots`.
pub fn reconcile_block_sprites(
    mut commands: Commands,
    sa: Res<ScheduleAnalysis>,
    model: Res<model::Model>,
    visible_blocks: Res<schedule::VisibleBlocks>,
    mode: Res<schedule::TimelineViewMode>,
    mut sprite_map: ResMut<BlockSpriteMap>,
    mut sprite_q: Query<&mut BlockSprite>,
) {
    if !visible_blocks.is_changed() && !mode.is_changed() {
        return;
    }
    // In resource view the timeline shows resource rows instead of block rows.
    if *mode == schedule::TimelineViewMode::Resource {
        return;
    }

    let on_critical_path: std::collections::HashSet<WorkBlockId> =
        sa.critical_path.iter().copied().collect();
    let new_id_set: std::collections::HashSet<WorkBlockId> =
        visible_blocks.ids.iter().copied().collect();

    // Despawn entities for blocks no longer in the visible set.
    let removed: Vec<WorkBlockId> = sprite_map
        .entities
        .keys()
        .filter(|id| !new_id_set.contains(id))
        .copied()
        .collect();
    for id in removed {
        if let Some(entity) = sprite_map.entities.remove(&id) {
            commands.entity(entity).despawn();
        }
    }

    // Reconcile each visible block in row order.
    for (row, &id) in visible_blocks.ids.iter().enumerate() {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };

        if let Some(&entity) = sprite_map.entities.get(&id) {
            // Existing entity: update row in place. Transform and color are
            // handled every frame by `sync_block_sprites`.
            if let Ok(mut block_sprite) = sprite_q.get_mut(entity) {
                block_sprite.row = row;
            }
        } else {
            // New entity: spawn parent sprite + label and dot children.
            let width = wb.duration_days * PIXELS_PER_DAY;
            let x = wb.start_day * PIXELS_PER_DAY + width * 0.5;
            let y = -(row as f32) * ROW_HEIGHT;

            let color = if let Some([r, g, b]) = wb.color {
                Color::from(LinearRgba::new(r, g, b, 1.0))
            } else if on_critical_path.contains(&id) {
                Color::from(LinearRgba::new(3.0, 2.2, 0.1, 1.0))
            } else {
                Color::from(PALETTE[row % PALETTE.len()])
            };

            let mut block_cmd = commands.spawn((
                BlockSprite { work_block_id: id, row },
                Sprite {
                    color,
                    custom_size: Some(Vec2::new(width, BLOCK_HEIGHT)),
                    ..default()
                },
                Transform::from_xyz(x, y, 0.0),
            ));

            // Inline name label — only when the bar is wide enough to be readable.
            if width >= MIN_LABEL_WIDTH {
                let available_chars = ((width - 8.0) / LABEL_CHAR_WIDTH) as usize;
                let display = if wb.name.chars().count() > available_chars && available_chars > 0 {
                    let truncated: String =
                        wb.name.chars().take(available_chars.saturating_sub(1)).collect();
                    format!("{truncated}…")
                } else {
                    wb.name.clone()
                };
                let name = wb.name.clone();
                block_cmd.with_children(|parent| {
                    // Dark shadow for contrast — 1 screen-pixel offset (updated by sync_block_labels).
                    parent.spawn((
                        BlockLabelShadow { full_name: name.clone(), work_block_id: id },
                        Text2d::new(display.clone()),
                        TextFont { font_size: 13.0, ..default() },
                        TextColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
                        Anchor::CENTER,
                        Transform::from_xyz(0.0, 0.0, 0.08),
                    ));
                    // White main label centered in the block.
                    parent.spawn((
                        BlockLabel { full_name: name, work_block_id: id },
                        Text2d::new(display),
                        TextFont { font_size: 13.0, ..default() },
                        TextColor(Color::srgba(1.0, 1.0, 1.0, 1.0)),
                        Anchor::CENTER,
                        Transform::from_xyz(0.0, 0.0, 0.15),
                    ));
                });
            }

            // Small dot indicator at top-right corner when the block has notes.
            if !wb.description.is_empty() && width >= 12.0 {
                block_cmd.with_children(|parent| {
                    parent.spawn((
                        DescriptionDot { work_block_id: id },
                        Text2d::new("·"),
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.7)),
                        Anchor::TOP_RIGHT,
                        Transform::from_xyz(width * 0.5 - 2.0, BLOCK_HEIGHT * 0.5 - 1.0, 0.2),
                    ));
                });
            }

            sprite_map.entities.insert(id, block_cmd.id());
        }
    }
}

/// Keeps `BlockLabel::full_name` and `BlockLabelShadow::full_name` current when
/// a block is renamed in the model. `sync_block_labels` drives displayed text
/// from `full_name`; without this system a rename would not reflect until the
/// next `reconcile_block_sprites` fires (only on visible-set/order changes).
pub fn sync_block_label_names(
    model: Res<model::Model>,
    mut label_q: Query<&mut BlockLabel>,
    mut shadow_q: Query<&mut BlockLabelShadow>,
) {
    if !model.is_changed() {
        return;
    }
    for mut label in &mut label_q {
        if let Some(wb) = model.work_blocks.get(&label.work_block_id) {
            if label.full_name != wb.name {
                label.full_name = wb.name.clone();
            }
        }
    }
    for mut shadow in &mut shadow_q {
        if let Some(wb) = model.work_blocks.get(&shadow.work_block_id) {
            if shadow.full_name != wb.name {
                shadow.full_name = wb.name.clone();
            }
        }
    }
}

/// Adds or removes the `DescriptionDot` child entity when a block's description
/// changes from empty to non-empty (or vice versa).
///
/// `reconcile_block_sprites` only fires on visible-set changes, so description
/// edits between re-orders would otherwise leave the dot out of sync without
/// this system.
pub fn sync_description_dots(
    mut commands: Commands,
    model: Res<model::Model>,
    sprite_map: Res<BlockSpriteMap>,
    dot_q: Query<(Entity, &DescriptionDot)>,
) {
    if !model.is_changed() {
        return;
    }

    let existing_dots: HashMap<WorkBlockId, Entity> =
        dot_q.iter().map(|(e, dot)| (dot.work_block_id, e)).collect();

    for (&id, &sprite_entity) in &sprite_map.entities {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        let width = wb.duration_days * PIXELS_PER_DAY;
        let should_have_dot = !wb.description.is_empty() && width >= 12.0;

        match (should_have_dot, existing_dots.get(&id)) {
            (true, None) => {
                commands.entity(sprite_entity).with_children(|parent| {
                    parent.spawn((
                        DescriptionDot { work_block_id: id },
                        Text2d::new("·"),
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.7)),
                        Anchor::TOP_RIGHT,
                        Transform::from_xyz(width * 0.5 - 2.0, BLOCK_HEIGHT * 0.5 - 1.0, 0.2),
                    ));
                });
            }
            (false, Some(&dot_entity)) => {
                commands.entity(dot_entity).despawn();
            }
            _ => {}
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

/// Updates `BlockLabel` and `BlockLabelShadow` children each frame.
///
/// Counter-scales both so labels remain at constant screen-space size.
/// Applies LOD-based text and moves the shadow 1 screen-pixel down-right
/// (shadow offset = scale world units, which equals 1 screen pixel at all zooms).
pub fn sync_block_labels(
    cam_q: Query<&Projection, With<Camera2d>>,
    mut label_q: Query<(&BlockLabel, &mut Text2d, &mut Visibility, &mut Transform), Without<BlockLabelShadow>>,
    mut shadow_q: Query<(&BlockLabelShadow, &mut Text2d, &mut Visibility, &mut Transform), Without<BlockLabel>>,
) {
    let Ok(proj) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let scale = ortho.scale;

    for (label, mut text2d, mut vis, mut transform) in &mut label_q {
        transform.scale = Vec3::splat(scale);
        transform.translation = Vec3::new(0.0, 0.0, 0.15);
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

    for (shadow, mut text2d, mut vis, mut transform) in &mut shadow_q {
        transform.scale = Vec3::splat(scale);
        // Shift by 1 screen pixel — in local space that's `scale` world units.
        transform.translation = Vec3::new(scale, -scale, 0.08);
        if scale > LOD_FAR_MIN {
            *vis = Visibility::Hidden;
        } else {
            *vis = Visibility::Inherited;
            let display: String = if scale < LOD_CLOSE_MAX {
                shadow.full_name.clone()
            } else {
                shadow.full_name.chars().take(LOD_ABBREV_CHARS).collect()
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
    dep_drag: Res<DepDragState>,
) {
    // Yield when a dep-handle drag is in progress.
    if dep_drag.from.is_some() {
        return;
    }
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
        // Reset so a later empty-space click doesn't inherit a stale timestamp.
        *last_empty_click = 0.0;
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

/// World-space radius of the left/right dep-creation handles on block edges.
const HANDLE_RADIUS: f32 = 8.0;
/// Hit-test radius for dep handles — slightly larger than visual to aid clicking.
const HANDLE_HIT_PX: f32 = 10.0;

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
    dep_drag: Res<DepDragState>,
) {
    if dep_drag.from.is_some() {
        resize.dragging = None;
        return;
    }
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
    dep_drag: Res<DepDragState>,
) {
    if dep_drag.from.is_some() {
        drag.dragging = None;
        return;
    }
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
    visible_blocks: Res<schedule::VisibleBlocks>,
    existing: Query<Entity, With<ConflictOverlay>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    if sa.resource_conflicts.is_empty() {
        return;
    }

    // Row lookup: WorkBlockId → row index (same ordering as block sprites).
    let row_of: HashMap<WorkBlockId, usize> = visible_blocks
        .ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();
    let total_rows = visible_blocks.ids.len().max(1);

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

/// Identifies which kind of uncertainty visual an entity represents and which
/// work block it belongs to. Used as the reconciliation key so the system can
/// update existing sprites in place rather than despawn-all/respawn-all.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UncertaintyOverlay {
    /// Translucent warm-glow tail extending past the block's right edge to the
    /// pessimistic estimate end. Opacity scales with `(1 − confidence)`.
    PessimisticTail(WorkBlockId),
    /// Narrow white vertical bar inside the block at the optimistic estimate end.
    OptimisticMarker(WorkBlockId),
}

/// Reconciles uncertainty overlay sprites with the current visible-block state.
///
/// Updates existing overlay transforms/sizes/colors in place. Spawns overlays
/// for newly visible blocks and despawns overlays for blocks that are no longer
/// visible or whose geometry no longer warrants a tail or marker.
pub fn sync_uncertainty_overlays(
    mut commands: Commands,
    model: Res<model::Model>,
    visible_blocks: Res<schedule::VisibleBlocks>,
    mut overlay_q: Query<(Entity, &UncertaintyOverlay, &mut Transform, &mut Sprite)>,
) {
    if !model.is_changed() && !visible_blocks.is_changed() {
        return;
    }

    // Build an index of existing overlay entities by key.
    let existing: HashMap<UncertaintyOverlay, Entity> = overlay_q
        .iter()
        .map(|(e, k, _, _)| (*k, e))
        .collect();

    // Compute the desired set of overlays.
    struct Overlay {
        key: UncertaintyOverlay,
        pos: Vec3,
        size: Vec2,
        color: Color,
    }
    let mut desired: Vec<Overlay> = Vec::new();

    for (row, &id) in visible_blocks.ids.iter().enumerate() {
        let Some(wb) = model.work_blocks.get(&id) else { continue };
        let y = -(row as f32) * ROW_HEIGHT;
        let confidence = wb.estimate.confidence.clamp(0.0, 1.0);
        let tail_alpha = (1.0 - confidence) * 0.55;

        // Pessimistic tail — extends past the right edge to the pessimistic end.
        let x_right = (wb.start_day + wb.duration_days) * PIXELS_PER_DAY;
        let x_pes  = (wb.start_day + wb.estimate.pessimistic) * PIXELS_PER_DAY;
        let tail_w = (x_pes - x_right).max(0.0);
        if tail_w > 0.5 && tail_alpha > 0.01 {
            desired.push(Overlay {
                key:   UncertaintyOverlay::PessimisticTail(id),
                pos:   Vec3::new(x_right + tail_w * 0.5, y, -0.1),
                size:  Vec2::new(tail_w, BLOCK_HEIGHT * 0.65),
                color: Color::from(LinearRgba::new(1.8, 1.3, 0.5, tail_alpha)),
            });
        }

        // Optimistic marker — narrow bar at the optimistic estimate end.
        let x_left    = wb.start_day * PIXELS_PER_DAY;
        let x_opt_end = (wb.start_day + wb.estimate.optimistic) * PIXELS_PER_DAY;
        if x_opt_end > x_left + 1.0 && x_opt_end < x_right - 1.0 {
            desired.push(Overlay {
                key:   UncertaintyOverlay::OptimisticMarker(id),
                pos:   Vec3::new(x_opt_end, y, 0.3),
                size:  Vec2::new(2.0, BLOCK_HEIGHT * 0.9),
                color: Color::from(LinearRgba::new(1.4, 1.4, 1.4, 0.55)),
            });
        }
    }

    // Update existing overlays in place; spawn new ones.
    let mut live: HashSet<Entity> = HashSet::with_capacity(desired.len());
    for ov in &desired {
        if let Some(&entity) = existing.get(&ov.key) {
            if let Ok((_, _, mut t, mut s)) = overlay_q.get_mut(entity) {
                t.translation  = ov.pos;
                s.custom_size  = Some(ov.size);
                s.color        = ov.color;
            }
            live.insert(entity);
        } else {
            commands.spawn((
                ov.key,
                Sprite { color: ov.color, custom_size: Some(ov.size), ..default() },
                Transform::from_translation(ov.pos),
            ));
        }
    }

    // Despawn stale overlays (removed blocks, or tail/marker no longer warranted).
    for (key, entity) in &existing {
        if !live.contains(entity) {
            let _ = key; // key unused here but makes the pattern clear
            commands.entity(*entity).despawn();
        }
    }
}

// ── Priority borders ─────────────────────────────────────────────────────────

/// Draws priority-scaled border rings around block sprites.
///
/// - Low (0): no border
/// - Normal (1): 1 thin white ring at 40% opacity
/// - High (2): 2 bright white rings
/// - Critical (3): 3 rings in an HDR gold color for bloom effect
///
/// Each ring is 1 screen pixel wide; `scale` world units = 1 screen pixel
/// (at scale=1, 1 world unit = 1 screen pixel).
pub fn draw_block_borders(
    mut gizmos: Gizmos,
    model: Res<model::Model>,
    cam_q: Query<&Projection, With<Camera2d>>,
    block_q: Query<(&BlockSprite, &Transform, &Sprite)>,
) {
    let scale = cam_q
        .single()
        .ok()
        .and_then(|p| if let Projection::Orthographic(o) = p { Some(o.scale) } else { None })
        .unwrap_or(1.0);

    for (bs, transform, sprite) in &block_q {
        let Some(wb) = model.work_blocks.get(&bs.work_block_id) else { continue };

        let (rings, color) = match wb.priority {
            0 => continue,
            1 => (1usize, Color::srgba(1.0, 1.0, 1.0, 0.40)),
            2 => (2usize, Color::srgba(1.0, 1.0, 1.0, 0.80)),
            _ => (3usize, Color::from(LinearRgba::new(3.0, 2.2, 0.4, 1.0))),
        };

        let Some(size) = sprite.custom_size else { continue };
        let center = transform.translation.truncate();

        for i in 0..rings {
            let expand = (i as f32 + 1.0) * scale;
            let hw = size.x * 0.5 + expand;
            let hh = size.y * 0.5 + expand;
            let tl = center + Vec2::new(-hw, hh);
            let tr = center + Vec2::new(hw, hh);
            let br = center + Vec2::new(hw, -hh);
            let bl = center + Vec2::new(-hw, -hh);
            gizmos.line_2d(tl, tr, color);
            gizmos.line_2d(tr, br, color);
            gizmos.line_2d(br, bl, color);
            gizmos.line_2d(bl, tl, color);
        }
    }
}

// ── Dependency edges ──────────────────────────────────────────────────────────

/// Persistent state for the right-click drag-to-create-dependency gesture.
#[derive(Resource, Default)]
pub struct DepDragState {
    /// Source block the user started dragging from, `None` when idle.
    pub from: Option<WorkBlockId>,
    /// `true` → dragged from the right-edge handle (this block is predecessor).
    /// `false` → dragged from the left-edge handle (this block is successor).
    pub from_right: bool,
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
    visible_blocks: Res<schedule::VisibleBlocks>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
) {
    let geom: HashMap<WorkBlockId, BlockGeom> = visible_blocks
        .ids
        .iter()
        .enumerate()
        .filter_map(|(row, &id)| {
            let wb = model.work_blocks.get(&id)?;
            Some((
                wb.id,
                BlockGeom {
                    xl: wb.start_day * PIXELS_PER_DAY,
                    xr: (wb.start_day + wb.duration_days) * PIXELS_PER_DAY,
                    y: -(row as f32) * ROW_HEIGHT,
                },
            ))
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
            let src = if drag.from_right {
                Vec2::new(fg.xr, fg.y)
            } else {
                Vec2::new(fg.xl, fg.y)
            };
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

/// Draws left-edge (incoming) and right-edge (outgoing) handle circles on any
/// block the pointer is hovering over. Highlights whichever handle is closest.
/// Also keeps the source handle highlighted while a dep drag is in progress.
pub fn draw_block_handles(
    mut gizmos: Gizmos,
    model: Res<model::Model>,
    visible_blocks: Res<schedule::VisibleBlocks>,
    drag: Res<DepDragState>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
) {
    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_tr)) = camera.single() else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_tr, cursor) else { return };

    let cyan = Color::srgba(0.3, 0.9, 1.0, 0.9);   // left handle (incoming)
    let amber = Color::srgba(1.0, 0.75, 0.2, 0.9);  // right handle (outgoing)
    let white = Color::WHITE;

    for (row, &id) in visible_blocks.ids.iter().enumerate() {
        let Some(wb) = model.work_blocks.get(&id) else { continue };
        if wb.duration_days <= 0.0 {
            continue;
        }
        let y = -(row as f32) * ROW_HEIGHT;
        let xl = wb.start_day * PIXELS_PER_DAY;
        let xr = (wb.start_day + wb.duration_days) * PIXELS_PER_DAY;

        let is_source = drag.from == Some(wb.id);

        // Show handles when hovering over this block or it is the drag source.
        let half_h = BLOCK_HEIGHT * 0.5;
        let in_block = world_pos.x >= xl
            && world_pos.x <= xr
            && (world_pos.y - y).abs() <= half_h;

        if !in_block && !is_source {
            continue;
        }

        let left_pos = Vec2::new(xl, y);
        let right_pos = Vec2::new(xr, y);
        let near_left = (world_pos - left_pos).length() < HANDLE_HIT_PX;
        let near_right = (world_pos - right_pos).length() < HANDLE_HIT_PX;

        // Left handle: cyan unless highlighted.
        let (lc, lr) = if near_left || (is_source && !drag.from_right) {
            (white, HANDLE_RADIUS * 1.4)
        } else {
            (cyan, HANDLE_RADIUS)
        };
        gizmos.circle_2d(left_pos, lr, lc);

        // Right handle: amber unless highlighted.
        let (rc, rr) = if near_right || (is_source && drag.from_right) {
            (white, HANDLE_RADIUS * 1.4)
        } else {
            (amber, HANDLE_RADIUS)
        };
        gizmos.circle_2d(right_pos, rr, rc);
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

    // Left-click on a handle starts a dep drag (takes priority over block actions).
    if mouse.just_pressed(MouseButton::Left) {
        for (bs, tr, sp) in &block_query {
            let Some(size) = sp.custom_size else { continue };
            let center = tr.translation.truncate();
            let half = size * 0.5;
            let left_pos = Vec2::new(center.x - half.x, center.y);
            let right_pos = Vec2::new(center.x + half.x, center.y);

            if (world_pos - right_pos).length() < HANDLE_HIT_PX {
                drag.from = Some(bs.work_block_id);
                drag.from_right = true;
                return;
            }
            if (world_pos - left_pos).length() < HANDLE_HIT_PX {
                drag.from = Some(bs.work_block_id);
                drag.from_right = false;
                return;
            }
        }
        // No handle hit — clear any stale dep drag state so guards don't block
        // block selection on this frame.
        if !mouse.pressed(MouseButton::Left) {
            drag.from = None;
        }
    }

    // Left-click release: finish a handle-initiated dep drag.
    if mouse.just_released(MouseButton::Left) {
        if let Some(from_id) = drag.from.take() {
            if let Some(to_id) = block_at(world_pos) {
                if to_id != from_id {
                    let dep_type = dep_type_from_modifiers(&keyboard);
                    let (pred, succ) = if drag.from_right {
                        (from_id, to_id) // right handle → from is predecessor
                    } else {
                        (to_id, from_id) // left handle → from is successor
                    };
                    let already = model.dependencies.values().any(|d| {
                        d.predecessor == pred
                            && d.successor == succ
                            && d.dependency_type == dep_type
                    });
                    if !already {
                        model.create_dependency(pred, succ, dep_type);
                        if let Err(e) = crate::db::save_model(&conn, &model) {
                            error!("save_model failed: {e}");
                        }
                    }
                }
            }
        }
    }

    // Right-click drag: existing shortcut, always treats source as predecessor.
    if mouse.just_pressed(MouseButton::Right) {
        drag.from = block_at(world_pos);
        drag.from_right = true;
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
/// change triggers `sync_block_label_names` which updates `BlockLabel::full_name`
/// so the display text reflects the new name on the next frame.
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
/// - Plain Enter: creates a block with the typed name, clears the buffer, stays
///   in create mode so the user can immediately type the next name.
/// - Ctrl+Enter / Cmd+Enter: inserts a newline within the current block name.
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
            ui.label("↵ to create  ·  Ctrl+↵ for newline  ·  Esc to exit");
            let response = ui.add(
                egui::TextEdit::multiline(&mut state.text_buf)
                    .hint_text("Block name…")
                    .desired_width(240.0)
                    .desired_rows(2),
            );
            response.request_focus();
            if response.has_focus() {
                let plain_enter = ui.input(|i| {
                    i.key_pressed(egui::Key::Enter)
                        && !i.modifiers.ctrl
                        && !i.modifiers.command
                });
                if plain_enter {
                    if state.text_buf.ends_with('\n') {
                        state.text_buf.pop();
                    }
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
    }

    if exit_mode {
        state.active = false;
        state.text_buf.clear();
    }
}

/// Shows a stats tooltip when the pointer hovers over a block sprite.
/// Displays start day, end day, duration, estimate range, and (if set) the
/// block's description. Renders an egui Area near the cursor.
pub fn draw_block_tooltip(
    mut egui_ctx: EguiContexts,
    model: Res<model::Model>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    block_q: Query<(&BlockSprite, &Transform, &Sprite)>,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    if ctx.is_pointer_over_area() {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_transform)) = camera.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_transform, cursor_pos) else { return };

    for (block_sprite, transform, sprite) in &block_q {
        let Some(size) = sprite.custom_size else { continue };
        let center = transform.translation.truncate();
        let half = size * 0.5;
        if world_pos.x >= center.x - half.x
            && world_pos.x <= center.x + half.x
            && world_pos.y >= center.y - half.y
            && world_pos.y <= center.y + half.y
        {
            let Some(wb) = model.work_blocks.get(&block_sprite.work_block_id) else { continue };
            let Some(screen_pos) = ctx.pointer_hover_pos() else { return };
            let end_day = wb.start_day + wb.duration_days;
            let est = &wb.estimate;
            egui::Area::new(egui::Id::new("block_stats_tooltip"))
                .order(egui::Order::Tooltip)
                .fixed_pos(screen_pos + egui::Vec2::new(14.0, 14.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.strong(&wb.name);
                        ui.separator();
                        egui::Grid::new("block_tooltip_grid")
                            .num_columns(2)
                            .spacing([8.0, 2.0])
                            .show(ui, |ui| {
                                ui.label("Start:");
                                ui.label(format!("day {:.1}", wb.start_day));
                                ui.end_row();
                                ui.label("End:");
                                ui.label(format!("day {:.1}", end_day));
                                ui.end_row();
                                ui.label("Duration:");
                                ui.label(format!("{:.1} days", wb.duration_days));
                                ui.end_row();
                                ui.label("Estimate:");
                                ui.label(format!(
                                    "opt {:.1} / ml {:.1} / pess {:.1}",
                                    est.optimistic, est.most_likely, est.pessimistic
                                ));
                                ui.end_row();
                            });
                        if !wb.description.is_empty() {
                            ui.separator();
                            ui.label(&wb.description);
                        }
                    });
                });
            return;
        }
    }
}
