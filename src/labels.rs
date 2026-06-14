use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    analysis::ScheduleAnalysis,
    blocks::BlockSprite,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    model::{Model, WorkBlockId},
    schedule::{Schedule, ViewScope},
};

/// Y position of day-number labels above the block rows.
const DAY_LABEL_Y: f32 = 55.0;
/// Draw a day label every this many days.
const DAY_STEP: i32 = 5;

/// Marker for day-number `Text2d` entities.
#[derive(Component)]
pub struct DayLabel;

/// Spawns (or re-spawns) day-number labels along the top of the timeline.
/// Row name labels are now rendered inline inside block bars (see `blocks::BlockLabel`).
pub fn spawn_labels(
    mut commands: Commands,
    schedule: Res<Schedule>,
    model: Res<Model>,
    scope: Res<ViewScope>,
    day_q: Query<Entity, With<DayLabel>>,
) {
    if !model.is_changed() && !scope.is_changed() {
        return;
    }
    for e in &day_q {
        commands.entity(e).despawn();
    }

    // Day number labels along the top of the grid.
    let span = schedule.total_duration_days.ceil() as i32 + DAY_STEP;
    for day in (0..=span).step_by(DAY_STEP as usize) {
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
    block_q: Query<(&BlockSprite, &Transform)>,
) {
    let bracket_color = Color::srgba(0.5, 0.5, 0.75, 0.45);

    // Build a lookup from WorkBlockId → row Y from live BlockSprite positions.
    let row_y: HashMap<WorkBlockId, f32> = block_q
        .iter()
        .map(|(bs, t)| (bs.work_block_id, t.translation.y))
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

pub fn scale_labels_to_zoom(
    cam_q: Query<&Projection, With<Camera2d>>,
    mut label_q: Query<&mut Transform, With<DayLabel>>,
) {
    let Ok(proj) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let s = ortho.scale;
    for mut transform in &mut label_q {
        transform.scale = Vec3::splat(s);
    }
}


/// Draws a red connecting line between each pair of blocks that violates a
/// dependency constraint. The line runs from the predecessor's right edge to
/// the successor's left edge, using live BlockSprite Y positions.
pub fn draw_violation_indicators(
    model: Res<Model>,
    analysis: Res<ScheduleAnalysis>,
    mut gizmos: Gizmos,
    block_q: Query<(&BlockSprite, &Transform)>,
) {
    if analysis.violations.is_empty() {
        return;
    }

    let violation_color = Color::from(LinearRgba::new(3.0, 0.1, 0.1, 1.0));

    let row_y: HashMap<WorkBlockId, f32> = block_q
        .iter()
        .map(|(bs, t)| (bs.work_block_id, t.translation.y))
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
