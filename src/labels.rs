use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    analysis::ScheduleAnalysis,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    model::{Model, WorkBlockId},
    schedule::{self, Schedule, ViewScope},
};

/// Y position of day-number labels above the block rows.
const DAY_LABEL_Y: f32 = 55.0;
/// X position of the right edge of row name labels.
const ROW_LABEL_X: f32 = -80.0;
/// Visual indentation per nesting level for row labels.
const INDENT_PX: f32 = 12.0;

/// Maps orthographic zoom scale to the day-label stride.
/// Returns the number of days between consecutive day labels.
fn day_step_for_zoom(scale: f32) -> i32 {
    if scale < 0.5 {
        1
    } else if scale < 2.0 {
        5
    } else if scale < 4.0 {
        10
    } else {
        30
    }
}

/// Marker for day-number `Text2d` entities.
#[derive(Component)]
pub struct DayLabel;

/// Marker for row-name `Text2d` entities.
#[derive(Component)]
pub struct RowLabel {
    pub work_block_id: WorkBlockId,
    pub row: usize,
}

/// Spawns (or re-spawns) row-name labels to the left of each block row.
/// Day-number labels are managed separately by `spawn_day_labels`.
pub fn spawn_labels(
    mut commands: Commands,
    model: Res<Model>,
    scope: Res<ViewScope>,
    row_q: Query<Entity, With<RowLabel>>,
) {
    if !model.is_changed() && !scope.is_changed() {
        return;
    }
    for e in &row_q {
        commands.entity(e).despawn();
    }

    // Row name labels — same sort order as block sprites for matching rows.
    let ordered = schedule::visible_blocks(&model, &scope);

    for (row, wb) in ordered.iter().enumerate() {
        let name = wb.name.clone();

        let depth = nesting_depth(&model, wb.id);
        let x = ROW_LABEL_X + depth as f32 * INDENT_PX;
        let y = -(row as f32) * ROW_HEIGHT;

        commands.spawn((
            RowLabel {
                work_block_id: wb.id,
                row,
            },
            Text2d::new(name),
            TextFont {
                font_size: 11.0,
                ..default()
            },
            TextColor(Color::srgba(0.85, 0.85, 0.95, 0.9)),
            Transform::from_xyz(x, y, 1.0),
        ));
    }
}

/// Spawns (or re-spawns) day-number labels along the top of the timeline.
///
/// Respawns when:
/// - The zoom band changes (scale crosses one of the 0.5 / 2.0 / 4.0 thresholds).
/// - `model` or `schedule` changes (timeline span may have shifted).
///
/// Uses a `Local<i32>` to track the previously-active stride so that smooth
/// zooming within a band incurs no per-frame entity churn.
pub fn spawn_day_labels(
    mut commands: Commands,
    schedule: Res<Schedule>,
    model: Res<Model>,
    cam_q: Query<&Projection, With<Camera2d>>,
    day_q: Query<Entity, With<DayLabel>>,
    mut prev_step: Local<i32>,
) {
    let scale = cam_q
        .single()
        .ok()
        .and_then(|proj| {
            if let Projection::Orthographic(o) = proj {
                Some(o.scale)
            } else {
                None
            }
        })
        .unwrap_or(1.0);

    let step = day_step_for_zoom(scale);
    let zoom_band_changed = step != *prev_step;

    if !zoom_band_changed && !schedule.is_changed() && !model.is_changed() {
        return;
    }
    *prev_step = step;

    for e in &day_q {
        commands.entity(e).despawn();
    }

    let span = schedule.total_duration_days.ceil() as i32 + step;
    for day in (0..=span).step_by(step as usize) {
        let x = day as f32 * PIXELS_PER_DAY;
        commands.spawn((
            DayLabel,
            Text2d::new(format!("D{day}")),
            TextFont {
                font_size: 11.0,
                ..default()
            },
            TextColor(Color::srgba(0.6, 0.6, 0.9, 0.75)),
            Transform::from_xyz(x, DAY_LABEL_Y, 1.0),
        ));
    }
}

