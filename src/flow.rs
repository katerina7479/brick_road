//! The FLOW view (#326): a read-only Sankey-flavoured projection of one plan.
//!
//! One horizontal *stream* per top-level block, in the plan view's row order.
//! A stream's thickness at any day is its staffing depth: one unit per leaf
//! block active that day. Each leaf is a *ribbon* coloured by its resource, so
//! the stream visibly bulges where work piles up and the colours trace whose
//! time is flowing into it, day by day. Ribbons pack upward as neighbours
//! finish (first-fit levels, compacted per day-segment), which gives the
//! stepped Sankey look on the day grid.
//!
//! Layout is computed in pure day/level units (`flow_layout`, unit-tested);
//! the renderer maps to pixels with the calendar's holiday-aware `day_to_x`.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    constants::EVENTS_ROW,
    model::{self, Day, Model, WorkBlockId},
};

/// Height of one allocation unit (one concurrent leaf), in world pixels.
pub const FLOW_UNIT: f32 = 24.0;
/// Vertical gap between streams, in world pixels.
pub const FLOW_LANE_GAP: f32 = 18.0;

/// One stream: a top-level block and the vertical band it occupies.
#[derive(Debug, PartialEq)]
pub struct FlowLane {
    pub root: WorkBlockId,
    pub name: String,
    /// First allocation unit of this lane in the stacked layout (lane gaps
    /// are applied at render time from the lane index).
    pub base_unit: i32,
    /// Height in units — the stream's maximum staffing depth (≥ 1).
    pub height: i32,
}

/// One ribbon segment: `leaf` drawn at `level` within its lane for the
/// working-day span `[day0, day1)`. A leaf yields several runs when it packs
/// upward mid-flight (its level changes as neighbours finish).
#[derive(Debug, PartialEq)]
pub struct FlowRun {
    pub leaf: WorkBlockId,
    /// Resource the ribbon is coloured by (index into `FlowLayout::resources`).
    pub resource: usize,
    pub day0: Day,
    pub day1: Day,
    pub lane: usize,
    pub level: i32,
}

/// The computed flow layout for one plan.
#[derive(Debug, Default)]
pub struct FlowLayout {
    pub lanes: Vec<FlowLane>,
    pub runs: Vec<FlowRun>,
    /// Distinct resource names, sorted case-insensitively; a ribbon's colour
    /// is the palette entry at its resource's index here.
    pub resources: Vec<String>,
}

/// Cached flow layout, recomputed when the model or view changes.
#[derive(Resource, Default)]
pub struct FlowCache(pub FlowLayout);

/// Marker for a spawned ribbon sprite (rebuilt wholesale on cache change).
#[derive(Component)]
pub struct FlowRect;

