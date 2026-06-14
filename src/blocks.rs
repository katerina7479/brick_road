use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::{
    analysis::ScheduleAnalysis,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    model::{self, WorkBlockId},
    schedule,
};

const BLOCK_HEIGHT: f32 = 28.0;

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
    existing: Query<Entity, With<BlockSprite>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let ordered = schedule::sorted_blocks(&model);

    let on_critical_path: std::collections::HashSet<WorkBlockId> =
        sa.critical_path.iter().copied().collect();

    for (row, wb) in ordered.iter().enumerate() {
        let width = wb.duration_days * PIXELS_PER_DAY;
        // Sprite origin is at its center in Bevy 2D.
        let x = wb.start_day * PIXELS_PER_DAY + width * 0.5;
        let y = -(row as f32) * ROW_HEIGHT;

        // Critical-path blocks glow gold; others cycle through the palette.
        let color = if on_critical_path.contains(&wb.id) {
            Color::from(LinearRgba::new(3.0, 2.2, 0.1, 1.0))
        } else {
            Color::from(PALETTE[row % PALETTE.len()])
        };

        commands.spawn((
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
    mut query: Query<(&BlockSprite, &mut Transform, &mut Sprite)>,
) {
    let on_critical: std::collections::HashSet<WorkBlockId> =
        sa.critical_path.iter().copied().collect();

    for (block_sprite, mut transform, mut sprite) in &mut query {
        let Some(wb) = model.work_blocks.get(&block_sprite.work_block_id) else {
            continue;
        };
        let width = wb.duration_days * PIXELS_PER_DAY;
        let x = wb.start_day * PIXELS_PER_DAY + width * 0.5;
        let y = -(block_sprite.row as f32) * ROW_HEIGHT;
        transform.translation.x = x;
        transform.translation.y = y;
        sprite.custom_size = Some(Vec2::new(width, BLOCK_HEIGHT));

        let base = PALETTE[block_sprite.row % PALETTE.len()];
        let id = block_sprite.work_block_id;
        sprite.color = if on_critical.contains(&id) {
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

/// Converts a left-click to a block selection.
///
/// Clicks that land inside egui areas (e.g. the side panel) are ignored.
/// Clicking the currently selected block deselects it; clicking empty space
/// clears the selection.
pub fn handle_block_selection(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut selected: ResMut<SelectedBlock>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
) {
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

    // Re-clicking the selected block toggles it off; otherwise set/clear.
    selected.0 = if clicked.is_some() && clicked == selected.0 {
        None
    } else {
        clicked
    };
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
    existing: Query<Entity, With<ConflictOverlay>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    if sa.resource_conflicts.is_empty() {
        return;
    }

    // Row lookup: WorkBlockId → row index (same ordering as block sprites).
    let ordered = schedule::sorted_blocks(&model);
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