/// Draws vertical bracket gizmos for each `Variant`'s children, showing
/// parent/child nesting relationships in the block layout.
pub fn draw_nesting_indicators(
    schedule: Res<Schedule>,
    model: Res<Model>,
    mut gizmos: Gizmos,
    row_q: Query<(&RowLabel, &Transform)>,
) {
    let bracket_color = Color::srgba(0.5, 0.5, 0.75, 0.45);

    // Build a lookup from WorkBlockId → row Y from the live RowLabel positions.
    let row_y: HashMap<WorkBlockId, f32> = row_q
        .iter()
        .map(|(rl, t)| (rl.work_block_id, t.translation.y))
        .collect();

    for variant in model.variants.values() {
        if variant.children.is_empty() {
            continue;
        }

        let ys: Vec<f32> = variant
            .children
            .iter()
            .filter_map(|id| row_y.get(id).copied())
            .collect();
        if ys.is_empty() {
            continue;
        }

        let top_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max) + ROW_HEIGHT * 0.4;
        let bot_y = ys.iter().cloned().fold(f32::INFINITY, f32::min) - ROW_HEIGHT * 0.4;

        // Place the bracket just to the left of the earliest child block.
        let left_x = variant
            .children
            .iter()
            .filter_map(|id| schedule.blocks.get(id))
            .map(|b| b.start_day * PIXELS_PER_DAY)
            .fold(f32::INFINITY, f32::min);

        if !left_x.is_finite() {
            continue;
        }

        let bx = left_x - 8.0;
        // Vertical bar.
        gizmos.line_2d(Vec2::new(bx, bot_y), Vec2::new(bx, top_y), bracket_color);
        // Horizontal serifs.
        gizmos.line_2d(
            Vec2::new(bx, top_y),
            Vec2::new(bx + 4.0, top_y),
            bracket_color,
        );
        gizmos.line_2d(
            Vec2::new(bx, bot_y),
            Vec2::new(bx + 4.0, bot_y),
            bracket_color,
        );
    }
}

/// Keeps all `DayLabel` and `RowLabel` `Text2d` entities at a constant
/// screen-space size by counter-scaling their `Transform` by the current
/// orthographic zoom each frame.
#[allow(clippy::type_complexity)]
pub fn scale_labels_to_zoom(
    cam_q: Query<&Projection, With<Camera2d>>,
    mut label_q: Query<&mut Transform, Or<(With<DayLabel>, With<RowLabel>)>>,
) {
    let Ok(proj) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let s = ortho.scale;
    for mut transform in &mut label_q {
        transform.scale = Vec3::splat(s);
    }
}

/// Returns how many variant layers deep this block is nested (0 = root-level).
fn nesting_depth(model: &Model, id: WorkBlockId) -> usize {
    for variant in model.variants.values() {
        if variant.children.contains(&id) {
            return 1 + nesting_depth(model, variant.parent);
        }
    }
    0
}

/// Draws a red connecting line between each pair of blocks that violates a
/// dependency constraint. The line runs from the predecessor's right edge to
/// the successor's left edge, using the row Y positions from live `RowLabel`
/// entities. Blocks with no placed row are skipped.
pub fn draw_violation_indicators(
    model: Res<Model>,
    analysis: Res<ScheduleAnalysis>,
    mut gizmos: Gizmos,
    row_q: Query<(&RowLabel, &Transform)>,
) {
    if analysis.violations.is_empty() {
        return;
    }

    let violation_color = Color::from(LinearRgba::new(3.0, 0.1, 0.1, 1.0));

    let row_y: HashMap<WorkBlockId, f32> = row_q
        .iter()
        .map(|(rl, t)| (rl.work_block_id, t.translation.y))
        .collect();

    for v in &analysis.violations {
        let Some(pred) = model.work_blocks.get(&v.predecessor) else {
            continue;
        };
        let Some(succ) = model.work_blocks.get(&v.successor) else {
            continue;
        };
        let Some(&pred_y) = row_y.get(&v.predecessor) else {
            continue;
        };
        let Some(&succ_y) = row_y.get(&v.successor) else {
            continue;
        };

        let pred_x = (pred.start_day + pred.duration_days) * PIXELS_PER_DAY;
        let succ_x = succ.start_day * PIXELS_PER_DAY;

        gizmos.line_2d(
            Vec2::new(pred_x, pred_y),
            Vec2::new(succ_x, succ_y),
            violation_color,
        );
    }
}
