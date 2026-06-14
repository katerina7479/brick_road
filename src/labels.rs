use std::collections::{HashMap, HashSet, VecDeque};

use bevy::prelude::*;
use chrono::Datelike;

use crate::{
    analysis::ScheduleAnalysis,
    blocks::BlockSprite,
    calendar::day_to_date,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    model::{Model, WorkBlockId},
    schedule::Schedule,
};

/// Precomputed nesting depth for every `WorkBlock`.
///
/// Depth 0 = top-level block (not a child of any variant).
/// Depth N = N hops through variant parent relationships from a root.
/// Recomputed once per model change so per-frame consumers pay O(1) per block.
#[derive(Debug, Default, Resource)]
pub struct NestingDepthMap {
    pub depths: HashMap<WorkBlockId, usize>,
}

/// Pure depth-map computation, extracted for testability.
///
/// BFS from root blocks (those not referenced in any variant's children) at
/// depth 0, expanding each block's own variant children at depth + 1.
/// If a block is reachable via multiple paths (diamond), whichever BFS front
/// reaches it first wins — depth is assigned once and never overwritten.
pub(crate) fn build_depth_map(model: &Model) -> HashMap<WorkBlockId, usize> {
    // Collect every block that appears as someone's variant child.
    let mut is_child: HashSet<WorkBlockId> = HashSet::new();
    for variant in model.variants.values() {
        for &child_id in &variant.children {
            is_child.insert(child_id);
        }
    }

    let mut depths: HashMap<WorkBlockId, usize> = HashMap::new();
    let mut queue: VecDeque<(WorkBlockId, usize)> = VecDeque::new();

    // Seed with roots (not a child of any variant) at depth 0.
    for &id in model.work_blocks.keys() {
        if !is_child.contains(&id) {
            depths.insert(id, 0);
            queue.push_back((id, 0));
        }
    }

    while let Some((id, depth)) = queue.pop_front() {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        for &variant_id in &wb.variants {
            let Some(variant) = model.variants.get(&variant_id) else {
                continue;
            };
            for &child_id in &variant.children {
                if let std::collections::hash_map::Entry::Vacant(e) = depths.entry(child_id) {
                    e.insert(depth + 1);
                    queue.push_back((child_id, depth + 1));
                }
            }
        }
    }

    depths
}

/// Rebuilds `NestingDepthMap` when the model changes. O(V+E) per rebuild.
pub fn compute_nesting_depths(model: Res<Model>, mut depth_map: ResMut<NestingDepthMap>) {
    if !model.is_changed() {
        return;
    }
    depth_map.depths = build_depth_map(&model);
}

/// Y position of day-number labels above the block rows.
const DAY_LABEL_Y: f32 = 55.0;

/// Maps orthographic zoom scale to (stride_days, use_month_format).
/// `stride_days` is the gap between labels; `use_month_format` switches to
/// "Mon YYYY" at far zoom where individual dates are too dense to read.
fn day_step_for_zoom(scale: f32) -> (i32, bool) {
    if scale < 0.5 {
        (1, false)
    } else if scale < 2.0 {
        (5, false)
    } else if scale < 4.0 {
        (10, false)
    } else {
        (30, true)
    }
}

/// Formats a timeline day number as a human-readable date label.
/// `month_only` → "Jun '25";  otherwise → "Jun 16".
fn format_day_label(day: i32, month_only: bool, model: &Model) -> String {
    let date = day_to_date(day as f32, &model.calendar);
    if month_only {
        format!("{} '{:02}", date.format("%b"), date.year() % 100)
    } else {
        format!("{} {}", date.format("%b"), date.day())
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

    let (step, month_only) = day_step_for_zoom(scale);
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
        let label = format_day_label(day, month_only, &model);
        commands.spawn((
            DayLabel,
            Text2d::new(label),
            TextFont {
                font_size: 11.0,
                ..default()
            },
            TextColor(Color::srgba(0.6, 0.6, 0.9, 0.75)),
            Transform::from_xyz(x, DAY_LABEL_Y, 1.0),
        ));
    }
}

/// Pixels of extra left-indent per nesting level for hierarchy brackets.
const DEPTH_INDENT_PX: f32 = 6.0;