/// Computes the flow layout for `plan_id`. Streams are the plan's top-level
/// blocks (Events-row targets excluded), ordered like the plan view: by row,
/// then start day, then id. Each stream's ribbons are its leaf descendants
/// (or the block itself when it is a leaf), packed per day-segment.
pub fn flow_layout(model: &Model, plan_id: model::PlanId) -> FlowLayout {
    let Some(plan) = model.plans.get(&plan_id) else {
        return FlowLayout::default();
    };

    // Streams in plan-view order.
    let mut roots: Vec<WorkBlockId> = plan
        .root_blocks
        .iter()
        .copied()
        .filter(|id| model.block_row(plan_id, *id) != EVENTS_ROW)
        .collect();
    roots.sort_by_key(|id| {
        let wb = model.work_blocks.get(id);
        (
            model.block_row(plan_id, *id),
            wb.map(|w| w.start_day).unwrap_or(0),
            id.0,
        )
    });

    // Leaves per stream, with their resource names.
    struct Leaf {
        id: WorkBlockId,
        start: Day,
        end: Day,
        resource: String,
    }
    let leaves_of = |root: WorkBlockId| -> Vec<Leaf> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let children = model.children(id);
            if children.is_empty() {
                let Some(wb) = model.work_blocks.get(&id) else {
                    continue;
                };
                if wb.duration_days <= 0 {
                    continue;
                }
                out.push(Leaf {
                    id,
                    start: wb.start_day,
                    end: wb.start_day + wb.duration_days,
                    resource: model.leaf_resource_name(plan_id, id, wb.parent),
                });
            } else {
                stack.extend(children);
            }
        }
        out
    };

    let per_root: Vec<(WorkBlockId, Vec<Leaf>)> = roots
        .into_iter()
        .map(|r| (r, leaves_of(r)))
        .filter(|(_, leaves)| !leaves.is_empty())
        .collect();

    // Global resource index (colour source), sorted for stability.
    let mut resources: Vec<String> = per_root
        .iter()
        .flat_map(|(_, leaves)| leaves.iter().map(|l| l.resource.clone()))
        .collect();
    resources.sort_by_key(|n| n.to_lowercase());
    resources.dedup();
    let resource_idx: HashMap<&str, usize> = resources
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    let mut lanes = Vec::new();
    let mut runs = Vec::new();
    let mut base_unit = 0;
    for (lane_idx, (root, leaves)) in per_root.iter().enumerate() {
        // Stable slot per leaf, then per-segment compaction: between two
        // consecutive start/end boundaries the active set is constant, and
        // each active leaf's level is its rank by slot — so ribbons pack
        // upward the moment a neighbour finishes.
        let slots: HashMap<WorkBlockId, i32> =
            model::assign_sublanes(leaves.iter().map(|l| (l.id, l.start, l.end)).collect())
                .into_iter()
                .collect();
        let mut bounds: Vec<Day> = leaves.iter().flat_map(|l| [l.start, l.end]).collect();
        bounds.sort_unstable();
        bounds.dedup();

        let mut height = 1;
        // Open run per leaf: (level, day the run started).
        let mut open: HashMap<WorkBlockId, (i32, Day)> = HashMap::new();
        for w in bounds.windows(2) {
            let (seg0, seg1) = (w[0], w[1]);
            let mut active: Vec<&Leaf> = leaves
                .iter()
                .filter(|l| l.start <= seg0 && seg1 <= l.end)
                .collect();
            active.sort_by_key(|l| (slots.get(&l.id).copied().unwrap_or(0), l.id.0));
            height = height.max(active.len() as i32);

            // Close runs for leaves that left or changed level.
            let levels: HashMap<WorkBlockId, i32> = active
                .iter()
                .enumerate()
                .map(|(i, l)| (l.id, i as i32))
                .collect();
            open.retain(|id, (level, started)| {
                if levels.get(id) == Some(level) {
                    return true;
                }
                runs.push(FlowRun {
                    leaf: *id,
                    resource: 0, // patched below from the leaf table
                    day0: *started,
                    day1: seg0,
                    lane: lane_idx,
                    level: *level,
                });
                false
            });
            // Open runs for newly placed leaves.
            for l in &active {
                let level = levels[&l.id];
                open.entry(l.id).or_insert((level, seg0));
            }
        }
        let last = bounds.last().copied().unwrap_or(0);
        for (id, (level, started)) in open {
            runs.push(FlowRun {
                leaf: id,
                resource: 0,
                day0: started,
                day1: last,
                lane: lane_idx,
                level,
            });
        }

        let name = model
            .work_blocks
            .get(root)
            .map(|wb| wb.name.clone())
            .unwrap_or_default();
        lanes.push(FlowLane {
            root: *root,
            name,
            base_unit,
            height,
        });
        base_unit += height;
    }

    // Patch each run's resource index from its leaf.
    let leaf_resource: HashMap<WorkBlockId, usize> = per_root
        .iter()
        .flat_map(|(_, leaves)| leaves.iter())
        .map(|l| (l.id, resource_idx[l.resource.as_str()]))
        .collect();
    for run in &mut runs {
        run.resource = leaf_resource.get(&run.leaf).copied().unwrap_or(0);
    }

    FlowLayout {
        lanes,
        runs,
        resources,
    }
}

