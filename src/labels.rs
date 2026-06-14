use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    analysis::ScheduleAnalysis,
    blocks::BlockSprite,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    model::{Model, WorkBlockId},
    schedule::Schedule,
};

/// Y position of day-number labels above the block rows.
const DAY_LABEL_Y: f32 = 55.0;

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

/// Stub — row labels removed by br-57 (names inside blocks), day labels
/// handled by `spawn_day_labels`. Kept as a no-op because main.rs
/// registrations reference it; safe to remove in a cleanup pass.
pub fn spawn_labels() {}

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
