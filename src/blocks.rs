use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use bevy::sprite::Anchor;
use bevy::window::{CursorIcon, SystemCursorIcon};

use crate::{
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    db, graph,
    model::{self, Day, DependencyType, PlanId, WorkBlockId},
    schedule,
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
pub const PALETTE: &[LinearRgba] = &[
    LinearRgba::new(0.25, 0.65, 2.80, 1.0), // indigo
    LinearRgba::new(0.20, 1.70, 0.80, 1.0), // emerald
    LinearRgba::new(2.20, 0.55, 0.15, 1.0), // orange
    LinearRgba::new(1.50, 0.30, 2.20, 1.0), // violet
    LinearRgba::new(0.15, 1.50, 1.60, 1.0), // teal
    LinearRgba::new(2.40, 1.60, 0.10, 1.0), // gold
];

/// The vertical placement of a block bar: `(center_y, height)`, given the
/// block's lane (`row`) within the plan being rendered. Every block — leaf or
/// rolled-up parent — occupies exactly one row on its own level's resource axis.
pub fn block_extent(row: i32) -> (f32, f32) {
    (-(row as f32) * ROW_HEIGHT, BLOCK_HEIGHT)
}

/// The horizontal placement of a block bar: `(left_x, width)`, holiday-aware.
/// The bar runs from its start day to `start + duration` working days; any
/// greyed holiday columns it crosses widen it so both ends stay on the grid.
pub fn block_span_x(wb: &model::WorkBlock, cal: &model::CalendarConfig) -> (f32, f32) {
    let (l, r) = block_edges_x(wb, cal);
    (l, r - l)
}

/// The left and right world-x edges of a block bar, holiday-aware.
pub fn block_edges_x(wb: &model::WorkBlock, cal: &model::CalendarConfig) -> (f32, f32) {
    let left = crate::calendar::day_to_x(wb.start_day, cal);
    let right = crate::calendar::day_to_x(wb.start_day + wb.duration_days, cal);
    (left, right.max(left))
}

/// The fill color a work block renders with: its explicit `color` if set,
/// otherwise the palette default for its lane (`row`) within the plan being
/// rendered. Shared so ghosts in branch swimlanes can outline in exactly the
/// source block's color.
pub fn block_color(wb: &model::WorkBlock, row: i32) -> LinearRgba {
    match wb.color {
        Some([r, g, b]) => LinearRgba::new(r, g, b, 1.0),
        None => PALETTE[row.rem_euclid(PALETTE.len() as i32) as usize],
    }
}

/// Extract the orthographic scale from a camera projection, or `None` for
/// perspective projections. Used by systems that need the current zoom level.
pub(crate) fn ortho_scale(proj: &Projection) -> Option<f32> {
    if let Projection::Orthographic(o) = proj {
        Some(o.scale)
    } else {
        None
    }
}

/// True when `world` falls within the axis-aligned rectangle of `sprite`
/// positioned by `transform`. Returns `false` for sprites without a custom size.
pub(crate) fn sprite_hit(transform: &Transform, sprite: &Sprite, world: Vec2) -> bool {
    let Some(size) = sprite.custom_size else {
        return false;
    };
    let center = transform.translation.truncate();
    let half = size * 0.5;
    world.x >= center.x - half.x
        && world.x <= center.x + half.x
        && world.y >= center.y - half.y
        && world.y <= center.y + half.y
}

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

/// Caches the compare plan's forward_pass result so `sync_compare_overlays` can
/// skip rescheduling on frames where neither the compare plan's block positions
/// nor the active plan's row assignments have changed.
#[derive(Resource, Default)]
pub struct CompareScheduleCache {
    plan_id: Option<PlanId>,
    /// `duration_days` per active block — the only per-block field `forward_pass` reads.
    /// `start_day` is intentionally excluded: the scheduler always places from day 0.
    block_snapshot: HashMap<WorkBlockId, i32>,
    /// Number of dependency edges whose `plan_id` matches the compare plan.
    /// Catches edge additions/removals that would change the scheduled topology.
    dep_count: usize,
    row_snapshot: HashMap<WorkBlockId, i32>,
    sched: Option<schedule::Schedule>,
}

/// Decides whether the cached schedule or sprite rows are stale.
///
/// Returns `(sched_stale, row_stale)`. Extracted as a pure function so the
/// invalidation logic can be unit-tested without constructing Bevy resources.
///
/// `block_snapshot` maps each active block to its `duration_days`.
/// `dep_count` is the number of dependency edges that belong to the compare plan.
pub(crate) fn compare_cache_is_stale(
    cache: &CompareScheduleCache,
    cmp_id: PlanId,
    block_snapshot: &HashMap<WorkBlockId, i32>,
    dep_count: usize,
    id_to_row: &HashMap<WorkBlockId, i32>,
) -> (bool, bool) {
    let plan_changed = cache.plan_id != Some(cmp_id);
    let sched_stale =
        plan_changed || &cache.block_snapshot != block_snapshot || cache.dep_count != dep_count;
    let row_stale = &cache.row_snapshot != id_to_row;
    (sched_stale, row_stale)
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

    // Reconcile each visible block at its explicit, user-assigned lane. The lane
    // is per-plan (freeform, not derived from sort order); the primary timeline
    // always renders the main plan, so rows come from main's block_rows.
    let main_id = model.main_plan_id();
    for &id in &visible_blocks.ids {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        let row = main_id.map(|m| model.block_row(m, id)).unwrap_or(0);

        if let Some(&entity) = sprite_map.entities.get(&id) {
            // Existing entity: update row in place. Transform and color are
            // handled every frame by `sync_block_sprites`.
            if let Ok(mut block_sprite) = sprite_q.get_mut(entity) {
                block_sprite.row = row;
            }
        } else {
            // New entity: spawn parent sprite + label and dot children.
            let (left_x, width) = block_span_x(wb, &model.calendar);
            let x = left_x + width * 0.5;
            let y = -(row as f32) * ROW_HEIGHT;

            let color = Color::from(block_color(wb, row));

            let mut block_cmd = commands.spawn((
                BlockSprite {
                    work_block_id: id,
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
                let available_chars = ((width - 8.0) / LABEL_CHAR_WIDTH) as usize;
                let display = if wb.name.chars().count() > available_chars && available_chars > 0 {
                    let truncated: String = wb
                        .name
                        .chars()
                        .take(available_chars.saturating_sub(1))
                        .collect();
                    format!("{truncated}…")
                } else {
                    wb.name.clone()
                };
                let name = wb.name.clone();
                block_cmd.with_children(|parent| {
                    // Light halo behind the dark text — 1 screen-pixel offset
                    // (updated by sync_block_labels). The blocks are light pastels,
                    // so dark text + a light halo reads far better than the reverse.
                    parent.spawn((
                        BlockLabelShadow {
                            full_name: name.clone(),
                            work_block_id: id,
                        },
                        Text2d::new(display.clone()),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.55)),
                        Anchor::CENTER,
                        Transform::from_xyz(0.0, 0.0, 0.08),
                    ));
                    // Dark main label centered in the block.
                    parent.spawn((
                        BlockLabel {
                            full_name: name,
                            work_block_id: id,
                        },
                        Text2d::new(display),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(Color::srgba(0.10, 0.10, 0.13, 1.0)),
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
                        TextFont {
                            font_size: 14.0,
                            ..default()
                        },
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

    let existing_dots: HashMap<WorkBlockId, Entity> = dot_q
        .iter()
        .map(|(e, dot)| (dot.work_block_id, e))
        .collect();

    for (&id, &sprite_entity) in &sprite_map.entities {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        let (_, width) = block_span_x(wb, &model.calendar);
        let should_have_dot = !wb.description.is_empty() && width >= 12.0;

        match (should_have_dot, existing_dots.get(&id)) {
            (true, None) => {
                commands.entity(sprite_entity).with_children(|parent| {
                    parent.spawn((
                        DescriptionDot { work_block_id: id },
                        Text2d::new("·"),
                        TextFont {
                            font_size: 14.0,
                            ..default()
                        },
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
///   1. Explicit per-block `color` override
///   2. Selection 2× — block is the currently selected block
///   3. Palette default
pub fn sync_block_sprites(
    model: Res<model::Model>,
    selected: Res<SelectedBlock>,
    today: Res<schedule::TodayMarker>,
    camera_q: Query<&Projection, With<Camera2d>>,
    mut query: Query<(&BlockSprite, &mut Transform, &mut Sprite)>,
) {
    let ortho_scale = camera_q.single().ok().and_then(ortho_scale).unwrap_or(1.0);
    let min_width = 8.0 * ortho_scale;

    let main_id = model.main_plan_id();
    for (block_sprite, mut transform, mut sprite) in &mut query {
        let id = block_sprite.work_block_id;
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        let (left_x, width) = block_span_x(wb, &model.calendar);
        // Expand to min_width before computing x so the sprite is always
        // left-anchored at start_day, not centered on the model midpoint.
        let visual_width = width.max(min_width);
        let x = left_x + visual_width * 0.5;
        // Read the live model row (not the cached BlockSprite.row, which only
        // refreshes when the visible set changes) so vertical drags track the
        // cursor immediately — same as start_day does for x. The primary timeline
        // renders the main plan, so the lane comes from main's block_rows.
        let row = main_id.map(|m| model.block_row(m, id)).unwrap_or(0);
        let (y, height) = block_extent(row);
        transform.translation.x = x;
        transform.translation.y = y;
        sprite.custom_size = Some(Vec2::new(visual_width, height));

        let base = PALETTE[row.rem_euclid(PALETTE.len() as i32) as usize];
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
/// Assigns stable row numbers to compare-only blocks (blocks present in the
/// comparison schedule but absent from the active plan's sprite map).
///
/// Extra rows are allocated starting at `max(id_to_row) + 1`, assigned in
/// ascending `WorkBlockId` order so the mapping is deterministic across frames.
/// Returns only the extra-row entries; shared blocks are looked up in `id_to_row`
/// by the caller.
pub(crate) fn assign_compare_extra_rows(
    id_to_row: &HashMap<model::WorkBlockId, i32>,
    compare_ids: impl Iterator<Item = model::WorkBlockId>,
) -> HashMap<model::WorkBlockId, i32> {
    let max_row = id_to_row.values().copied().max().unwrap_or(0);
    let mut extra_ids: Vec<model::WorkBlockId> = compare_ids
        .filter(|id| !id_to_row.contains_key(id))
        .collect();
    extra_ids.sort_by_key(|id| id.0);
    extra_ids
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, max_row + 1 + i as i32))
        .collect()
}

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
    mut cache: ResMut<CompareScheduleCache>,
    block_sprites: Query<&BlockSprite>,
) {
    if !compare_state.is_changed() && !model.is_changed() {
        return;
    }

    let Some(cmp_id) = compare_state.compare_plan_id else {
        for (_, entity) in map.entities.drain() {
            commands.entity(entity).despawn();
        }
        *cache = CompareScheduleCache::default();
        return;
    };
    let Some(cmp_plan) = model.plans.get(&cmp_id).cloned() else {
        for (_, entity) in map.entities.drain() {
            commands.entity(entity).despawn();
        }
        *cache = CompareScheduleCache::default();
        return;
    };

    let block_snapshot: HashMap<WorkBlockId, i32> = cmp_plan
        .root_blocks
        .iter()
        .filter_map(|id| model.work_blocks.get(id).map(|wb| (*id, wb.duration_days)))
        .collect();

    let dep_count = model
        .dependencies
        .values()
        .filter(|d| d.plan_id == cmp_id)
        .count();

    let id_to_row: HashMap<WorkBlockId, i32> = block_sprites
        .iter()
        .map(|bs| (bs.work_block_id, bs.row))
        .collect();

    let (sched_stale, row_stale) =
        compare_cache_is_stale(&cache, cmp_id, &block_snapshot, dep_count, &id_to_row);

    if !sched_stale && !row_stale {
        return;
    }

    if sched_stale {
        let cmp_graph = graph::build_graph(&model, &cmp_plan);
        match schedule::forward_pass(&model, &cmp_graph) {
            Ok(s) => {
                cache.plan_id = Some(cmp_id);
                cache.block_snapshot = block_snapshot;
                cache.dep_count = dep_count;
                cache.sched = Some(s);
            }
            Err(_) => {
                for (_, entity) in map.entities.drain() {
                    commands.entity(entity).despawn();
                }
                *cache = CompareScheduleCache::default();
                return;
            }
        }
    }
    cache.row_snapshot = id_to_row.clone();

    for (_, entity) in map.entities.drain() {
        commands.entity(entity).despawn();
    }
    let cmp_sched = cache.sched.as_ref().unwrap();

    let extra_rows = assign_compare_extra_rows(&id_to_row, cmp_sched.blocks.keys().copied());

    for (&id, cmp_block) in &cmp_sched.blocks {
        let row = id_to_row
            .get(&id)
            .or_else(|| extra_rows.get(&id))
            .copied()
            .unwrap_or(0);

        let lx = crate::calendar::day_to_x(cmp_block.start_day, &model.calendar);
        let rx = crate::calendar::day_to_x(
            cmp_block.start_day + cmp_block.duration_days,
            &model.calendar,
        );
        let width = (rx - lx).max(0.0);
        let x = lx + width * 0.5;
        let y = -(row as f32) * ROW_HEIGHT;

        let active_dur = model.work_blocks.get(&id).map(|wb| wb.duration_days);
        let color = if active_dur.is_none() {
            Color::from(LinearRgba::new(1.2, 0.15, 0.15, 0.45))
        } else if active_dur == Some(cmp_block.duration_days) {
            Color::from(LinearRgba::new(0.5, 0.5, 0.55, 0.22))
        } else {
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
    name_edit: Res<NameEditState>,
    mut label_q: Query<
        (&BlockLabel, &mut Text2d, &mut Visibility, &mut Transform),
        Without<BlockLabelShadow>,
    >,
    mut shadow_q: Query<
        (
            &BlockLabelShadow,
            &mut Text2d,
            &mut Visibility,
            &mut Transform,
        ),
        Without<BlockLabel>,
    >,
) {
    let Ok(proj) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let scale = ortho.scale;

    let block_width = |id: &WorkBlockId| -> f32 {
        model
            .work_blocks
            .get(id)
            .map(|wb| block_span_x(wb, &model.calendar).1)
            .unwrap_or(0.0)
    };

    for (label, mut text2d, mut vis, mut transform) in &mut label_q {
        // The block being renamed shows the seamless in-place editor instead;
        // hide its baked label so the live text and the editor don't overlap.
        if name_edit.editing == Some(label.work_block_id) {
            *vis = Visibility::Hidden;
            continue;
        }
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
        if name_edit.editing == Some(shadow.work_block_id) {
            *vis = Visibility::Hidden;
            continue;
        }
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
    mut selected_plan: ResMut<crate::SelectedPlan>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
    name_edit: Res<NameEditState>,
    model: Res<model::Model>,
    dep_drag: Res<DepDragState>,
    drill: Res<schedule::DrillScope>,
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

    // Branch/plan UI only exists at the plan level; inside a drilled block there
    // are no lanes or markers, so these guards are skipped.
    if drill.path.is_empty() {
        // Below the first branch lane is band territory: those clicks belong to
        // the band handlers, never to main.
        if let Some(top) = crate::bands::bands_top_y(&model) {
            if world_pos.y <= top {
                return;
            }
        }

        // A click on (or near) a branch marker belongs to `handle_branch_selection`.
        let marker_scale = cam_proj.single().ok().and_then(ortho_scale).unwrap_or(1.0);
        if model.main_plan_id().is_some_and(|p| {
            crate::branch_plan_at_x(&model, p, world_pos.x, 6.0 * marker_scale).is_some()
        }) {
            return;
        }
        selected_plan.0 = None;
    }

    // Hit-test against the block sprites.
    let mut clicked: Option<WorkBlockId> = None;
    for (block_sprite, transform, sprite) in &block_query {
        if sprite_hit(transform, sprite, world_pos) {
            clicked = Some(block_sprite.work_block_id);
            break;
        }
    }

    if let Some(id) = clicked {
        // Re-clicking the selected block toggles it off; otherwise select it.
        selected.0 = if Some(id) == selected.0 {
            None
        } else {
            Some(id)
        };
        selected_dep.0 = None;
    } else {
        // A dependency edge under the cursor takes priority — select it (for delete).
        let scale = cam_proj.single().ok().and_then(ortho_scale).unwrap_or(1.0);
        if let Some(dep_id) = nearest_dep_edge(&model, world_pos, 7.0 * scale) {
            selected_dep.0 = Some(dep_id);
            selected.0 = None;
            return;
        }
        selected_dep.0 = None;
        // Empty space: deselect. (Double-click-to-create lives in
        // `handle_canvas_create`.)
        selected.0 = None;
    }
}

/// Double-click empty canvas to create a block. At the plan's top level the
/// block is a new root block (and links through to branches as a ghost); when
/// drilled into a block, it's created as a child of that block instead.
#[allow(clippy::too_many_arguments)]
pub fn handle_canvas_create(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    name_edit: Res<NameEditState>,
    dep_drag: Res<DepDragState>,
    drill: Res<schedule::DrillScope>,
    mut selected: ResMut<SelectedBlock>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
    mut last_click: Local<f32>,
) {
    if name_edit.editing.is_some() || dep_drag.from.is_some() {
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
    let Ok((cam, cam_tr)) = camera.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_tr, cursor) else {
        return;
    };

    // Band territory belongs to the lane handlers — only at the plan level.
    if drill.current().is_none() {
        if let Some(top) = crate::bands::bands_top_y(&model) {
            if world_pos.y <= top {
                return;
            }
        }
    }
    // Only empty space — bail if a block is under the cursor.
    for (_, transform, sprite) in &block_query {
        if sprite_hit(transform, sprite, world_pos) {
            return;
        }
    }

    // Require a double-click (≤ 0.35s).
    let now = time.elapsed_secs();
    if now - *last_click >= 0.35 {
        *last_click = now;
        return;
    }
    *last_click = 0.0;

    // Rows are centered on integers, so round picks the lane the cursor is in.
    // Days are cells starting at the boundary, so floor puts the block in the
    // cell you clicked (round would jump to the next cell past a cell's midpoint).
    let row = (-world_pos.y / ROW_HEIGHT).round() as i32;
    let raw_start = crate::calendar::x_to_day(world_pos.x, &model.calendar).max(0);

    let Some(plan_id) = model.main_plan_id() else {
        return;
    };
    let new_id = if let Some(parent) = drill.current() {
        // Drilled in: the new block is a child of the current block. Children
        // default to 1 day (finer-grained detail than the week-default roots).
        model.add_child_block(plan_id, parent, "New Block", raw_start.max(0), 1, row)
    } else {
        let branch_min = model
            .plans
            .get(&plan_id)
            .and_then(|p| p.branch_start_day)
            .unwrap_or(0);
        let id = model.create_work_block("New Block");
        if let Some(wb) = model.work_blocks.get_mut(&id) {
            wb.start_day = raw_start.max(branch_min);
            wb.duration_days = 5;
        }
        if let Some(plan) = model.plans.get_mut(&plan_id) {
            plan.root_blocks.push(id);
        }
        model.set_block_row(plan_id, id, row);
        // Link through to existing branches as a ghost. No-op off main.
        model.link_main_block_to_branches(id);
        id
    };
    if let Err(e) = db::save_model(&conn, &model) {
        error!("save_model failed: {e}");
    }
    selected.0 = Some(new_id);
}

/// Tracks an in-progress block drag initiated by the user.
#[derive(Resource, Default)]
pub struct DragState {
    /// `(block, x-offset-from-left-edge px, grab-row-delta)`. The row delta is
    /// `block.row − cursor_row` at grab time, so the block keeps its row when you
    /// click without moving (important for tall multi-row roll-up parents — a
    /// click on the lower part must not snap the whole block to that row).
    dragging: Option<(WorkBlockId, f32, i32)>,
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
/// - Held: update `duration_days` so the right edge tracks the cursor, clamped
///   to ≥ 1 day.
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
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
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
            let Some(size) = sprite.custom_size else {
                continue;
            };
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
            // Cancel if the block left the view (e.g. drilled away).
            if !block_query.iter().any(|(bs, _, _)| bs.work_block_id == id) {
                resize.dragging = None;
                return;
            }
            // End the block at the working day nearest the cursor.
            // `x_to_day` resolves any greyed holiday column to the adjacent
            // working day. The `+0.5 * PPD` offset snaps to the *nearest*
            // boundary rather than always flooring, so dragging into the
            // middle of a holiday column lands past it, not before it.
            let start = model.work_blocks.get(&id).map(|wb| wb.start_day);
            if let Some(start) = start {
                let end_day =
                    crate::calendar::x_to_day(world_pos.x + PIXELS_PER_DAY * 0.5, &model.calendar);
                if let Some(wb) = model.work_blocks.get_mut(&id) {
                    wb.duration_days = (end_day - start).max(1);
                }
            }
        }
        return;
    }

    // Release: cascade constraints and persist.
    if mouse.just_released(MouseButton::Left) {
        if let Some(id) = resize.dragging.take() {
            schedule::cascade_dependencies(&mut model, id);
            // Resizing a child may change its parent's rolled-up extent.
            if let Some(parent) = model.work_blocks.get(&id).and_then(|wb| wb.parent) {
                model.recompute_rollup(parent);
            }
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
            if sprite_hit(transform, sprite, world_pos) {
                let id = block_sprite.work_block_id;
                let start_px = model
                    .work_blocks
                    .get(&id)
                    .map(|wb| crate::calendar::day_to_x(wb.start_day, &model.calendar))
                    .unwrap_or(0.0);
                let block_row = model
                    .main_plan_id()
                    .map(|p| model.block_row(p, id))
                    .unwrap_or(0);
                let cursor_row = (-world_pos.y / ROW_HEIGHT).round() as i32;
                // Offsets preserve where within the block the user grabbed, in x
                // and in rows — so a click without dragging never moves it.
                drag.dragging = Some((id, world_pos.x - start_px, block_row - cursor_row));
                // Selection is owned by handle_block_selection (which toggles on
                // re-click); don't re-select here or a second click can't deselect.
                break;
            }
        }
        return;
    }

    // Held: slide start_day to follow cursor.
    if mouse.pressed(MouseButton::Left) {
        if let Some((id, offset_px, grab_row_delta)) = drag.dragging {
            // Cancel if the block left the view (e.g. a double-click drilled into
            // it) — never keep moving a block that's no longer on screen.
            if !block_query.iter().any(|(bs, _, _)| bs.work_block_id == id) {
                drag.dragging = None;
                return;
            }
            let branch_min = model
                .main_plan_id()
                .and_then(|p| model.plans.get(&p))
                .and_then(|p| p.branch_start_day)
                .unwrap_or(0);
            let new_start = crate::calendar::x_to_day(
                world_pos.x - offset_px + PIXELS_PER_DAY * 0.5,
                &model.calendar,
            )
            .max(0);
            // Dependencies don't constrain a drag — you can place a block into a
            // violation; the offending edge just turns red. (Only >= 0 / the
            // fork day are enforced.)
            let new_start = new_start.max(branch_min);
            // Row follows the cursor but offset by where you grabbed, so a tall
            // block keeps its top row when clicked without moving.
            let cursor_row = (-world_pos.y / ROW_HEIGHT).round() as i32;
            let new_row = cursor_row + grab_row_delta;
            // Only write when values changed — avoids tripping is_changed() every
            // held frame when the cursor hasn't moved.
            let cur_start = model.work_blocks.get(&id).map(|wb| wb.start_day);
            if cur_start != Some(new_start) {
                if let Some(wb) = model.work_blocks.get_mut(&id) {
                    wb.start_day = new_start;
                }
            }
            if let Some(p) = model.main_plan_id() {
                if model.block_row(p, id) != new_row {
                    model.set_block_row(p, id, new_row);
                }
            }
        }
        return;
    }

    // Release: cascade dependencies and persist.
    if mouse.just_released(MouseButton::Left) {
        if let Some((id, _, _)) = drag.dragging.take() {
            schedule::cascade_dependencies(&mut model, id);
            // Moving a child may change its parent's rolled-up extent.
            if let Some(parent) = model.work_blocks.get(&id).and_then(|wb| wb.parent) {
                model.recompute_rollup(parent);
            }
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

    let existing: HashMap<PastPortionOverlay, Entity> =
        overlay_q.iter().map(|(e, k, _, _)| (*k, e)).collect();

    struct Overlay {
        key: PastPortionOverlay,
        pos: Vec3,
        size: Vec2,
    }
    let mut desired: Vec<Overlay> = Vec::new();

    let main_id = model.main_plan_id();
    for &id in &visible_blocks.ids {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        let end_day = wb.start_day + wb.duration_days;
        if wb.start_day >= today.day || end_day <= today.day {
            continue;
        }
        let x_left = crate::calendar::day_to_x(wb.start_day, &model.calendar);
        let past_width = crate::calendar::day_to_x(today.day, &model.calendar) - x_left;
        let y = -(main_id.map(|m| model.block_row(m, id)).unwrap_or(0) as f32) * ROW_HEIGHT;
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
                Sprite {
                    color: overlay_color,
                    custom_size: Some(ov.size),
                    ..default()
                },
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
    let scale = cam_q.single().ok().and_then(ortho_scale).unwrap_or(1.0);

    for (bs, transform, sprite) in &block_q {
        let Some(wb) = model.work_blocks.get(&bs.work_block_id) else {
            continue;
        };

        let (rings, color) = match wb.priority {
            0 => continue,
            1 => (1usize, Color::srgba(1.0, 1.0, 1.0, 0.40)),
            2 => (2usize, Color::srgba(1.0, 1.0, 1.0, 0.80)),
            _ => (3usize, Color::from(LinearRgba::new(3.0, 2.2, 0.4, 1.0))),
        };

        let Some(size) = sprite.custom_size else {
            continue;
        };
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
#[allow(clippy::too_many_arguments)]
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
    let main_id = model.main_plan_id();
    let geom: HashMap<WorkBlockId, BlockGeom> = visible_blocks
        .ids
        .iter()
        .filter_map(|&id| {
            let wb = model.work_blocks.get(&id)?;
            let (xl, xr) = block_edges_x(wb, &model.calendar);
            Some((
                wb.id,
                BlockGeom {
                    xl,
                    xr,
                    y: -(main_id.map(|m| model.block_row(m, id)).unwrap_or(0) as f32) * ROW_HEIGHT,
                },
            ))
        })
        .collect();

    let ortho_scale = cam_proj.single().ok().and_then(ortho_scale).unwrap_or(1.0);

    // Fade edges between LOD_FAR_MIN and LOD_DEP_HIDE; skip entirely beyond LOD_DEP_HIDE.
    let edge_alpha = if ortho_scale <= LOD_FAR_MIN {
        1.0_f32
    } else {
        ((LOD_DEP_HIDE - ortho_scale) / (LOD_DEP_HIDE - LOD_FAR_MIN)).clamp(0.0, 1.0)
    };

    if edge_alpha > 0.0 {
        for (dep_id, dep) in &model.dependencies {
            let (Some(pg), Some(sg)) = (geom.get(&dep.predecessor), geom.get(&dep.successor))
            else {
                continue;
            };

            // Arrow points FROM the dependent (successor) TO what it depends on
            // (predecessor), so the arrowhead sits on the predecessor's anchor.
            let (src, dst) =
                dep_draw_endpoints(dep.dependency_type, pg.xl, pg.xr, pg.y, sg.xl, sg.xr, sg.y);

            let is_selected = selected_dep.0 == Some(*dep_id);
            let color = if is_selected {
                Color::srgba(1.7, 1.2, 0.25, edge_alpha.max(0.9)) // bright selection highlight
            } else if !schedule::dependency_satisfied(&model, dep) {
                Color::srgba(2.2, 0.25, 0.25, edge_alpha.max(0.85)) // violated → red
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

pub(crate) fn draw_arrowhead(gizmos: &mut Gizmos, src: Vec2, dst: Vec2, color: Color) {
    let dir = (dst - src).normalize_or_zero();
    if dir == Vec2::ZERO {
        return;
    }
    let perp = Vec2::new(-dir.y, dir.x);
    gizmos.line_2d(dst, dst - dir * 8.0 + perp * 4.0, color);
    gizmos.line_2d(dst, dst - dir * 8.0 - perp * 4.0, color);
}

/// Arrow endpoints for drawing a dependency: `(src=successor_anchor,
/// dst=predecessor_anchor)`. The arrowhead sits at `dst` (the predecessor).
/// Shared between the main timeline and branch swimlanes.
pub(crate) fn dep_draw_endpoints(
    dep_type: DependencyType,
    pred_xl: f32,
    pred_xr: f32,
    pred_y: f32,
    succ_xl: f32,
    succ_xr: f32,
    succ_y: f32,
) -> (Vec2, Vec2) {
    match dep_type {
        DependencyType::FinishToStart => (Vec2::new(succ_xl, succ_y), Vec2::new(pred_xr, pred_y)),
        DependencyType::StartToStart => (Vec2::new(succ_xl, succ_y), Vec2::new(pred_xl, pred_y)),
        DependencyType::FinishToFinish => (Vec2::new(succ_xr, succ_y), Vec2::new(pred_xr, pred_y)),
        DependencyType::StartToFinish => (Vec2::new(succ_xr, succ_y), Vec2::new(pred_xl, pred_y)),
    }
}

/// World-space endpoints of a dependency edge for click hit-testing. Returns
/// `None` if a block is missing or unplaced. Order follows `dep_draw_endpoints`
/// (succ_anchor, pred_anchor); distance via `point_segment_dist` is direction-independent.
fn dep_endpoints(model: &model::Model, dep: &model::Dependency) -> Option<(Vec2, Vec2)> {
    let pred = model.work_blocks.get(&dep.predecessor)?;
    let succ = model.work_blocks.get(&dep.successor)?;
    if pred.duration_days <= 0 || succ.duration_days <= 0 {
        return None;
    }
    let (p_xl, p_xr) = block_edges_x(pred, &model.calendar);
    let p_y = -(model.block_row(dep.plan_id, dep.predecessor) as f32) * ROW_HEIGHT;
    let (s_xl, s_xr) = block_edges_x(succ, &model.calendar);
    let s_y = -(model.block_row(dep.plan_id, dep.successor) as f32) * ROW_HEIGHT;
    Some(dep_draw_endpoints(
        dep.dependency_type,
        p_xl,
        p_xr,
        p_y,
        s_xl,
        s_xr,
        s_y,
    ))
}

/// Distance from point `p` to segment `a`–`b`.
pub(crate) fn point_segment_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
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
pub(crate) fn dep_type_from_edges(source_finish: bool, target_finish: bool) -> DependencyType {
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
    let Ok((cam, cam_tr)) = camera.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_tr, cursor) else {
        return;
    };

    // One subtle connector dot; brightens slightly (no white, no big grow) when
    // hovered or while it's the drag source.
    let dot = Color::srgba(0.55, 0.62, 0.78, 0.65);
    let dot_hi = Color::srgba(0.80, 0.86, 0.98, 0.95);

    let main_id = model.main_plan_id();
    for &id in &visible_blocks.ids {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        if wb.duration_days <= 0 {
            continue;
        }
        let y = -(main_id.map(|m| model.block_row(m, id)).unwrap_or(0) as f32) * ROW_HEIGHT;
        let (xl, xr) = block_edges_x(wb, &model.calendar);
        let half_h = BLOCK_HEIGHT * 0.5;

        let is_source = drag.from == Some(wb.id);

        // Show the handle when hovering this block or while it's the drag source.
        let in_block = world_pos.x >= xl && world_pos.x <= xr && (world_pos.y - y).abs() <= half_h;
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
    model: Res<model::Model>,
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
    let mut hit_main = false;
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
            hit_main = true;
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
            hit_main = true;
            break;
        }
    }

    // Fall through to the branch lanes when not over a main block.
    if !hit_main {
        if let Some(lane_icon) = crate::bands::lane_cursor_at(&model, world_pos) {
            icon = lane_icon;
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
    let Ok((cam, cam_tr)) = camera.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_tr, cursor) else {
        return;
    };

    // Returns the block under `pos` and whether `pos` is in its right (finish) half.
    let block_at = |pos: Vec2| -> Option<(WorkBlockId, bool)> {
        for (bs, tr, sp) in &block_query {
            if sprite_hit(tr, sp, pos) {
                return Some((bs.work_block_id, pos.x >= tr.translation.x));
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
    // is implied by which edge you grabbed and which half you dropped on. You
    // drag FROM the dependent (successor) TO the block it depends on
    // (predecessor): from-edge = the successor's anchor, drop-edge = the
    // predecessor's anchor. (`dep_type_from_edges` is (predecessor_finish,
    // successor_finish), so pass the drop/finish flags in that order.)
    if mouse.just_released(MouseButton::Left) {
        if let Some(succ_id) = drag.from.take() {
            if let Some((pred_id, pred_finish)) = block_at(world_pos) {
                if pred_id != succ_id {
                    let dep_type = dep_type_from_edges(pred_finish, drag.from_right);
                    // Create only (idempotent); deletion is click-the-edge + Delete.
                    let exists = model.dependencies.values().any(|d| {
                        d.predecessor == pred_id
                            && d.successor == succ_id
                            && d.dependency_type == dep_type
                    });
                    if !exists {
                        model.create_dependency(pred_id, succ_id, dep_type);
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
        if let Some(succ_id) = drag.from.take() {
            if let Some((pred_id, pred_finish)) = block_at(world_pos) {
                if pred_id != succ_id {
                    let dep_type = dep_type_from_edges(pred_finish, drag.from_right);
                    let exists = model.dependencies.values().any(|d| {
                        d.predecessor == pred_id
                            && d.successor == succ_id
                            && d.dependency_type == dep_type
                    });
                    if !exists {
                        model.create_dependency(pred_id, succ_id, dep_type);
                        if let Err(e) = crate::db::save_model(&conn, &model) {
                            error!("save_model failed: {e}");
                        }
                    }
                }
            }
        }
    }
}

/// Double-click a block to "drill in" — push it onto the drill path so the
/// timeline shows its children (a mini-plan you edit the same way). Renaming is
/// now select-then-type (`handle_type_to_rename`), so double-click is free.
///
/// Must run before `handle_block_selection` so the guard there sees the updated
/// drill state on the same frame.
#[allow(clippy::too_many_arguments)]
pub fn handle_block_drill(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    name_edit: Res<NameEditState>,
    mut drill: ResMut<schedule::DrillScope>,
    mut selected: ResMut<SelectedBlock>,
    block_query: Query<(&BlockSprite, &Transform, &Sprite)>,
    mut last_click: Local<Option<(WorkBlockId, f32)>>,
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
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) else {
        return;
    };

    let now = time.elapsed_secs();

    for (block_sprite, transform, sprite) in &block_query {
        if sprite_hit(transform, sprite, world_pos) {
            let id = block_sprite.work_block_id;
            if let Some((last_id, last_time)) = *last_click {
                if last_id == id && now - last_time < 0.4 {
                    *last_click = None;
                    drill.path.push(id);
                    selected.0 = None; // the old selection isn't in the new view
                    return;
                }
            }
            *last_click = Some((id, now));
            return;
        }
    }

    *last_click = None;
}

/// Escape drills out one level (when not editing a name). Paired with the
/// breadcrumb in the top bar for jumping multiple levels at once.
pub fn handle_drill_out(
    mut egui_ctx: EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    name_edit: Res<NameEditState>,
    mut drill: ResMut<schedule::DrillScope>,
) {
    if name_edit.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            return;
        }
    }
    if keyboard.just_pressed(KeyCode::Escape) && !drill.path.is_empty() {
        drill.path.pop();
    }
}

/// When a block is selected and you start typing a character, begin renaming it
/// with that character replacing the name (spreadsheet-style). Modifier combos
/// (Ctrl/Cmd) are ignored so shortcuts don't trigger a rename.
pub fn handle_type_to_rename(
    mut egui_ctx: EguiContexts,
    selected: Res<SelectedBlock>,
    lane_selected: Res<crate::bands::LaneSelection>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut key_events: MessageReader<bevy::input::keyboard::KeyboardInput>,
    mut name_edit: ResMut<NameEditState>,
    mut lane_rename: ResMut<crate::bands::LaneBlockRename>,
) {
    if name_edit.editing.is_some() || lane_rename.editing.is_some() {
        key_events.clear();
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            key_events.clear();
            return;
        }
    }
    // A selected main block renames via the main overlay; a selected lane block
    // via the lane overlay. Exactly one is selected at a time.
    let target = selected
        .0
        .map(|id| (id, false))
        .or_else(|| lane_selected.0.map(|(id, _)| (id, true)));
    let Some((id, is_lane)) = target else {
        key_events.clear();
        return;
    };
    let modifier = keyboard.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    for ev in key_events.read() {
        if !ev.state.is_pressed() || modifier {
            continue;
        }
        if let bevy::input::keyboard::Key::Character(s) = &ev.logical_key {
            if is_lane {
                lane_rename.editing = Some(id);
                lane_rename.buf = s.to_string();
            } else {
                name_edit.editing = Some(id);
                name_edit.text_buf = s.to_string();
            }
            return;
        }
    }
}

/// Renders an egui `TextEdit` overlay anchored to the editing block's screen
/// position while `NameEditState::editing` is `Some`. Commits on Enter or
/// focus-loss; cancels on Escape. On commit, persists to model + DB; the model
/// change triggers `sync_block_label_names` which updates `BlockLabel::full_name`
/// so the display text reflects the new name on the next frame.
#[allow(clippy::too_many_arguments)]
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
    let Some(edit_id) = name_edit.editing else {
        return;
    };

    let Ok(_window) = windows.single() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };

    // Center the field on the block being renamed (fall back near the corner).
    let mut center = egui::pos2(80.0, 120.0);
    for (bs, transform) in &block_query {
        if bs.work_block_id == edit_id {
            if let Ok(vp) = camera.world_to_viewport(camera_transform, transform.translation) {
                center = egui::pos2(vp.x, vp.y);
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
        // Seamless in-place edit: no panel/box. The field is transparent and the
        // text matches the block's baked label (dark, 13px), so it reads as
        // editing the name where it sits. The field is a fixed width anchored on
        // the block, so text extends rightward as you type rather than sliding.
        const W: f32 = 160.0;
        egui::Area::new(egui::Id::new("name_edit_overlay"))
            .fixed_pos(egui::pos2(center.x - W * 0.5, center.y - 9.0))
            .show(ctx, |ui| {
                ui.visuals_mut().extreme_bg_color = egui::Color32::TRANSPARENT;
                ui.visuals_mut().widgets.active.bg_stroke = egui::Stroke::NONE;
                ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::NONE;
                let response = ui.add(
                    egui::TextEdit::singleline(&mut name_edit.text_buf)
                        .desired_width(W)
                        .horizontal_align(egui::Align::Center)
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        .font(egui::FontId::proportional(13.0))
                        .text_color(egui::Color32::from_rgb(26, 26, 33)),
                );
                response.request_focus();
                // Commit if focus is lost (Tab, click outside, etc.).
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
    dependencies: Vec<model::Dependency>,
    /// (plan_id, root_block_ids that were in this plan)
    plan_roots: Vec<(model::PlanId, Vec<WorkBlockId>)>,
    /// Surviving blocks whose `parent` pointed at the deleted block — cleared to
    /// `None` by `delete_work_block` and restored to the deleted id on undo.
    reparented_children: Vec<WorkBlockId>,
}

/// Single-slot undo buffer for block deletions. Holds the most recent deletion;
/// overwritten on each delete; consumed by undo.
#[derive(Resource, Default)]
pub struct UndoStack {
    last_deletion: Option<DeletedBlockSnapshot>,
}

fn build_deletion_snapshot(model: &model::Model, id: WorkBlockId) -> DeletedBlockSnapshot {
    // Only the block itself is deleted (blocks are flat now).
    let blocks = model
        .work_blocks
        .get(&id)
        .cloned()
        .into_iter()
        .collect::<Vec<_>>();
    let dependencies = model
        .dependencies
        .values()
        .filter(|d| d.predecessor == id || d.successor == id)
        .cloned()
        .collect();

    let mut plan_roots = Vec::new();
    for (&plan_id, plan) in &model.plans {
        if plan.root_blocks.contains(&id) {
            plan_roots.push((plan_id, vec![id]));
        }
    }

    // Surviving blocks parented to the deleted block — delete_work_block clears
    // their parent, and undo must restore it.
    let reparented_children = model
        .work_blocks
        .values()
        .filter(|wb| wb.parent == Some(id))
        .map(|wb| wb.id)
        .collect();

    DeletedBlockSnapshot {
        blocks,
        dependencies,
        plan_roots,
        reparented_children,
    }
}

fn restore_deletion_snapshot(model: &mut model::Model, snap: DeletedBlockSnapshot) {
    let restored_ids: Vec<WorkBlockId> = snap.blocks.iter().map(|wb| wb.id).collect();
    for wb in snap.blocks {
        model.work_blocks.insert(wb.id, wb);
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
    // Re-point children at the restored parent (the deleted block).
    if let Some(&parent_id) = restored_ids.first() {
        for child_id in snap.reparented_children {
            if let Some(child) = model.work_blocks.get_mut(&child_id) {
                child.parent = Some(parent_id);
            }
        }
    }
}

/// Detects Delete/Backspace and immediately removes the selected block from the
/// model. Runs in Update BEFORE `update_visible_blocks` so sprite reconciliation
/// fires in the same frame — this avoids the timing bug where a deletion in
/// `EguiPrimaryContextPass` would be invisible to `is_changed()` the next frame.
#[allow(clippy::too_many_arguments)]
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

/// Remove a single work block from the model, cleaning up all cross-references.
///
/// Deleted:
/// - The work block itself
/// - All `Dependency` edges that touch it
/// - Entries in `plan.root_blocks` and `plan.block_rows` for it
///
/// Any surviving block whose `parent` pointed at the deleted block has its
/// `parent` reset to `None` to avoid a dangling reference.
pub fn delete_work_block(model: &mut model::Model, id: WorkBlockId) {
    model.work_blocks.remove(&id);
    model
        .dependencies
        .retain(|_, dep| dep.predecessor != id && dep.successor != id);
    for plan in model.plans.values_mut() {
        plan.root_blocks.retain(|&bid| bid != id);
        plan.block_rows.remove(&id);
    }
    // Clear dangling parent references on surviving children.
    for wb in model.work_blocks.values_mut() {
        if wb.parent == Some(id) {
            wb.parent = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;

    #[test]
    fn delete_simple_block_removes_it() {
        let mut m = Model::default();
        let a = m.create_work_block("A");
        delete_work_block(&mut m, a);
        assert!(!m.work_blocks.contains_key(&a));
    }

    #[test]
    fn delete_block_clears_dangling_child_parent() {
        let mut m = Model::default();
        let parent = m.create_work_block("P");
        let child = m.create_work_block("C");
        m.work_blocks.get_mut(&child).unwrap().parent = Some(parent);

        delete_work_block(&mut m, parent);

        assert!(!m.work_blocks.contains_key(&parent), "parent removed");
        assert!(m.work_blocks.contains_key(&child), "child survives");
        assert_eq!(
            m.work_blocks.get(&child).unwrap().parent,
            None,
            "child parent cleared"
        );
    }

    #[test]
    fn delete_block_cleans_plan_root_and_rows() {
        let mut m = Model::default();
        let pid = m.create_plan("p", None);
        let a = m.create_work_block("A");
        m.plans.get_mut(&pid).unwrap().root_blocks.push(a);
        m.set_block_row(pid, a, 3);

        delete_work_block(&mut m, a);

        assert!(!m.plans[&pid].root_blocks.contains(&a));
        assert!(!m.plans[&pid].block_rows.contains_key(&a));
    }

    #[test]
    fn delete_block_removes_its_dependencies() {
        use crate::model::DependencyType;
        let mut m = Model::default();
        let a = m.create_work_block("A");
        let b = m.create_work_block("B");
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);

        delete_work_block(&mut m, a);

        assert!(!m.dependencies.contains_key(&dep));
        assert!(m.work_blocks.contains_key(&b), "B survives");
    }

    #[test]
    fn delete_block_only_removes_itself() {
        // Blocks are flat: deleting a parent leaves its children in place
        // (with parent cleared), it does not cascade.
        let mut m = Model::default();
        let parent = m.create_work_block("P");
        let child = m.create_work_block("C");
        m.work_blocks.get_mut(&child).unwrap().parent = Some(parent);

        delete_work_block(&mut m, parent);

        assert!(!m.work_blocks.contains_key(&parent));
        assert!(m.work_blocks.contains_key(&child), "child not cascaded");
        assert_eq!(m.work_blocks.get(&child).unwrap().parent, None);
    }

    // ── Drag / row derivation ────────────────────────────────────────────────

    #[test]
    fn row_derivation_negative_y_maps_to_correct_row() {
        // world_y is always negative (rows go down). The formula used on drag is
        // `(-world_y / ROW_HEIGHT).round() as i32`.
        let r = |y: f32| (-y / ROW_HEIGHT).round() as i32;
        assert_eq!(r(0.0), 0);
        assert_eq!(r(-40.0), 1); // exactly row 1
        assert_eq!(r(-80.0), 2); // exactly row 2
        assert_eq!(r(-20.0), 1); // midpoint: 0.5f32.round() == 1 (half-away-from-zero)
        assert_eq!(r(-60.0), 2); // midpoint between row 1 and 2
        assert_eq!(r(-39.0), 1); // just below row 1 boundary
        assert_eq!(r(-1.0), 0); // almost row 0
    }

    #[test]
    fn resize_day_from_world_x() {
        // handle_block_resize snaps the right edge to the nearest day via
        // `x_to_day(world_x + PIXELS_PER_DAY * 0.5, &cal)`. Test that
        // clicking squarely inside a day resolves to that day (no holiday).
        use crate::model::CalendarConfig;
        let cal = CalendarConfig::default();
        let snap = |x: f32| crate::calendar::x_to_day(x + PIXELS_PER_DAY * 0.5, &cal);
        // Clicking in the middle of day 3's column → day 3.
        assert_eq!(snap(3.0 * PIXELS_PER_DAY + PIXELS_PER_DAY * 0.5), 4);
        // Clicking at the start of day 0's column → day 0.
        assert_eq!(snap(0.0), 0);
        // Clicking just before the day-5 boundary → day 4.
        assert_eq!(snap(4.5 * PIXELS_PER_DAY - 0.1), 4);
    }

    #[test]
    fn row_derivation_never_negative_when_clamped() {
        // handle_block_drag clamps with .max(0): a positive world_y (above the
        // origin) must never produce a negative row.
        let r = |y: f32| (-y / ROW_HEIGHT).round().max(0.0) as i32;
        assert_eq!(r(40.0), 0);
        assert_eq!(r(200.0), 0);
    }

    // ── dep_type_from_edges ──────────────────────────────────────────────────

    #[test]
    fn dep_type_finish_to_start() {
        assert_eq!(
            dep_type_from_edges(true, false),
            crate::model::DependencyType::FinishToStart
        );
    }

    #[test]
    fn dep_type_finish_to_finish() {
        assert_eq!(
            dep_type_from_edges(true, true),
            crate::model::DependencyType::FinishToFinish
        );
    }

    #[test]
    fn dep_type_start_to_start() {
        assert_eq!(
            dep_type_from_edges(false, false),
            crate::model::DependencyType::StartToStart
        );
    }

    #[test]
    fn dep_type_start_to_finish() {
        assert_eq!(
            dep_type_from_edges(false, true),
            crate::model::DependencyType::StartToFinish
        );
    }

    // ── block_edges_x / block_span_x ────────────────────────────────────────

    #[test]
    fn block_edges_x_no_holidays_simple() {
        use crate::model::CalendarConfig;
        let mut m = Model::default();
        let id = m.create_work_block("A");
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = 0;
        wb.duration_days = 5;
        let cal = CalendarConfig::default(); // no holidays
        let (left, right) = block_edges_x(wb, &cal);
        assert!((left - 0.0).abs() < 0.001, "left at day 0");
        assert!(
            (right - 5.0 * PIXELS_PER_DAY).abs() < 0.001,
            "right at day 5"
        );
    }

    #[test]
    fn block_span_x_width_equals_right_minus_left() {
        use crate::model::CalendarConfig;
        let mut m = Model::default();
        let id = m.create_work_block("A");
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = 2;
        wb.duration_days = 3;
        let cal = CalendarConfig::default();
        let (left, width) = block_span_x(wb, &cal);
        let (l2, r2) = block_edges_x(wb, &cal);
        assert!((left - l2).abs() < 0.001);
        assert!((width - (r2 - l2)).abs() < 0.001);
    }

    #[test]
    fn block_edges_x_holiday_within_span_widens_right_edge() {
        // A holiday on 2025-01-03 (working day 2) inserts a visual column before
        // day 3. A block from day 1 to day 4 crosses it, so right_x is wider.
        use crate::model::{CalendarConfig, NonWorkingDate};
        use chrono::NaiveDate;
        let mut cal = CalendarConfig::default();
        let holiday = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(); // a Friday
        cal.non_working_dates = vec![NonWorkingDate {
            date: holiday,
            description: String::new(),
        }];

        let mut m = Model::default();
        let id = m.create_work_block("A");
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = 1;
        wb.duration_days = 3; // ends at day 4

        let (left_h, right_h) = block_edges_x(wb, &cal);

        let cal_no_hol = CalendarConfig::default();
        let (left_n, right_n) = block_edges_x(wb, &cal_no_hol);

        // Left edge is before the holiday column so it stays the same.
        assert!((left_h - left_n).abs() < 0.001, "left unchanged");
        // Right edge is pushed out by one holiday column.
        assert!(
            (right_h - right_n - PIXELS_PER_DAY).abs() < 0.001,
            "right wider by one day: got {right_h} expected {}",
            right_n + PIXELS_PER_DAY
        );
    }

    #[test]
    fn block_edges_x_holiday_before_span_shifts_both_edges() {
        // Holiday before the block shifts both left and right by one column.
        use crate::model::{CalendarConfig, NonWorkingDate};
        use chrono::NaiveDate;
        let mut cal = CalendarConfig::default();
        let holiday = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(); // day 2
        cal.non_working_dates = vec![NonWorkingDate {
            date: holiday,
            description: String::new(),
        }];

        let mut m = Model::default();
        let id = m.create_work_block("A");
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = 4; // starts after the holiday column (boundary is at 3)
        wb.duration_days = 2;

        let (left_h, right_h) = block_edges_x(wb, &cal);
        let (left_n, right_n) = block_edges_x(wb, &CalendarConfig::default());

        assert!(
            (left_h - left_n - PIXELS_PER_DAY).abs() < 0.001,
            "left shifted right by one"
        );
        assert!(
            (right_h - right_n - PIXELS_PER_DAY).abs() < 0.001,
            "right shifted right by one"
        );
    }

    // ── fit_label ────────────────────────────────────────────────────────────

    #[test]
    fn fit_label_short_name_fits_unchanged() {
        // 200 world px / 1.0 scale = 200 screen px → max_chars = (200-8)/8 = 24
        let result = fit_label("Hello", 200.0, 1.0);
        assert_eq!(result, Some("Hello".to_string()));
    }

    #[test]
    fn fit_label_long_name_gets_truncated_with_ellipsis() {
        // 80 world px / 1.0 scale = 80 screen px → max_chars = (80-8)/8 = 9
        // 9 chars: keep 8, append "…"
        let result = fit_label("Hello World Long", 80.0, 1.0);
        assert_eq!(result, Some("Hello Wo…".to_string()));
    }

    #[test]
    fn fit_label_too_narrow_returns_none() {
        // 8 world px / 1.0 scale = 8 screen px → max_chars = (8-8)/8 = 0 < 1
        let result = fit_label("Hi", 8.0, 1.0);
        assert_eq!(result, None);
    }

    #[test]
    fn fit_label_scale_beyond_far_lod_returns_none() {
        // scale > LOD_FAR_MIN (6.0) → always None regardless of block width
        let result = fit_label("Hello", 1000.0, 7.0);
        assert_eq!(result, None);
    }

    #[test]
    fn fit_label_exactly_at_far_lod_boundary_proceeds() {
        // scale == LOD_FAR_MIN is NOT > LOD_FAR_MIN, so it falls through to the
        // length check. Block is wide enough for the full name.
        let result = fit_label("Hi", 200.0, LOD_FAR_MIN);
        assert_eq!(result, Some("Hi".to_string()));
    }

    #[test]
    fn fit_label_one_char_max_returns_ellipsis_only() {
        // 16 world px / 1.0 = 16 screen px → max_chars = (16-8)/8 = 1
        // Name is longer than 1 char → "…"
        let result = fit_label("Hi", 16.0, 1.0);
        assert_eq!(result, Some("…".to_string()));
    }

    // ── undo snapshot round-trip ─────────────────────────────────────────────

    #[test]
    fn undo_snapshot_round_trip_single_block() {
        let mut m = Model::default();
        let plan = m.create_plan("p", None);
        let a = m.create_work_block("A");
        m.plans.get_mut(&plan).unwrap().root_blocks.push(a);

        let snap = build_deletion_snapshot(&m, a);
        delete_work_block(&mut m, a);
        assert!(!m.work_blocks.contains_key(&a), "block gone after delete");
        assert!(!m.plans[&plan].root_blocks.contains(&a), "root cleared");

        restore_deletion_snapshot(&mut m, snap);
        assert!(m.work_blocks.contains_key(&a), "block restored");
        assert!(m.plans[&plan].root_blocks.contains(&a), "root restored");
    }

    #[test]
    fn undo_snapshot_restores_dependencies() {
        use crate::model::DependencyType;
        let mut m = Model::default();
        let _plan = m.create_plan("p", None);
        let a = m.create_work_block("A");
        let b = m.create_work_block("B");
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);

        let snap = build_deletion_snapshot(&m, a);
        delete_work_block(&mut m, a);
        assert!(!m.dependencies.contains_key(&dep), "dep gone");

        restore_deletion_snapshot(&mut m, snap);
        assert!(m.dependencies.contains_key(&dep), "dep restored");
        assert!(m.work_blocks.contains_key(&b), "B survived throughout");
    }

    #[test]
    fn undo_snapshot_restores_reparented_children() {
        let mut m = Model::default();
        let _plan = m.create_plan("p", None);
        let parent = m.create_work_block("P");
        let child = m.create_work_block("C");
        m.work_blocks.get_mut(&child).unwrap().parent = Some(parent);

        let snap = build_deletion_snapshot(&m, parent);
        delete_work_block(&mut m, parent);
        assert_eq!(
            m.work_blocks.get(&child).unwrap().parent,
            None,
            "child parent cleared by delete"
        );

        restore_deletion_snapshot(&mut m, snap);
        assert!(m.work_blocks.contains_key(&parent), "parent restored");
        assert_eq!(
            m.work_blocks.get(&child).unwrap().parent,
            Some(parent),
            "child re-parented on restore"
        );
    }

    #[test]
    fn undo_snapshot_dep_only_for_deleted_block() {
        // Snapshot of A must NOT include the B→C dep that doesn't touch A.
        use crate::model::DependencyType;
        let mut m = Model::default();
        let _plan = m.create_plan("p", None);
        let a = m.create_work_block("A");
        let b = m.create_work_block("B");
        let c = m.create_work_block("C");
        let _dep_bc = m.create_dependency(b, c, DependencyType::FinishToStart);
        let dep_ab = m.create_dependency(a, b, DependencyType::FinishToStart);

        let snap = build_deletion_snapshot(&m, a);
        assert_eq!(snap.dependencies.len(), 1);
        assert_eq!(snap.dependencies[0].id, dep_ab);
    }

    // ---- compare_cache_is_stale tests ----

    fn make_cache(
        cmp_id: WorkBlockId,
        block_snapshot: HashMap<WorkBlockId, i32>,
        dep_count: usize,
        row_snapshot: HashMap<WorkBlockId, i32>,
    ) -> super::CompareScheduleCache {
        use crate::model::PlanId;
        super::CompareScheduleCache {
            plan_id: Some(PlanId(cmp_id.0)),
            block_snapshot,
            dep_count,
            row_snapshot,
            sched: None,
        }
    }

    #[test]
    fn cache_is_stale_on_plan_id_change() {
        use crate::model::{PlanId, WorkBlockId};
        let cache = make_cache(WorkBlockId(1), HashMap::new(), 0, HashMap::new());
        let (sched_stale, row_stale) = super::compare_cache_is_stale(
            &cache,
            PlanId(99), // different plan
            &HashMap::new(),
            0,
            &HashMap::new(),
        );
        assert!(sched_stale, "changing plan_id must invalidate schedule");
        assert!(!row_stale);
    }

    #[test]
    fn cache_is_stale_on_block_duration_change() {
        use crate::model::{PlanId, WorkBlockId};
        let snap: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 5)].into_iter().collect();
        let cache = make_cache(WorkBlockId(42), snap, 0, HashMap::new());
        // Same plan, same dep_count, but duration changed.
        let new_snap: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 10)].into_iter().collect();
        let (sched_stale, _) =
            super::compare_cache_is_stale(&cache, PlanId(42), &new_snap, 0, &HashMap::new());
        assert!(sched_stale, "duration change must invalidate schedule");
    }

    #[test]
    fn cache_is_stale_on_dep_count_change() {
        use crate::model::{PlanId, WorkBlockId};
        let cache = make_cache(WorkBlockId(7), HashMap::new(), 2, HashMap::new());
        let (sched_stale, _) = super::compare_cache_is_stale(
            &cache,
            PlanId(7),
            &HashMap::new(),
            3, // one more dep
            &HashMap::new(),
        );
        assert!(sched_stale, "dep count change must invalidate schedule");
    }

    #[test]
    fn cache_is_stale_on_row_change() {
        use crate::model::{PlanId, WorkBlockId};
        let rows: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 0)].into_iter().collect();
        let cache = make_cache(WorkBlockId(5), HashMap::new(), 0, rows);
        let new_rows: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 1)].into_iter().collect();
        let (sched_stale, row_stale) =
            super::compare_cache_is_stale(&cache, PlanId(5), &HashMap::new(), 0, &new_rows);
        assert!(
            !sched_stale,
            "row change alone must not invalidate schedule"
        );
        assert!(row_stale, "row change must mark rows stale");
    }

    #[test]
    fn cache_is_not_stale_when_nothing_changed() {
        use crate::model::{PlanId, WorkBlockId};
        let snap: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 5)].into_iter().collect();
        let rows: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 2)].into_iter().collect();
        let cache = make_cache(WorkBlockId(3), snap.clone(), 1, rows.clone());
        let (sched_stale, row_stale) =
            super::compare_cache_is_stale(&cache, PlanId(3), &snap, 1, &rows);
        assert!(!sched_stale);
        assert!(!row_stale);
    }

    #[test]
    fn assign_compare_extra_rows_places_compare_only_blocks_after_max_row() {
        use crate::model::WorkBlockId;
        // Active plan has blocks at rows 2 and 5.
        let id_to_row: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 2), (WorkBlockId(2), 5)]
            .into_iter()
            .collect();
        // Compare schedule has block 1 (shared) and block 3 (compare-only).
        let extra =
            assign_compare_extra_rows(&id_to_row, [WorkBlockId(1), WorkBlockId(3)].into_iter());
        assert!(
            !extra.contains_key(&WorkBlockId(1)),
            "shared block must not appear in extra"
        );
        assert_eq!(
            extra[&WorkBlockId(3)],
            6,
            "compare-only block gets max_row+1"
        );
    }

    #[test]
    fn assign_compare_extra_rows_ordering_is_by_id_not_insertion() {
        use crate::model::WorkBlockId;
        // Empty active plan → max_row = 0, extra rows start at 1.
        let id_to_row: HashMap<WorkBlockId, i32> = HashMap::new();
        // IDs passed in reverse order; sort by id.0 must win.
        let extra = assign_compare_extra_rows(
            &id_to_row,
            [WorkBlockId(30), WorkBlockId(10), WorkBlockId(20)].into_iter(),
        );
        assert_eq!(extra[&WorkBlockId(10)], 1);
        assert_eq!(extra[&WorkBlockId(20)], 2);
        assert_eq!(extra[&WorkBlockId(30)], 3);
    }

    #[test]
    fn assign_compare_extra_rows_empty_compare_schedule_returns_empty() {
        use crate::model::WorkBlockId;
        let id_to_row: HashMap<WorkBlockId, i32> = [(WorkBlockId(1), 3)].into_iter().collect();
        let extra = assign_compare_extra_rows(&id_to_row, std::iter::empty());
        assert!(extra.is_empty());
    }

    // ── hdr_swatch_color ─────────────────────────────────────────────────────

    #[test]
    fn hdr_swatch_color_black_is_zero() {
        let c = hdr_swatch_color([0.0, 0.0, 0.0]);
        assert_eq!(c, egui::Color32::from_rgb(0, 0, 0));
    }

    #[test]
    fn hdr_swatch_color_white_is_255() {
        let c = hdr_swatch_color([1.0, 1.0, 1.0]);
        assert_eq!(c, egui::Color32::from_rgb(255, 255, 255));
    }

    #[test]
    fn hdr_swatch_color_mid_gray_matches_srgb_encoding() {
        // Linear 0.5 → sRGB ≈ 0.7354 → byte 188.
        let c = hdr_swatch_color([0.5, 0.5, 0.5]);
        let [r, g, b, _] = c.to_array();
        assert_eq!(r, g);
        assert_eq!(g, b);
        // Correct sRGB encoding of linear 0.5 is ~187–188.
        assert!((r as i32 - 188).abs() <= 1, "expected ~188, got {r}");
    }

    #[test]
    fn hdr_swatch_color_hdr_clamped_to_white() {
        // HDR values > 1.0 are clamped before encoding; all channels > 1.0 → white.
        let c = hdr_swatch_color([2.0, 1.5, 3.0]);
        assert_eq!(c, egui::Color32::from_rgb(255, 255, 255));
    }

    // ── block_color ───────────────────────────────────────────────────────────

    fn make_block_with_color(color: Option<[f32; 3]>) -> crate::model::WorkBlock {
        use crate::model::WorkBlockId;
        crate::model::WorkBlock {
            id: WorkBlockId(1),
            name: "test".to_string(),
            description: String::new(),
            start_day: 0,
            duration_days: 1,
            parent: None,
            priority: 0,
            t_shirt_size: None,
            rollup: false,
            color,
        }
    }

    #[test]
    fn block_color_uses_custom_color_when_set() {
        let wb = make_block_with_color(Some([2.0, 0.5, 1.0]));
        let c = block_color(&wb, 0);
        assert_eq!(c, bevy::color::LinearRgba::new(2.0, 0.5, 1.0, 1.0));
    }

    #[test]
    fn block_color_falls_back_to_palette_when_none() {
        let wb = make_block_with_color(None);
        let c = block_color(&wb, 0);
        assert_eq!(c, PALETTE[0]);
    }

    #[test]
    fn block_color_palette_cycles_with_row() {
        let wb = make_block_with_color(None);
        let len = PALETTE.len() as i32;
        for row in 0..len {
            assert_eq!(block_color(&wb, row), PALETTE[row as usize]);
        }
        // Wraps: row == len maps to PALETTE[0].
        assert_eq!(block_color(&wb, len), PALETTE[0]);
    }

    #[test]
    fn block_color_palette_wraps_negative_rows() {
        let wb = make_block_with_color(None);
        // rem_euclid maps -1 to PALETTE[len - 1], not a panic.
        let len = PALETTE.len();
        assert_eq!(block_color(&wb, -1), PALETTE[len - 1]);
    }

    #[test]
    fn block_color_none_after_reset_uses_palette() {
        // Simulate setting then clearing a custom color.
        let mut wb = make_block_with_color(Some([1.0, 0.0, 0.0]));
        wb.color = None;
        assert_eq!(block_color(&wb, 1), PALETTE[1]);
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
                    i.key_pressed(egui::Key::Enter) && !i.modifiers.ctrl && !i.modifiers.command
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
            let Some(plan_id) = model.main_plan_id() else {
                return;
            };
            let new_id = model.create_work_block(name);
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
            }
            if let Some(plan) = model.plans.get_mut(&plan_id) {
                plan.root_blocks.push(new_id);
            }
            model.set_block_row(plan_id, new_id, new_row);
            // A new block on main links through to existing branches as a ghost.
            model.link_main_block_to_branches(new_id);
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
/// Displays start day, end day, duration, and (if set) the block's
/// description. Renders an egui Area near the cursor.
pub fn draw_block_tooltip(
    mut egui_ctx: EguiContexts,
    model: Res<model::Model>,
    name_edit: Res<NameEditState>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    block_q: Query<(&BlockSprite, &Transform, &Sprite)>,
) {
    // Don't clutter an inline rename with the hover stats popup.
    if name_edit.editing.is_some() {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    if ctx.is_pointer_over_area() {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_transform)) = camera.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok(world_pos) = cam.viewport_to_world_2d(cam_transform, cursor_pos) else {
        return;
    };

    for (block_sprite, transform, sprite) in &block_q {
        if sprite_hit(transform, sprite, world_pos) {
            let Some(wb) = model.work_blocks.get(&block_sprite.work_block_id) else {
                continue;
            };
            let Some(screen_pos) = ctx.pointer_hover_pos() else {
                return;
            };
            let end_day = wb.start_day + wb.duration_days;
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
                                ui.label(format!("day {}", wb.start_day));
                                ui.end_row();
                                ui.label("End:");
                                ui.label(format!("day {}", end_day));
                                ui.end_row();
                                ui.label("Duration:");
                                ui.label(format!("{} days", wb.duration_days));
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

/// State for the editable size-map settings window: whether it is showing. The
/// window is launched from the block inspector fly-out's SIZE section and edits
/// the global `t_shirt_sizes` table.
#[derive(Resource, Default)]
pub struct SizePickerState {
    pub settings_open: bool,
}

/// Edit buffers backing the block inspector fly-out. `bound` is the block the
/// buffers currently mirror; when the selection changes the fly-out flushes the
/// old buffers to their block and reloads from the newly selected one, so
/// in-progress name/description text is never silently dropped.
#[derive(Resource, Default)]
pub struct BlockInspectorState {
    pub bound: Option<WorkBlockId>,
    pub name_buf: String,
    pub desc_buf: String,
}

/// Human-readable priority labels indexed by `WorkBlock::priority` (0..=3).
const PRIORITY_LABELS: [&str; 4] = ["Low", "Normal", "High", "Critical"];

/// A section heading inside the inspector fly-out: muted small-caps label over a
/// separator, matching the settings fly-out's sectioning.
fn inspector_section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(title)
            .size(11.0)
            .color(egui::Color32::from_rgb(150, 130, 96)),
    );
    ui.separator();
    ui.add_space(4.0);
}

/// Convert an HDR linear-RGB block color into a displayable egui swatch color.
/// Channels are clamped to [0, 1] and sRGB-encoded; the bloom-driven over-bright
/// values collapse to their base hue, which is enough to identify a swatch.
fn hdr_swatch_color(rgb: [f32; 3]) -> egui::Color32 {
    let enc = |c: f32| -> u8 {
        let c = c.clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    egui::Color32::from_rgb(enc(rgb[0]), enc(rgb[1]), enc(rgb[2]))
}

/// Flush the inspector's name/description buffers to `id`, saving only if either
/// actually changed. A blank name is ignored (a block must keep a name).
fn flush_inspector_buffers(
    model: &mut model::Model,
    id: WorkBlockId,
    name_buf: &str,
    desc_buf: &str,
    conn: &rusqlite::Connection,
) {
    let mut changed = false;
    if let Some(wb) = model.work_blocks.get_mut(&id) {
        let trimmed = name_buf.trim();
        if !trimmed.is_empty() && wb.name != trimmed {
            wb.name = trimmed.to_string();
            changed = true;
        }
        if wb.description != desc_buf {
            wb.description = desc_buf.to_string();
            changed = true;
        }
    }
    if changed {
        if let Err(e) = db::save_model(conn, model) {
            error!("save_model failed: {e}");
        }
    }
}

/// Right-side block inspector fly-out. Appears whenever a block is selected and
/// gathers all of its editable properties in one cohesive panel — name,
/// description, t-shirt size (write-through to `duration_days`), priority, and
/// color — instead of scattering them across pop-ups. Dismisses on the ✕ button,
/// on Escape, or when the block is deselected. Yields the right slot to the
/// settings fly-out while that is open.
#[allow(clippy::too_many_arguments)]
pub fn block_inspector_flyout_ui(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedBlock>,
    mut state: ResMut<BlockInspectorState>,
    mut picker: ResMut<SizePickerState>,
    mut model: ResMut<model::Model>,
    settings: Res<crate::SettingsState>,
    keys: Res<ButtonInput<KeyCode>>,
    conn: NonSend<rusqlite::Connection>,
) {
    // The settings fly-out owns the right slot while it is open.
    if settings.open {
        return;
    }
    let Some(id) = selected.0 else {
        state.bound = None;
        return;
    };
    // Selection points at a block that no longer exists (e.g. just deleted).
    if !model.work_blocks.contains_key(&id) {
        selected.0 = None;
        state.bound = None;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // (Re)bind the edit buffers when the selection changes, first flushing the
    // previously bound block's pending edits so nothing is lost on a quick switch.
    if state.bound != Some(id) {
        if let Some(prev) = state.bound {
            let (n, d) = (state.name_buf.clone(), state.desc_buf.clone());
            flush_inspector_buffers(&mut model, prev, &n, &d, &conn);
        }
        if let Some(wb) = model.work_blocks.get(&id) {
            state.name_buf = wb.name.clone();
            state.desc_buf = wb.description.clone();
        }
        state.bound = Some(id);
    }

    // Escape deselects — unless a text field is capturing the key, in which case
    // egui uses it to defocus the field first.
    if keys.just_pressed(KeyCode::Escape) && !ctx.wants_keyboard_input() {
        let (n, d) = (state.name_buf.clone(), state.desc_buf.clone());
        flush_inspector_buffers(&mut model, id, &n, &d, &conn);
        selected.0 = None;
        state.bound = None;
        return;
    }

    // Snapshot current values for highlighting; the panel closure only reads
    // these and the edit buffers, recording user intent in the locals below.
    let cur_priority = model
        .work_blocks
        .get(&id)
        .map(|wb| wb.priority)
        .unwrap_or(1);
    let cur_size = model
        .work_blocks
        .get(&id)
        .and_then(|wb| wb.t_shirt_size.clone());
    let cur_color = model.work_blocks.get(&id).and_then(|wb| wb.color);
    let duration_days = model
        .work_blocks
        .get(&id)
        .map(|wb| wb.duration_days)
        .unwrap_or(0);
    let sizes = model.t_shirt_sizes.clone();

    let mut commit_name = false;
    let mut commit_desc = false;
    let mut chosen_size: Option<(String, Day)> = None;
    let mut chosen_priority: Option<u8> = None;
    let mut chosen_color: Option<Option<[f32; 3]>> = None;
    let mut edit_sizes = false;
    let mut close = false;

    egui::SidePanel::right("block_inspector_flyout")
        .resizable(false)
        .exact_width(272.0)
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(26, 20, 12))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 46, 26)))
                .inner_margin(egui::Margin::same(14)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Block")
                        .size(16.0)
                        .strong()
                        .color(egui::Color32::from_rgb(238, 212, 152)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(egui::RichText::new("✕").size(14.0)).clicked() {
                        close = true;
                    }
                });
            });

            // ── Name ───────────────────────────────────────────────────────
            inspector_section(ui, "NAME");
            let resp = ui
                .add(egui::TextEdit::singleline(&mut state.name_buf).desired_width(f32::INFINITY));
            if resp.lost_focus() {
                commit_name = true;
            }

            // ── Description ────────────────────────────────────────────────
            inspector_section(ui, "DESCRIPTION");
            let resp = ui.add(
                egui::TextEdit::multiline(&mut state.desc_buf)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3)
                    .hint_text("Notes about this block"),
            );
            if resp.lost_focus() {
                commit_desc = true;
            }

            // ── Size ───────────────────────────────────────────────────────
            inspector_section(ui, "SIZE");
            ui.horizontal_wrapped(|ui| {
                for size in &sizes {
                    let is_cur = cur_size.as_deref() == Some(size.label.as_str());
                    let text = format!("{} · {}d", size.label, size.days);
                    if ui.selectable_label(is_cur, text).clicked() {
                        chosen_size = Some((size.label.clone(), size.days));
                    }
                }
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{duration_days} working days"))
                        .color(egui::Color32::from_rgb(150, 130, 96)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("⚙ Edit sizes…").clicked() {
                        edit_sizes = true;
                    }
                });
            });

            // ── Priority ───────────────────────────────────────────────────
            inspector_section(ui, "PRIORITY");
            ui.horizontal_wrapped(|ui| {
                for (i, label) in PRIORITY_LABELS.iter().enumerate() {
                    if ui
                        .selectable_label(cur_priority as usize == i, *label)
                        .clicked()
                    {
                        chosen_priority = Some(i as u8);
                    }
                }
            });

            // ── Color ──────────────────────────────────────────────────────
            inspector_section(ui, "COLOR");
            ui.horizontal_wrapped(|ui| {
                for swatch in PALETTE {
                    let [r, g, b, _] = swatch.to_f32_array();
                    let rgb = [r, g, b];
                    let is_cur = cur_color == Some(rgb);
                    let mut btn = egui::Button::new("")
                        .fill(hdr_swatch_color(rgb))
                        .min_size(egui::vec2(26.0, 22.0))
                        .corner_radius(egui::CornerRadius::same(4));
                    if is_cur {
                        btn = btn.stroke(egui::Stroke::new(2.0, egui::Color32::WHITE));
                    }
                    if ui.add(btn).clicked() {
                        chosen_color = Some(Some(rgb));
                    }
                }
            });
            ui.add_space(4.0);
            if cur_color.is_some() && ui.small_button("Reset to default").clicked() {
                chosen_color = Some(None);
            }
        });

    // Apply the recorded intent. Name/description go through the buffer flush
    // (which saves only on a real change); the discrete pickers mutate directly.
    if commit_name || commit_desc {
        let (n, d) = (state.name_buf.clone(), state.desc_buf.clone());
        flush_inspector_buffers(&mut model, id, &n, &d, &conn);
    }
    // Only take a mutable borrow when there is something to apply — otherwise
    // `get_mut` would trip `Model`'s change-detection every frame the fly-out is
    // open and force needless reschedules.
    if chosen_size.is_some() || chosen_priority.is_some() || chosen_color.is_some() {
        if let Some(wb) = model.work_blocks.get_mut(&id) {
            if let Some((label, days)) = chosen_size {
                wb.duration_days = days;
                wb.t_shirt_size = Some(label);
            }
            if let Some(p) = chosen_priority {
                wb.priority = p;
            }
            if let Some(c) = chosen_color {
                wb.color = c;
            }
        }
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
    if edit_sizes {
        picker.settings_open = true;
    }
    if close {
        let (n, d) = (state.name_buf.clone(), state.desc_buf.clone());
        flush_inspector_buffers(&mut model, id, &n, &d, &conn);
        selected.0 = None;
        state.bound = None;
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
                        .add(
                            egui::DragValue::new(&mut size.days)
                                .range(1..=400)
                                .suffix(" d"),
                        )
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