/// World-Y of the centre of `level` within `lane` (lane gaps by index).
pub fn flow_level_y(lanes: &[FlowLane], lane: usize, level: i32) -> f32 {
    let base = lanes[lane].base_unit as f32 * FLOW_UNIT + lane as f32 * FLOW_LANE_GAP;
    -(base + (level as f32 + 0.5) * FLOW_UNIT)
}

/// Recomputes `FlowCache` when the model or view changes (flow view only).
pub fn update_flow_view(
    model: Res<Model>,
    view: Res<crate::ViewMode>,
    mut cache: ResMut<FlowCache>,
) {
    if !model.is_changed() && !view.is_changed() {
        return;
    }
    if view.kind != crate::ViewKind::Flow {
        return;
    }
    let plan_id = view.plan.or_else(|| model.main_plan_id());
    cache.0 = plan_id.map(|p| flow_layout(&model, p)).unwrap_or_default();
}

/// Rebuilds the ribbon sprites from `FlowCache`: despawn-all + respawn on
/// cache or view change (the view is read-only, so wholesale rebuild is
/// simpler than reconciliation and never runs during a drag).
pub fn sync_flow_sprites(
    mut commands: Commands,
    model: Res<Model>,
    view: Res<crate::ViewMode>,
    cache: Res<FlowCache>,
    existing: Query<Entity, With<FlowRect>>,
) {
    if !cache.is_changed() && !view.is_changed() {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    if view.kind != crate::ViewKind::Flow {
        return;
    }

    let off = model.calendar.global_off_days();
    let layout = &cache.0;
    for run in &layout.runs {
        let x0 = crate::calendar::day_to_x(run.day0, &off, &model.calendar);
        let x1 = crate::calendar::day_to_x(run.day1, &off, &model.calendar);
        let width = (x1 - x0 - 2.0).max(2.0);
        let y = flow_level_y(&layout.lanes, run.lane, run.level);
        let base = crate::blocks::PALETTE[run.resource % crate::blocks::PALETTE.len()];
        let mut cmd = commands.spawn((
            FlowRect,
            Sprite {
                color: Color::from(LinearRgba::new(base.red, base.green, base.blue, 0.92)),
                custom_size: Some(Vec2::new(width, FLOW_UNIT - 3.0)),
                ..default()
            },
            Transform::from_xyz(x0 + 1.0 + width * 0.5, y, 0.0),
        ));
        // Ribbon label: the resource whose time this slice is, when it fits.
        let label = layout
            .resources
            .get(run.resource)
            .cloned()
            .unwrap_or_default();
        if width >= label.chars().count() as f32 * 8.0 + 10.0 {
            cmd.with_children(|parent| {
                parent.spawn((
                    Text2d::new(label),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(Color::srgba(0.08, 0.09, 0.12, 1.0)),
                    bevy::sprite::Anchor::CENTER,
                    Transform::from_xyz(0.0, 0.0, 0.15),
                ));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(m: &mut Model, plan: model::PlanId, name: &str, start: Day, dur: Day) -> WorkBlockId {
        m.add_block_to_plan(plan, name, start, dur, 0)
    }

    #[test]
    fn single_leaf_root_is_one_ribbon() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let a = placed(&mut m, plan, "A", 2, 5);
        let fl = flow_layout(&m, plan);
        assert_eq!(fl.lanes.len(), 1);
        assert_eq!(fl.lanes[0].root, a);
        assert_eq!(fl.lanes[0].height, 1);
        assert_eq!(fl.runs.len(), 1);
        let r = &fl.runs[0];
        assert_eq!((r.day0, r.day1, r.lane, r.level), (2, 7, 0, 0));
        // Unnamed row 0 groups under its placeholder resource.
        assert_eq!(fl.resources[r.resource], "Resource 1");
    }

    #[test]
    fn overlapping_children_thicken_the_stream() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let root = placed(&mut m, plan, "P1", 0, 10);
        let a = m.add_child_block(plan, root, "a", 0, 10, 0);
        let b = m.add_child_block(plan, root, "b", 2, 4, 1);
        let fl = flow_layout(&m, plan);
        assert_eq!(fl.lanes.len(), 1, "the container is the stream, not a leaf");
        assert_eq!(fl.lanes[0].height, 2, "two concurrent leaves = 2 units");
        let runs_of = |id: WorkBlockId| -> Vec<(Day, Day, i32)> {
            let mut v: Vec<_> = fl
                .runs
                .iter()
                .filter(|r| r.leaf == id)
                .map(|r| (r.day0, r.day1, r.level))
                .collect();
            v.sort();
            v
        };
        assert_eq!(runs_of(a), vec![(0, 10, 0)]);
        assert_eq!(runs_of(b), vec![(2, 6, 1)]);
    }

    #[test]
    fn ribbon_packs_upward_when_neighbour_finishes() {
        // A [0,4) holds level 0; B [2,8) starts below it, then packs up to
        // level 0 when A ends — two runs for B, the stepped Sankey look.
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let root = placed(&mut m, plan, "P1", 0, 8);
        let _a = m.add_child_block(plan, root, "a", 0, 4, 0);
        let b = m.add_child_block(plan, root, "b", 2, 6, 1);
        let fl = flow_layout(&m, plan);
        let mut b_runs: Vec<_> = fl
            .runs
            .iter()
            .filter(|r| r.leaf == b)
            .map(|r| (r.day0, r.day1, r.level))
            .collect();
        b_runs.sort();
        assert_eq!(b_runs, vec![(2, 4, 1), (4, 8, 0)]);
    }

    #[test]
    fn lanes_stack_in_plan_row_order_and_events_are_excluded() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let p2 = m.add_block_to_plan(plan, "P2", 0, 5, 1);
        let p1 = m.add_block_to_plan(plan, "P1", 0, 5, 0);
        let _ev = m.add_block_to_plan(plan, "GA", 3, 1, EVENTS_ROW);
        // P1's two concurrent leaves make its lane 2 units tall.
        let _c1 = m.add_child_block(plan, p1, "c1", 0, 5, 0);
        let _c2 = m.add_child_block(plan, p1, "c2", 0, 5, 1);
        let fl = flow_layout(&m, plan);
        assert_eq!(fl.lanes.len(), 2, "the event is not a stream");
        assert_eq!(fl.lanes[0].root, p1, "row 0 stream first");
        assert_eq!(fl.lanes[0].base_unit, 0);
        assert_eq!(fl.lanes[0].height, 2);
        assert_eq!(fl.lanes[1].root, p2);
        assert_eq!(
            fl.lanes[1].base_unit, 2,
            "later streams start below the previous stream's units"
        );
    }

    #[test]
    fn ribbon_colours_index_sorted_resources() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let root = placed(&mut m, plan, "P1", 0, 8);
        m.set_resource_kind("Zara", model::ResourceType::Engineer);
        m.set_resource_kind("Alice", model::ResourceType::Engineer);
        let a = m.add_child_block(plan, root, "a", 0, 4, 0);
        let b = m.add_child_block(plan, root, "b", 4, 4, 1);
        {
            let p = m.plans.get_mut(&plan).unwrap();
            p.set_row_name(Some(root), 0, "Zara".to_string());
            p.set_row_name(Some(root), 1, "Alice".to_string());
        }
        let fl = flow_layout(&m, plan);
        assert_eq!(fl.resources, vec!["Alice".to_string(), "Zara".to_string()]);
        let res_of = |id: WorkBlockId| {
            fl.runs
                .iter()
                .find(|r| r.leaf == id)
                .map(|r| fl.resources[r.resource].clone())
        };
        assert_eq!(res_of(a).as_deref(), Some("Zara"));
        assert_eq!(res_of(b).as_deref(), Some("Alice"));
    }
}
