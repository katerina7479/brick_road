use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use bevy::sprite::Anchor;
use bevy::window::{CursorIcon, SystemCursorIcon};

use crate::{
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    db, graph,
    model::{self, Day, DependencyType, Estimate, PlanId, WorkBlockId},
    schedule::{self, ViewScope},
};

const BLOCK_HEIGHT: f32 = 28.0;
/// Minimum logical block width (px) below which the inline name label is hidden.
const MIN_LABEL_WIDTH: f32 = 20.0;
/// Approximate pixel width per character at font_size 13 (used for truncation).
const LABEL_CHAR_WIDTH: f32 = 8.0;

/// ortho.scale above this → hide block name entirely; also the start of dep-edge fade.
const LOD_FAR_MIN: f32 = 6.0;
/// ortho.scale above this → dependency edges are fully hidden.
const LOD_DEP_HIDE: f32 = 10.0;

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
    LinearRgba::new(0.25, 0.65, 2.80, 1.0), // indigo
    LinearRgba::new(0.20, 1.70, 0.80, 1.0), // emerald
    LinearRgba::new(2.20, 0.55, 0.15, 1.0), // orange
    LinearRgba::new(1.50, 0.30, 2.20, 1.0), // violet
    LinearRgba::new(0.15, 1.50, 1.60, 1.0), // teal
    LinearRgba::new(2.40, 1.60, 0.10, 1.0), // gold
];

/// Tracks the currently selected work block (if any).
#[derive(Resource, Default)]
pub struct SelectedBlock(pub Option<WorkBlockId>);

/// Tracks the currently selected dependency edge (for click-to-delete).
#[derive(Resource, Default)]
pub struct SelectedDependency(pub Option<model::DependencyId>);

/// Maps each currently-visible `WorkBlockId` to its `BlockSprite` entity.
///
/// Maintained by `reconcile_block_sprites` to allow incremental ECS updates:
/// only newly visible blocks are spawned; only removed blocks are despawned.
#[derive(Resource, Default)]
pub struct BlockSpriteMap {
    pub entities: HashMap<WorkBlockId, Entity>,
}

/// Which comparison plan (if any) is overlaid on the timeline.
/// Written by the side panel UI; read by `sync_compare_overlays`.
#[derive(Resource, Default)]
pub struct ComparePlanState {
    pub compare_plan_id: Option<PlanId>,
}

/// Tracks ghost sprite entities for the comparison plan overlay.
#[derive(Resource, Default)]
pub struct CompareBlockSpriteMap {
    pub entities: HashMap<WorkBlockId, Entity>,
}

/// Marker for a ghost block sprite showing how a block is placed in the comparison plan.
#[derive(Component)]
pub struct CompareBlockSprite {
    pub work_block_id: WorkBlockId,
}