/// Draws vertical bracket gizmos for each `Variant`'s children, showing
/// parent/child nesting relationships in the block layout.
///
/// Brackets for deeper nesting levels are shifted further left by
/// `DEPTH_INDENT_PX` per level so nested groups remain visually distinct
/// even when their child blocks share the same x range.
pub fn draw_nesting_indicators(
    schedule: Res<Schedule>,
    model: Res<Model>,
    depth_map: Res<NestingDepthMap>,
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

        // Indent deeper brackets further left so nesting levels are visually distinct.
        let depth = depth_map.depths.get(&variant.parent).copied().unwrap_or(0);
        let bx = left_x - 8.0 - depth as f32 * DEPTH_INDENT_PX;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Estimate, Model};

    fn est() -> Estimate {
        Estimate { most_likely: 3.0, optimistic: 2.0, pessimistic: 5.0, confidence: 0.8 }
    }

    #[test]
    fn empty_model_produces_empty_map() {
        let model = Model::default();
        assert!(build_depth_map(&model).is_empty());
    }

    #[test]
    fn single_root_block_has_depth_zero() {
        let mut model = Model::default();
        let id = model.create_work_block("root", est());
        let depths = build_depth_map(&model);
        assert_eq!(depths[&id], 0);
    }

    #[test]
    fn child_of_variant_has_depth_one() {
        let mut model = Model::default();
        let parent = model.create_work_block("parent", est());
        let child = model.create_work_block("child", est());
        let vid = model.create_variant("v", parent);
        model.work_blocks.get_mut(&parent).unwrap().variants.push(vid);
        model.variants.get_mut(&vid).unwrap().children.push(child);

        let depths = build_depth_map(&model);
        assert_eq!(depths[&parent], 0);
        assert_eq!(depths[&child], 1);
    }

    #[test]
    fn three_level_hierarchy() {
        let mut model = Model::default();
        let root = model.create_work_block("root", est());
        let mid = model.create_work_block("mid", est());
        let leaf = model.create_work_block("leaf", est());

        let v1 = model.create_variant("v1", root);
        model.work_blocks.get_mut(&root).unwrap().variants.push(v1);
        model.variants.get_mut(&v1).unwrap().children.push(mid);

        let v2 = model.create_variant("v2", mid);
        model.work_blocks.get_mut(&mid).unwrap().variants.push(v2);
        model.variants.get_mut(&v2).unwrap().children.push(leaf);

        let depths = build_depth_map(&model);
        assert_eq!(depths[&root], 0);
        assert_eq!(depths[&mid], 1);
        assert_eq!(depths[&leaf], 2);
    }

    #[test]
    fn multiple_roots_all_at_depth_zero() {
        let mut model = Model::default();
        let a = model.create_work_block("a", est());
        let b = model.create_work_block("b", est());
        let depths = build_depth_map(&model);
        assert_eq!(depths[&a], 0);
        assert_eq!(depths[&b], 0);
    }

    #[test]
    fn diamond_uses_first_assigned_depth() {
        let mut model = Model::default();
        let root = model.create_work_block("root", est());
        let shared = model.create_work_block("shared", est());

        let v1 = model.create_variant("v1", root);
        let v2 = model.create_variant("v2", root);
        model.work_blocks.get_mut(&root).unwrap().variants.push(v1);
        model.work_blocks.get_mut(&root).unwrap().variants.push(v2);
        model.variants.get_mut(&v1).unwrap().children.push(shared);
        model.variants.get_mut(&v2).unwrap().children.push(shared);

        let depths = build_depth_map(&model);
        assert_eq!(depths[&root], 0);
        // shared is reachable via v1 and v2 — both paths are depth 1.
        assert_eq!(depths[&shared], 1);
    }

    fn mon_config() -> crate::model::CalendarConfig {
        crate::model::CalendarConfig {
            start_date: chrono::NaiveDate::from_ymd_opt(2025, 6, 16).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
        }
    }

    #[test]
    fn format_day_label_day_zero_shows_start_date() {
        let mut model = Model::default();
        model.calendar = mon_config();
        assert_eq!(format_day_label(0, false, &model), "Jun 16");
    }

    #[test]
    fn format_day_label_five_working_days_is_next_monday() {
        let mut model = Model::default();
        model.calendar = mon_config();
        // 5 working days from Mon Jun 16 = Mon Jun 23
        assert_eq!(format_day_label(5, false, &model), "Jun 23");
    }

    #[test]
    fn format_day_label_month_only_shows_abbreviated_month_and_year() {
        let mut model = Model::default();
        model.calendar = mon_config();
        assert_eq!(format_day_label(0, true, &model), "Jun '25");
    }

    #[test]
    fn day_step_for_zoom_close_is_daily_no_month() {
        let (step, month_only) = day_step_for_zoom(0.3);
        assert_eq!(step, 1);
        assert!(!month_only);
    }

    #[test]
    fn day_step_for_zoom_far_is_monthly_with_month_format() {
        let (step, month_only) = day_step_for_zoom(5.0);
        assert_eq!(step, 30);
        assert!(month_only);
    }
}
