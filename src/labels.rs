use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    analysis::ScheduleAnalysis,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    model::{Model, WorkBlockId},
    schedule::{self, Schedule},
};

/// Y position of day-number labels above the block rows.
const DAY_LABEL_Y: f32 = 55.0;
/// X position of the right edge of row name labels.
const ROW_LABEL_X: f32 = -80.0;
/// Draw a day label every this many days.
const DAY_STEP: i32 = 5;
/// Visual indentation per nesting level for row labels.
const INDENT_PX: f32 = 12.0;

/// Marker for day-number `Text2d` entities.
#[derive(Component)]
pub struct DayLabel;

/// Marker for row-name `Text2d` entities.
#[derive(Component)]
pub struct RowLabel {
    pub work_block_id: WorkBlockId,
    pub row: usize,
}

/// Spawns (or re-spawns) all timeline labels:
/// - Day numbers along the top at every `DAY_STEP` days.
/// - Work-block names to the left of each row, indented by nesting depth.
pub fn spawn_labels(
    mut commands: Commands,
    schedule: Res<Schedule>,
    model: Res<Model>,
    day_q: Query<Entity, With<DayLabel>>,
    row_q: Query<Entity, With<RowLabel>>,
) {
    if !model.is_changed() {
        return;
    }
    for e in &day_q {
        commands.entity(e).despawn();
    }
    for e in &row_q {
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

    // Row name labels — same sort order as block sprites for matching rows.
    let ordered = schedule::sorted_blocks(&model);

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