/// Faded palette for branch ghost lanes — distinct colors, lower saturation
/// than the main block palette so they read as "alternative" rather than active.
pub const BRANCH_PALETTE: &[LinearRgba] = &[
    LinearRgba::new(0.30, 0.80, 1.80, 1.0), // sky blue
    LinearRgba::new(0.80, 1.60, 0.40, 1.0), // lime
    LinearRgba::new(1.80, 0.50, 1.40, 1.0), // rose
    LinearRgba::new(1.60, 1.20, 0.30, 1.0), // gold
];


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
    /// Explicit vertical lane (can be negative — above the baseline).
    pub row: i32,
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
    model: Res<model::Model>,
    visible_blocks: Res<schedule::VisibleBlocks>,
    mut sprite_map: ResMut<BlockSpriteMap>,
    mut sprite_q: Query<&mut BlockSprite>,
) {
    if !visible_blocks.is_changed() {
        return;
    }

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

    // Reconcile each visible block at its explicit, user-assigned lane. Rows are
    // a real persisted field now (freeform), not derived from sort order, so
    // blocks stay exactly where the user puts them.
    for &id in &visible_blocks.ids {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        let row = wb.row;

        if let Some(&entity) = sprite_map.entities.get(&id) {
            // Existing entity: update row in place. Transform and color are
            // handled every frame by `sync_block_sprites`.
            if let Ok(mut block_sprite) = sprite_q.get_mut(entity) {
                block_sprite.row = row;
            }
        } else {
            // New entity: spawn parent sprite + label and dot children.
            let width = wb.duration_days as f32 * PIXELS_PER_DAY;
            let x = wb.start_day as f32 * PIXELS_PER_DAY + width * 0.5;
            let y = -(row as f32) * ROW_HEIGHT;

            let color = if let Some([r, g, b]) = wb.color {
                Color::from(LinearRgba::new(r, g, b, 1.0))
            } else {
                Color::from(PALETTE[row.rem_euclid(PALETTE.len() as i32) as usize])
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
        let width = wb.duration_days as f32 * PIXELS_PER_DAY;
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
    model: Res<model::Model>,
    selected: Res<SelectedBlock>,
    today: Res<schedule::TodayMarker>,
    camera_q: Query<&Projection, With<Camera2d>>,
    mut query: Query<(&BlockSprite, &mut Transform, &mut Sprite)>,
) {
    let ortho_scale = camera_q
        .single()
        .ok()
        .and_then(|p| if let Projection::Orthographic(o) = p { Some(o.scale) } else { None })
        .unwrap_or(1.0);
    let min_width = 8.0 * ortho_scale;

    for (block_sprite, mut transform, mut sprite) in &mut query {
        let Some(wb) = model.work_blocks.get(&block_sprite.work_block_id) else {
            continue;
        };
        let width = wb.duration_days as f32 * PIXELS_PER_DAY;
        // Expand to min_width before computing x so the sprite is always
        // left-anchored at start_day, not centered on the model midpoint.
        let visual_width = width.max(min_width);
        let x = wb.start_day as f32 * PIXELS_PER_DAY + visual_width * 0.5;
        let y = -(block_sprite.row as f32) * ROW_HEIGHT;
        transform.translation.x = x;
        transform.translation.y = y;
        sprite.custom_size = Some(Vec2::new(visual_width, BLOCK_HEIGHT));

        let base = PALETTE[block_sprite.row.rem_euclid(PALETTE.len() as i32) as usize];
        let id = block_sprite.work_block_id;
        // Color hierarchy: user color > selected highlight > palette.
        sprite.color = if let Some([r, g, b]) = wb.color {
            Color::from(LinearRgba::new(r, g, b, 1.0))
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

        // Subtly mute blocks that are entirely in the past: partial desaturation
        // and slight dimming so they remain readable but look clearly historical.
        if wb.start_day + wb.duration_days <= today.day {
            let c = sprite.color.to_linear();
            let lum = 0.2126 * c.red + 0.7152 * c.green + 0.0722 * c.blue;
            // Blend 25% toward grayscale, dim to 85%, keep mostly opaque.
            let desat = 0.25_f32;
            let r = c.red + (lum - c.red) * desat;
            let g = c.green + (lum - c.green) * desat;
            let b = c.blue + (lum - c.blue) * desat;
            sprite.color = Color::from(LinearRgba::new(r * 0.85, g * 0.85, b * 0.85, 0.80));
        }
    }
}

/// Spawns, updates, and despawns ghost block sprites overlaid on the timeline
/// to show how the comparison plan schedules each block.
///
/// Fires when `ComparePlanState` or the model changes. Clears all ghost sprites
/// before rebuilding so there is never stale state. Ghost sprites sit at Z = -0.5
/// (behind active-plan blocks). Color encodes the relationship to the active plan:
///   faint gray  — same duration as active plan (timing may still differ)
///   amber       — duration differs between plans
///   coral       — block is only in the comparison plan
pub fn sync_compare_overlays(
    mut commands: Commands,
    model: Res<model::Model>,
    compare_state: Res<ComparePlanState>,
    mut map: ResMut<CompareBlockSpriteMap>,
    block_sprites: Query<&BlockSprite>,
) {
    if !compare_state.is_changed() && !model.is_changed() {
        return;
    }

    for (_, entity) in map.entities.drain() {
        commands.entity(entity).despawn();
    }

    let Some(cmp_id) = compare_state.compare_plan_id else { return };
    let Some(cmp_plan) = model.plans.get(&cmp_id).cloned() else { return };

    let cmp_graph = graph::build_graph(&model, &cmp_plan);
    let Ok(cmp_sched) = schedule::forward_pass(&model, &cmp_plan, &cmp_graph) else { return };

    // Build id → row from the active plan's current block sprites.
    let id_to_row: HashMap<WorkBlockId, i32> = block_sprites
        .iter()
        .map(|bs| (bs.work_block_id, bs.row))
        .collect();

    let max_row = id_to_row.values().copied().max().unwrap_or(0);
    let mut next_extra_row = max_row + 1;

    for (&id, cmp_block) in &cmp_sched.blocks {
        let row = if let Some(&r) = id_to_row.get(&id) {
            r
        } else {
            let r = next_extra_row;
            next_extra_row += 1;
            r
        };

        let width = cmp_block.duration_days as f32 * PIXELS_PER_DAY;
        let x = cmp_block.start_day as f32 * PIXELS_PER_DAY + width * 0.5;
        let y = -(row as f32) * ROW_HEIGHT;

        let active_dur = model.work_blocks.get(&id).map(|wb| wb.duration_days);
        let color = if active_dur.is_none() {
            // Compare-only block.
            Color::from(LinearRgba::new(1.2, 0.15, 0.15, 0.45))
        } else if active_dur == Some(cmp_block.duration_days) {
            // Same duration — ghost confirms the block matches.
            Color::from(LinearRgba::new(0.5, 0.5, 0.55, 0.22))
        } else {
            // Duration differs — amber ghost.
            Color::from(LinearRgba::new(1.4, 0.9, 0.05, 0.45))
        };

        let entity = commands
            .spawn((
                CompareBlockSprite { work_block_id: id },
                Sprite {
                    color,
                    custom_size: Some(Vec2::new(width.max(8.0), BLOCK_HEIGHT * 0.7)),
                    ..default()
                },
                Transform::from_xyz(x, y, -0.5),
            ))
            .id();
        map.entities.insert(id, entity);
    }
}

/// Updates `BlockLabel` and `BlockLabelShadow` children each frame.
///
/// Counter-scales both so labels remain at constant screen-space size.
/// Applies LOD-based text and moves the shadow 1 screen-pixel down-right
/// (shadow offset = scale world units, which equals 1 screen pixel at all zooms).
/// Fits a block label to the block's on-screen width. Returns the text to show
/// (truncated with "…" when needed) or `None` when the block is too narrow — or
/// the view too zoomed out — to show any label. Hidden labels are still readable
/// via the hover tooltip, so a label never spills past its block's edges.
fn fit_label(full_name: &str, block_world_w: f32, scale: f32) -> Option<String> {
    if scale > LOD_FAR_MIN {
        return None;
    }
    // The label renders at a constant screen size, so compare against the block's
    // width in screen pixels (world width / zoom scale).
    let screen_w = block_world_w / scale;
    let max_chars = ((screen_w - LABEL_CHAR_WIDTH) / LABEL_CHAR_WIDTH).floor();
    if max_chars < 1.0 {
        return None;
    }
    let max_chars = max_chars as usize;
    if full_name.chars().count() <= max_chars {
        Some(full_name.to_string())
    } else if max_chars == 1 {
        Some("…".to_string())
    } else {
        let kept: String = full_name.chars().take(max_chars - 1).collect();
        Some(format!("{kept}…"))
    }
}

pub fn sync_block_labels(
    cam_q: Query<&Projection, With<Camera2d>>,
    model: Res<model::Model>,
    mut label_q: Query<(&BlockLabel, &mut Text2d, &mut Visibility, &mut Transform), Without<BlockLabelShadow>>,
    mut shadow_q: Query<(&BlockLabelShadow, &mut Text2d, &mut Visibility, &mut Transform), Without<BlockLabel>>,
) {
    let Ok(proj) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let scale = ortho.scale;

    let block_width = |id: &WorkBlockId| -> f32 {
        model
            .work_blocks
            .get(id)
            .map(|wb| wb.duration_days as f32 * PIXELS_PER_DAY)
            .unwrap_or(0.0)
    };

    for (label, mut text2d, mut vis, mut transform) in &mut label_q {
        transform.scale = Vec3::splat(scale);
        transform.translation = Vec3::new(0.0, 0.0, 0.15);
        match fit_label(&label.full_name, block_width(&label.work_block_id), scale) {
            Some(display) => {
                *vis = Visibility::Inherited;
                *text2d = Text2d::new(display);
            }
            None => *vis = Visibility::Hidden,
        }
    }

    for (shadow, mut text2d, mut vis, mut transform) in &mut shadow_q {
        transform.scale = Vec3::splat(scale);
        // Shift by 1 screen pixel — in local space that's `scale` world units.
        transform.translation = Vec3::new(scale, -scale, 0.08);
        match fit_label(&shadow.full_name, block_width(&shadow.work_block_id), scale) {
            Some(display) => {
                *vis = Visibility::Inherited;
                *text2d = Text2d::new(display);
            }
            None => *vis = Visibility::Hidden,
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
    cam_proj: Query<&Projection, With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut selected: ResMut<SelectedBlock>,
    mut selected_dep: ResMut<SelectedDependency>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
    name_edit: Res<NameEditState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
    time: Res<Time>,
    mut last_empty_click: Local<f32>,
    dep_drag: Res<DepDragState>,
    active_schedule: Res<schedule::Schedule>,
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

    // Hit-test against the block sprites.
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
        selected_dep.0 = None;
    } else {
        // A dependency edge under the cursor takes priority — select it (for delete).
        let scale = cam_proj
            .single()
            .ok()
            .and_then(|p| if let Projection::Orthographic(o) = p { Some(o.scale) } else { None })
            .unwrap_or(1.0);
        if let Some(dep_id) = nearest_dep_edge(&model, world_pos, 7.0 * scale) {
            selected_dep.0 = Some(dep_id);
            selected.0 = None;
            *last_empty_click = 0.0;
            return;
        }
        selected_dep.0 = None;

        // Empty space: single click deselects, double-click (≤350 ms) creates a block.
        let now = time.elapsed_secs();
        let is_double_click = now - *last_empty_click < 0.35;
        if is_double_click {
            // Reset so a subsequent third click doesn't trigger another creation.
            *last_empty_click = 0.0;
            let raw_start = (world_pos.x / PIXELS_PER_DAY).max(0.0).round() as Day;
            let branch_min = model
                .plans
                .get(&active_schedule.plan_id)
                .and_then(|p| p.branch_start_day)
                .unwrap_or(0);
            let start_day = raw_start.max(branch_min);
            let est = Estimate {
                most_likely: 1,
                optimistic: 1,
                pessimistic: 2,
                confidence: 0.8,
            };
            let new_id = model.create_work_block("New Block", est);
            // Spawn at the double-clicked lane; the block stays where you put it.
            let row = (-world_pos.y / ROW_HEIGHT).round() as i32;
            if let Some(wb) = model.work_blocks.get_mut(&new_id) {
                wb.start_day = start_day;
                wb.duration_days = 5;
                wb.row = row;
            }
            let plan_id = active_schedule.plan_id;
            if let Some(plan) = model.plans.get_mut(&plan_id) {
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

/// World-space radius of the small left/right edge dep-creation handles.
const HANDLE_RADIUS: f32 = 4.0;
/// Hit-test radius for the dep handle — slightly larger than visual to aid clicking.
const HANDLE_HIT_PX: f32 = 8.0;

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
                    ((world_pos.x - wb.start_day as f32 * PIXELS_PER_DAY) / PIXELS_PER_DAY).max(1.0);
                wb.duration_days = raw_dur.round() as Day;
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
    active_schedule: Res<schedule::Schedule>,
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
                    .map(|wb| wb.start_day as f32 * PIXELS_PER_DAY)
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
            let branch_min = model
                .plans
                .get(&active_schedule.plan_id)
                .and_then(|p| p.branch_start_day)
                .unwrap_or(0);
            let new_start = ((world_pos.x - offset_px) / PIXELS_PER_DAY)
                .max(0.0)
                .round() as Day;
            let new_start = new_start.max(branch_min);
            // Vertical drag snaps the block to whichever lane the cursor is over
            // (negative rows sit above the baseline).
            let new_row = (-world_pos.y / ROW_HEIGHT).round() as i32;
            if let Some(wb) = model.work_blocks.get_mut(&id) {
                wb.start_day = new_start;
                wb.row = new_row;
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


// ── Past-portion overlay ──────────────────────────────────────────────────────

/// Reconciliation key for the dark overlay covering the past portion of a block
/// that straddles the today line.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PastPortionOverlay(pub WorkBlockId);

/// Reconciles past-portion overlays with the current visible-block state.
///
/// For each block straddling today (start_day < today < end_day), a
/// semi-transparent dark sprite covers the elapsed portion. Blocks entirely
/// in the past are desaturated in `sync_block_sprites`.
pub fn sync_past_overlays(
    mut commands: Commands,
    model: Res<model::Model>,
    today: Res<schedule::TodayMarker>,
    visible_blocks: Res<schedule::VisibleBlocks>,
    mut overlay_q: Query<(Entity, &PastPortionOverlay, &mut Transform, &mut Sprite)>,
) {
    if !model.is_changed() && !visible_blocks.is_changed() && !today.is_changed() {
        return;
    }

    let existing: HashMap<PastPortionOverlay, Entity> = overlay_q
        .iter()
        .map(|(e, k, _, _)| (*k, e))
        .collect();

    struct Overlay {
        key: PastPortionOverlay,
        pos: Vec3,
        size: Vec2,
    }
    let mut desired: Vec<Overlay> = Vec::new();

    for &id in &visible_blocks.ids {
        let Some(wb) = model.work_blocks.get(&id) else { continue };
        let end_day = wb.start_day + wb.duration_days;
        if wb.start_day >= today.day || end_day <= today.day {
            continue;
        }
        let past_width = (today.day - wb.start_day) as f32 * PIXELS_PER_DAY;
        let x_left = wb.start_day as f32 * PIXELS_PER_DAY;
        let y = -(wb.row as f32) * ROW_HEIGHT;
        desired.push(Overlay {
            key: PastPortionOverlay(id),
            pos: Vec3::new(x_left + past_width * 0.5, y, 0.2),
            size: Vec2::new(past_width, BLOCK_HEIGHT),
        });
    }

    let overlay_color = Color::from(LinearRgba::new(0.0, 0.0, 0.0, 0.5));
    let mut live: HashSet<Entity> = HashSet::with_capacity(desired.len());
    for ov in &desired {
        if let Some(&entity) = existing.get(&ov.key) {
            if let Ok((_, _, mut t, mut s)) = overlay_q.get_mut(entity) {
                t.translation = ov.pos;
                s.custom_size = Some(ov.size);
            }
            live.insert(entity);
        } else {
            commands.spawn((
                ov.key,
                Sprite { color: overlay_color, custom_size: Some(ov.size), ..default() },
                Transform::from_translation(ov.pos),
            ));
        }
    }

    for (&_key, &entity) in existing.iter().filter(|(_, e)| !live.contains(e)) {
        commands.entity(entity).despawn();
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
///   Edge — dim cyan
///   In-progress drag — white
pub fn draw_dependency_edges(
    mut gizmos: Gizmos,
    model: Res<model::Model>,
    drag: Res<DepDragState>,
    selected_dep: Res<SelectedDependency>,
    visible_blocks: Res<schedule::VisibleBlocks>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    cam_proj: Query<&Projection, With<Camera2d>>,
) {
    let geom: HashMap<WorkBlockId, BlockGeom> = visible_blocks
        .ids
        .iter()
        .filter_map(|&id| {
            let wb = model.work_blocks.get(&id)?;
            Some((
                wb.id,
                BlockGeom {
                    xl: wb.start_day as f32 * PIXELS_PER_DAY,
                    xr: (wb.start_day + wb.duration_days) as f32 * PIXELS_PER_DAY,
                    y: -(wb.row as f32) * ROW_HEIGHT,
                },
            ))
        })
        .collect();

    let ortho_scale = cam_proj
        .single()
        .ok()
        .and_then(|p| if let Projection::Orthographic(o) = p { Some(o.scale) } else { None })
        .unwrap_or(1.0);

    // Fade edges between LOD_FAR_MIN and LOD_DEP_HIDE; skip entirely beyond LOD_DEP_HIDE.
    let edge_alpha = if ortho_scale <= LOD_FAR_MIN {
        1.0_f32
    } else {
        ((LOD_DEP_HIDE - ortho_scale) / (LOD_DEP_HIDE - LOD_FAR_MIN)).clamp(0.0, 1.0)
    };

    if edge_alpha > 0.0 {
        for (dep_id, dep) in &model.dependencies {
            let (Some(pg), Some(sg)) = (geom.get(&dep.predecessor), geom.get(&dep.successor)) else {
                continue;
            };

            let (src, dst) = match dep.dependency_type {
                DependencyType::FinishToStart => (Vec2::new(pg.xr, pg.y), Vec2::new(sg.xl, sg.y)),
                DependencyType::StartToStart => (Vec2::new(pg.xl, pg.y), Vec2::new(sg.xl, sg.y)),
                DependencyType::FinishToFinish => (Vec2::new(pg.xr, pg.y), Vec2::new(sg.xr, sg.y)),
                DependencyType::StartToFinish => (Vec2::new(pg.xl, pg.y), Vec2::new(sg.xr, sg.y)),
            };

            let is_selected = selected_dep.0 == Some(*dep_id);
            let color = if is_selected {
                Color::srgba(1.7, 1.2, 0.25, edge_alpha.max(0.9)) // bright selection highlight
            } else {
                Color::srgba(0.35, 0.85, 0.85, 0.65 * edge_alpha)
            };

            gizmos.line_2d(src, dst, color);
            draw_arrowhead(&mut gizmos, src, dst, color);
            if is_selected {
                // Thicken by drawing offset parallels (gizmo lines are 1px).
                let n = (dst - src).normalize_or_zero();
                let off = Vec2::new(-n.y, n.x);
                gizmos.line_2d(src + off, dst + off, color);
                gizmos.line_2d(src - off, dst - off, color);
            }
        }
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

/// World-space endpoints (predecessor anchor → successor anchor) of a dependency
/// edge, mirroring `draw_dependency_edges`. `None` if a block is missing/unplaced.
fn dep_endpoints(model: &model::Model, dep: &model::Dependency) -> Option<(Vec2, Vec2)> {
    let pred = model.work_blocks.get(&dep.predecessor)?;
    let succ = model.work_blocks.get(&dep.successor)?;
    if pred.duration_days <= 0 || succ.duration_days <= 0 {
        return None;
    }
    let p_xl = pred.start_day as f32 * PIXELS_PER_DAY;
    let p_xr = (pred.start_day + pred.duration_days) as f32 * PIXELS_PER_DAY;
    let p_y = -(pred.row as f32) * ROW_HEIGHT;
    let s_xl = succ.start_day as f32 * PIXELS_PER_DAY;
    let s_xr = (succ.start_day + succ.duration_days) as f32 * PIXELS_PER_DAY;
    let s_y = -(succ.row as f32) * ROW_HEIGHT;
    let (src, dst) = match dep.dependency_type {
        DependencyType::FinishToStart => (Vec2::new(p_xr, p_y), Vec2::new(s_xl, s_y)),
        DependencyType::StartToStart => (Vec2::new(p_xl, p_y), Vec2::new(s_xl, s_y)),
        DependencyType::FinishToFinish => (Vec2::new(p_xr, p_y), Vec2::new(s_xr, s_y)),
        DependencyType::StartToFinish => (Vec2::new(p_xl, p_y), Vec2::new(s_xr, s_y)),
    };
    Some((src, dst))
}

/// Distance from point `p` to segment `a`–`b`.
fn point_segment_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// Id of the dependency whose edge is nearest `world_pos`, within `threshold`
/// world units (closest wins).
fn nearest_dep_edge(
    model: &model::Model,
    world_pos: Vec2,
    threshold: f32,
) -> Option<model::DependencyId> {
    let mut best: Option<(model::DependencyId, f32)> = None;
    for (id, dep) in &model.dependencies {
        let Some((src, dst)) = dep_endpoints(model, dep) else {
            continue;
        };
        let d = point_segment_dist(world_pos, src, dst);
        if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((*id, d));
        }
    }
    best.map(|(id, _)| id)
}

/// Dependency type implied by which edge you drag from and which edge you drop
/// on. `*_finish` is true for the finish (right) edge, false for the start (left)
/// edge. The drag source is always the predecessor, the drop target the successor.
fn dep_type_from_edges(source_finish: bool, target_finish: bool) -> DependencyType {
    match (source_finish, target_finish) {
        (true, false) => DependencyType::FinishToStart,
        (true, true) => DependencyType::FinishToFinish,
        (false, false) => DependencyType::StartToStart,
        (false, true) => DependencyType::StartToFinish,
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

    // One subtle connector dot; brightens slightly (no white, no big grow) when
    // hovered or while it's the drag source.
    let dot = Color::srgba(0.55, 0.62, 0.78, 0.65);
    let dot_hi = Color::srgba(0.80, 0.86, 0.98, 0.95);

    for &id in &visible_blocks.ids {
        let Some(wb) = model.work_blocks.get(&id) else { continue };
        if wb.duration_days <= 0 {
            continue;
        }
        let y = -(wb.row as f32) * ROW_HEIGHT;
        let xl = wb.start_day as f32 * PIXELS_PER_DAY;
        let xr = (wb.start_day + wb.duration_days) as f32 * PIXELS_PER_DAY;
        let half_h = BLOCK_HEIGHT * 0.5;

        let is_source = drag.from == Some(wb.id);

        // Show the handle when hovering this block or while it's the drag source.
        let in_block = world_pos.x >= xl
            && world_pos.x <= xr
            && (world_pos.y - y).abs() <= half_h;
        if !in_block && !is_source {
            continue;
        }

        // Small handles on the left (incoming) and right (outgoing) edges, both
        // the same muted color. Small enough to leave the right edge grabbable
        // for resizing above and below the dot.
        let left_pos = Vec2::new(xl, y);
        let right_pos = Vec2::new(xr, y);
        let near_left = (world_pos - left_pos).length() < HANDLE_HIT_PX;
        let near_right = (world_pos - right_pos).length() < HANDLE_HIT_PX;

        let (lc, lr) = if near_left || (is_source && !drag.from_right) {
            (dot_hi, HANDLE_RADIUS + 1.0)
        } else {
            (dot, HANDLE_RADIUS)
        };
        gizmos.circle_2d(left_pos, lr, lc);

        let (rc, rr) = if near_right || (is_source && drag.from_right) {
            (dot_hi, HANDLE_RADIUS + 1.0)
        } else {
            (dot, HANDLE_RADIUS)
        };
        gizmos.circle_2d(right_pos, rr, rc);
    }
}

/// Sets the OS cursor to reflect what the hovered block region does: a connect
/// (crosshair) cursor over the dependency dots, a resize cursor near the right
/// edge, and a move cursor over the block body. Leaves egui to manage the cursor
/// while the pointer is over a UI area.
pub fn update_cursor_icon(
    mut commands: Commands,
    mut egui_ctx: EguiContexts,
    windows: Query<(Entity, &Window)>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    block_query: Query<(&Transform, &Sprite), With<BlockSprite>>,
) {
    let Ok((win_entity, window)) = windows.single() else {
        return;
    };
    if egui_ctx
        .ctx_mut()
        .map(|c| c.is_pointer_over_area())
        .unwrap_or(false)
    {
        return; // egui manages its own cursor over UI
    }
    let Ok((cam, cam_tr)) = camera.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_tr, cursor) else {
        return;
    };

    let mut icon = SystemCursorIcon::Default;
    for (transform, sprite) in &block_query {
        let Some(size) = sprite.custom_size else {
            continue;
        };
        let center = transform.translation.truncate();
        let half = size * 0.5;

        // Priority matches the interaction order: dep handle > resize edge > move.
        let near_handle = (world_pos - Vec2::new(center.x - half.x, center.y)).length()
            < HANDLE_HIT_PX
            || (world_pos - Vec2::new(center.x + half.x, center.y)).length() < HANDLE_HIT_PX;
        if near_handle {
            icon = SystemCursorIcon::Crosshair;
            break;
        }
        let inside = world_pos.x >= center.x - half.x
            && world_pos.x <= center.x + half.x
            && world_pos.y >= center.y - half.y
            && world_pos.y <= center.y + half.y;
        if inside {
            icon = if world_pos.x >= center.x + half.x - EDGE_GRAB_PX {
                SystemCursorIcon::EwResize
            } else {
                SystemCursorIcon::Move
            };
            break;
        }
    }

    commands.entity(win_entity).insert(CursorIcon::System(icon));
}

#[allow(clippy::too_many_arguments)]
pub fn handle_dep_drag(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
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

    // Returns the block under `pos` and whether `pos` is in its right (finish) half.
    let block_at = |pos: Vec2| -> Option<(WorkBlockId, bool)> {
        for (bs, tr, sp) in &block_query {
            let Some(size) = sp.custom_size else { continue };
            let center = tr.translation.truncate();
            let half = size * 0.5;
            if pos.x >= center.x - half.x
                && pos.x <= center.x + half.x
                && pos.y >= center.y - half.y
                && pos.y <= center.y + half.y
            {
                return Some((bs.work_block_id, pos.x >= center.x));
            }
        }
        None
    };

    // Left-click on an edge handle starts a dep drag (right = outgoing, so source
    // is the predecessor; left = incoming, so source is the successor).
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
    }

    // Left-click release: finish a handle-initiated dep drag. The dependency type
    // is implied by the source edge (which handle) and the target edge (drop half);
    // the drag source is always the predecessor.
    if mouse.just_released(MouseButton::Left) {
        if let Some(from_id) = drag.from.take() {
            if let Some((to_id, to_finish)) = block_at(world_pos) {
                if to_id != from_id {
                    let dep_type = dep_type_from_edges(drag.from_right, to_finish);
                    // Create only (idempotent); deletion is click-the-edge + Delete.
                    let exists = model.dependencies.values().any(|d| {
                        d.predecessor == from_id
                            && d.successor == to_id
                            && d.dependency_type == dep_type
                    });
                    if !exists {
                        model.create_dependency(from_id, to_id, dep_type);
                        if let Err(e) = crate::db::save_model(&conn, &model) {
                            error!("save_model failed: {e}");
                        }
                    }
                }
            }
        }
    }

    // Right-click drag: shortcut that drags from the source's finish edge.
    if mouse.just_pressed(MouseButton::Right) {
        drag.from = block_at(world_pos).map(|(id, _)| id);
        drag.from_right = true;
    }

    if mouse.just_released(MouseButton::Right) {
        if let Some(from_id) = drag.from.take() {
            if let Some((to_id, to_finish)) = block_at(world_pos) {
                if to_id != from_id {
                    let dep_type = dep_type_from_edges(drag.from_right, to_finish);
                    let exists = model.dependencies.values().any(|d| {
                        d.predecessor == from_id
                            && d.successor == to_id
                            && d.dependency_type == dep_type
                    });
                    if !exists {
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
                            scope.scope_stack.push(schedule::ScopeEntry::Block(id));
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

/// Snapshot of everything removed by a single block deletion, enabling undo.
struct DeletedBlockSnapshot {
    blocks: Vec<model::WorkBlock>,
    variants: Vec<model::Variant>,
    dependencies: Vec<model::Dependency>,
    /// (plan_id, root_block_ids that were in this plan)
    plan_roots: Vec<(model::PlanId, Vec<WorkBlockId>)>,
    /// (plan_id, selected_variants entries for deleted blocks)
    plan_sel_vars: Vec<(model::PlanId, Vec<(WorkBlockId, model::VariantId)>)>,
    /// (plan_id, allocations for deleted blocks)
    plan_allocs: Vec<(model::PlanId, Vec<model::ResourceAllocation>)>,
    /// (variant_id, child_ids) for surviving variants whose children lists
    /// contained deleted blocks — needed to restore hierarchy on undo.
    variant_child_refs: Vec<(model::VariantId, Vec<WorkBlockId>)>,
}

/// Single-slot undo buffer for block deletions. Holds the most recent deletion;
/// overwritten on each delete; consumed by undo.
#[derive(Resource, Default)]
pub struct UndoStack {
    last_deletion: Option<DeletedBlockSnapshot>,
}

fn build_deletion_snapshot(model: &model::Model, id: WorkBlockId) -> DeletedBlockSnapshot {
    // Mirror the BFS in delete_work_block to find all blocks that will be removed.
    let mut to_delete: Vec<WorkBlockId> = vec![id];
    let mut visited: HashSet<WorkBlockId> = HashSet::from([id]);
    let mut i = 0;
    while i < to_delete.len() {
        let cur = to_delete[i];
        if let Some(wb) = model.work_blocks.get(&cur) {
            for &var_id in &wb.variants {
                if let Some(var) = model.variants.get(&var_id) {
                    for &child_id in &var.children {
                        if visited.insert(child_id) {
                            to_delete.push(child_id);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    let delete_set: HashSet<WorkBlockId> = to_delete.iter().copied().collect();

    let blocks = to_delete
        .iter()
        .filter_map(|&bid| model.work_blocks.get(&bid).cloned())
        .collect();
    let variants = model
        .variants
        .values()
        .filter(|v| delete_set.contains(&v.parent))
        .cloned()
        .collect();
    let dependencies = model
        .dependencies
        .values()
        .filter(|d| delete_set.contains(&d.predecessor) || delete_set.contains(&d.successor))
        .cloned()
        .collect();

    let mut plan_roots = Vec::new();
    let mut plan_sel_vars = Vec::new();
    let mut plan_allocs = Vec::new();
    for (&plan_id, plan) in &model.plans {
        let roots: Vec<WorkBlockId> =
            plan.root_blocks.iter().filter(|&&b| delete_set.contains(&b)).copied().collect();
        if !roots.is_empty() {
            plan_roots.push((plan_id, roots));
        }
        let sel: Vec<(WorkBlockId, model::VariantId)> = plan
            .selected_variants
            .iter()
            .filter(|(b, _)| delete_set.contains(b))
            .map(|(&b, &v)| (b, v))
            .collect();
        if !sel.is_empty() {
            plan_sel_vars.push((plan_id, sel));
        }
        let allocs: Vec<model::ResourceAllocation> = plan
            .allocations
            .iter()
            .filter(|a| delete_set.contains(&a.work_block_id))
            .cloned()
            .collect();
        if !allocs.is_empty() {
            plan_allocs.push((plan_id, allocs));
        }
    }

    // Capture references from surviving variants (not owned by deleted blocks)
    // whose children lists include deleted blocks — delete_work_block strips
    // these but restore needs to re-add them.
    let variant_child_refs = model
        .variants
        .iter()
        .filter(|(_, v)| !delete_set.contains(&v.parent))
        .filter_map(|(&vid, v)| {
            let refs: Vec<WorkBlockId> = v
                .children
                .iter()
                .filter(|&&b| delete_set.contains(&b))
                .copied()
                .collect();
            if refs.is_empty() { None } else { Some((vid, refs)) }
        })
        .collect();

    DeletedBlockSnapshot {
        blocks,
        variants,
        dependencies,
        plan_roots,
        plan_sel_vars,
        plan_allocs,
        variant_child_refs,
    }
}

fn restore_deletion_snapshot(model: &mut model::Model, snap: DeletedBlockSnapshot) {
    for wb in snap.blocks {
        model.work_blocks.insert(wb.id, wb);
    }
    for var in snap.variants {
        model.variants.insert(var.id, var);
    }
    for dep in snap.dependencies {
        model.dependencies.insert(dep.id, dep);
    }
    for (plan_id, roots) in snap.plan_roots {
        if let Some(plan) = model.plans.get_mut(&plan_id) {
            for bid in roots {
                if !plan.root_blocks.contains(&bid) {
                    plan.root_blocks.push(bid);
                }
            }
        }
    }
    for (plan_id, sel_vars) in snap.plan_sel_vars {
        if let Some(plan) = model.plans.get_mut(&plan_id) {
            for (bid, vid) in sel_vars {
                plan.selected_variants.insert(bid, vid);
            }
        }
    }
    for (plan_id, allocs) in snap.plan_allocs {
        if let Some(plan) = model.plans.get_mut(&plan_id) {
            for alloc in allocs {
                let already = plan
                    .allocations
                    .iter()
                    .any(|a| a.work_block_id == alloc.work_block_id && a.resource_id == alloc.resource_id);
                if !already {
                    plan.allocations.push(alloc);
                }
            }
        }
    }
    for (vid, children) in snap.variant_child_refs {
        if let Some(var) = model.variants.get_mut(&vid) {
            for bid in children {
                if !var.children.contains(&bid) {
                    var.children.push(bid);
                }
            }
        }
    }
}

/// Detects Delete/Backspace and immediately removes the selected block from the
/// model. Runs in Update BEFORE `update_visible_blocks` so sprite reconciliation
/// fires in the same frame — this avoids the timing bug where a deletion in
/// `EguiPrimaryContextPass` would be invisible to `is_changed()` the next frame.
pub fn handle_block_delete(
    mut egui_ctx: EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut selected: ResMut<SelectedBlock>,
    mut selected_dep: ResMut<SelectedDependency>,
    name_edit: Res<NameEditState>,
    mut model: ResMut<model::Model>,
    mut undo: ResMut<UndoStack>,
    conn: NonSend<rusqlite::Connection>,
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
        // A selected dependency edge deletes first; otherwise delete the block.
        if let Some(dep_id) = selected_dep.0.take() {
            model.dependencies.remove(&dep_id);
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
        } else if let Some(id) = selected.0 {
            undo.last_deletion = Some(build_deletion_snapshot(&model, id));
            delete_work_block(&mut model, id);
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
            selected.0 = None;
        }
    }
}

/// Restores the most recent block deletion on Ctrl+Z / Cmd+Z.
pub fn handle_undo(
    mut egui_ctx: EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    name_edit: Res<NameEditState>,
    mut model: ResMut<model::Model>,
    mut undo: ResMut<UndoStack>,
    conn: NonSend<rusqlite::Connection>,
) {
    if name_edit.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            return;
        }
    }
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
        || keyboard.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight]);
    if ctrl && keyboard.just_pressed(KeyCode::KeyZ) {
        if let Some(snap) = undo.last_deletion.take() {
            restore_deletion_snapshot(&mut model, snap);
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
        }
    }
}

/// Remove a work block and all of its descendants (variants' children,
/// recursively) from the model, cleaning up all cross-references.
///
/// Deleted:
/// - The work block itself and every descendant `WorkBlock`
/// - All `Dependency` edges that touch any deleted block
/// - Entries in `plan.root_blocks`, `plan.selected_variants`, and
///   `plan.allocations` for every deleted block
/// - All `Variant` records whose parent is any deleted block
/// - References to deleted blocks in the `children` lists of surviving variants
pub fn delete_work_block(model: &mut model::Model, id: WorkBlockId) {
    // BFS to collect the block and all of its variant descendants.
    let mut to_delete: Vec<WorkBlockId> = vec![id];
    let mut visited: HashSet<WorkBlockId> = HashSet::from([id]);
    let mut i = 0;
    while i < to_delete.len() {
        let cur = to_delete[i];
        if let Some(wb) = model.work_blocks.get(&cur) {
            for &var_id in &wb.variants.clone() {
                if let Some(var) = model.variants.get(&var_id) {
                    for &child_id in &var.children.clone() {
                        if visited.insert(child_id) {
                            to_delete.push(child_id);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    let delete_set: HashSet<WorkBlockId> = to_delete.iter().copied().collect();

    for &del_id in &to_delete {
        model.work_blocks.remove(&del_id);
    }
    model.dependencies.retain(|_, dep| {
        !delete_set.contains(&dep.predecessor) && !delete_set.contains(&dep.successor)
    });
    for plan in model.plans.values_mut() {
        plan.root_blocks.retain(|bid| !delete_set.contains(bid));
        for &del_id in &to_delete {
            plan.selected_variants.remove(&del_id);
        }
        plan.allocations.retain(|a| !delete_set.contains(&a.work_block_id));
    }
    // Remove deleted IDs from surviving variants' children lists before
    // removing the variants themselves, so the retain below is consistent.
    for variant in model.variants.values_mut() {
        variant.children.retain(|bid| !delete_set.contains(bid));
    }
    model.variants.retain(|_, v| !delete_set.contains(&v.parent));
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Estimate, Model};

    fn est() -> Estimate {
        Estimate { most_likely: 5, optimistic: 3, pessimistic: 8, confidence: 0.8 }
    }

    #[test]
    fn delete_simple_block_removes_it() {
        let mut m = Model::default();
        let a = m.create_work_block("A", est());
        delete_work_block(&mut m, a);
        assert!(!m.work_blocks.contains_key(&a));
    }

    #[test]
    fn delete_block_with_variants_removes_children() {
        let mut m = Model::default();
        let parent = m.create_work_block("P", est());
        let var = m.create_variant("V", parent);
        let child = m.create_work_block("C", est());
        m.work_blocks.get_mut(&parent).unwrap().variants.push(var);
        m.variants.get_mut(&var).unwrap().children.push(child);

        delete_work_block(&mut m, parent);

        assert!(!m.work_blocks.contains_key(&parent), "parent removed");
        assert!(!m.work_blocks.contains_key(&child), "child removed");
        assert!(!m.variants.contains_key(&var), "variant removed");
    }

    #[test]
    fn delete_block_cleans_plan_root_and_allocations() {
        let mut m = Model::default();
        let wid = m.create_world("w");
        let pid = m.create_plan("p", wid, None);
        let a = m.create_work_block("A", est());
        m.plans.get_mut(&pid).unwrap().root_blocks.push(a);

        delete_work_block(&mut m, a);

        assert!(!m.plans[&pid].root_blocks.contains(&a));
    }

    #[test]
    fn delete_block_removes_its_dependencies() {
        use crate::model::DependencyType;
        let mut m = Model::default();
        let a = m.create_work_block("A", est());
        let b = m.create_work_block("B", est());
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);

        delete_work_block(&mut m, a);

        assert!(!m.dependencies.contains_key(&dep));
        assert!(m.work_blocks.contains_key(&b), "B survives");
    }

    #[test]
    fn delete_recursive_two_levels() {
        let mut m = Model::default();
        let parent = m.create_work_block("P", est());
        let var = m.create_variant("V", parent);
        let child = m.create_work_block("C", est());
        let var2 = m.create_variant("V2", child);
        let grandchild = m.create_work_block("GC", est());
        m.work_blocks.get_mut(&parent).unwrap().variants.push(var);
        m.variants.get_mut(&var).unwrap().children.push(child);
        m.work_blocks.get_mut(&child).unwrap().variants.push(var2);
        m.variants.get_mut(&var2).unwrap().children.push(grandchild);

        delete_work_block(&mut m, parent);

        assert!(!m.work_blocks.contains_key(&parent));
        assert!(!m.work_blocks.contains_key(&child));
        assert!(!m.work_blocks.contains_key(&grandchild));
        assert!(m.variants.is_empty());
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
    active_schedule: Res<schedule::Schedule>,
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
                most_likely: 1,
                optimistic: 1,
                pessimistic: 2,
                confidence: 0.8,
            };
            let new_id = model.create_work_block(name, est);
            let plan_id = active_schedule.plan_id;
            // Place the new block at branch_min so it appears immediately on the timeline.
            // For baseline plans (branch_start_day=None) branch_min is 0.0 — day 0 is the
            // correct default since the user can drag to reposition. Leaving duration_days=0
            // would make the block invisible, which is worse UX.
            // For branch plans branch_min is the branch start day, keeping new work inside
            // the branch window.
            let branch_min = model
                .plans
                .get(&plan_id)
                .and_then(|p| p.branch_start_day)
                .unwrap_or(0);
            // No cursor in bulk-create mode: stack each new block one lane down
            // (by current block count) so they don't pile onto the same spot.
            let new_row = model
                .plans
                .get(&plan_id)
                .map(|p| p.root_blocks.len() as i32)
                .unwrap_or(0);
            if let Some(wb) = model.work_blocks.get_mut(&new_id) {
                wb.start_day = branch_min;
                wb.duration_days = 5;
                wb.row = new_row;
            }
            if let Some(plan) = model.plans.get_mut(&plan_id) {
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

// ── T-shirt size picker ──────────────────────────────────────────────────────

/// State for the size picker: which block it targets (if open) and whether the
/// editable size-map settings window is showing.
#[derive(Resource, Default)]
pub struct SizePickerState {
    pub target: Option<WorkBlockId>,
    pub settings_open: bool,
}

/// Opens the size picker for the selected block on `s` (and closes it / the
/// settings on `Esc`). Guarded so it never fires while typing in an egui field.
pub fn handle_size_picker_hotkey(
    mut egui_ctx: EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Res<SelectedBlock>,
    name_edit: Res<NameEditState>,
    mut picker: ResMut<SizePickerState>,
) {
    if name_edit.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            return;
        }
    }
    if keyboard.just_pressed(KeyCode::Escape) {
        picker.target = None;
        picker.settings_open = false;
        return;
    }
    if keyboard.just_pressed(KeyCode::KeyS) {
        if let Some(id) = selected.0 {
            // Toggle on the selected block.
            picker.target = if picker.target == Some(id) { None } else { Some(id) };
            picker.settings_open = false;
        }
    }
}

/// Renders the size picker anchored next to the target block. Clicking a size
/// sets the block's duration and records the chosen size label.
pub fn draw_size_picker_popup(
    mut contexts: EguiContexts,
    mut picker: ResMut<SizePickerState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    block_query: Query<(&BlockSprite, &Transform)>,
) {
    // The settings window takes over while it's open.
    if picker.settings_open {
        return;
    }
    let Some(target) = picker.target else { return };
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // Anchor to the block's screen position (fallback: upper-left).
    let mut screen_pos = egui::pos2(80.0, 120.0);
    if let Ok((cam, cam_tf)) = camera.single() {
        for (bs, transform) in &block_query {
            if bs.work_block_id == target {
                if let Ok(vp) = cam.world_to_viewport(cam_tf, transform.translation) {
                    screen_pos = egui::pos2(vp.x + 14.0, vp.y - 8.0);
                }
                break;
            }
        }
    }

    let current = model
        .work_blocks
        .get(&target)
        .and_then(|wb| wb.t_shirt_size.clone());
    let sizes = model.t_shirt_sizes.clone();

    let mut chosen: Option<(String, Day)> = None;
    let mut open_settings = false;
    let mut close = false;

    egui::Area::new(egui::Id::new("size_picker_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen_pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(116.0);
                ui.label(egui::RichText::new("Size").strong());
                for size in &sizes {
                    let is_current = current.as_deref() == Some(size.label.as_str());
                    let text = format!("{}   {} d", size.label, size.days);
                    if ui.selectable_label(is_current, text).clicked() {
                        chosen = Some((size.label.clone(), size.days));
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.small_button("⚙ Edit…").clicked() {
                        open_settings = true;
                    }
                    if ui.small_button("Close").clicked() {
                        close = true;
                    }
                });
            });
        });

    if let Some((label, days)) = chosen {
        if let Some(wb) = model.work_blocks.get_mut(&target) {
            wb.duration_days = days;
            wb.t_shirt_size = Some(label);
        }
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
        picker.target = None;
    } else if open_settings {
        picker.settings_open = true;
    } else if close {
        picker.target = None;
    }
}

/// Editable size-map window: rename, set days, add and remove sizes. Persists on
/// every change so the picker reflects edits immediately.
pub fn draw_size_settings_popup(
    mut contexts: EguiContexts,
    mut picker: ResMut<SizePickerState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
) {
    if !picker.settings_open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut changed = false;
    let mut remove: Option<usize> = None;
    let mut add = false;
    let mut done = false;

    egui::Window::new("Edit sizes")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            for (i, size) in model.t_shirt_sizes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::TextEdit::singleline(&mut size.label).desired_width(52.0))
                        .changed()
                    {
                        changed = true;
                    }
                    if ui
                        .add(egui::DragValue::new(&mut size.days).range(1..=400).suffix(" d"))
                        .changed()
                    {
                        changed = true;
                    }
                    if ui.small_button("×").on_hover_text("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("＋ Add size").clicked() {
                    add = true;
                }
                if ui.button("Done").clicked() {
                    done = true;
                }
            });
        });

    if let Some(i) = remove {
        if i < model.t_shirt_sizes.len() {
            model.t_shirt_sizes.remove(i);
            changed = true;
        }
    }
    if add {
        model.t_shirt_sizes.push(model::TShirtSize {
            label: "New".to_string(),
            days: 5,
        });
        changed = true;
    }
    if changed {
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
    if done {
        picker.settings_open = false;
    }
}
