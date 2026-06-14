use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::model::{
    DependencyId, DependencyType, Model, Plan, ResourceBlock, ResourceBlockId, WorkBlockId,
};

/// A single dependency whose constraint is not satisfied by the current
/// `WorkBlock` placements (`start_day` / `duration_days`).
#[derive(Debug, Clone, PartialEq)]
pub struct DependencyViolation {
    pub dependency_id: DependencyId,
    pub predecessor: WorkBlockId,
    pub successor: WorkBlockId,
    pub dependency_type: DependencyType,
    pub lag: f32,
    /// Days by which the constraint is violated (always > 0 when present).
    pub violation_days: f32,
}

/// A time window in which total resource demand exceeds available capacity.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceConflict {
    pub resource_id: ResourceBlockId,
    pub window_start: f32,
    pub window_end: f32,
    /// Sum of `allocation_factor` for all blocks active in this window.
    pub demand: f32,
    /// Available capacity at this time (from `AvailabilityTimeline`, or 1.0 if none).
    pub capacity: f32,
    /// `demand − capacity`, always > 0.
    pub overload: f32,
    /// Every block that is allocated to this resource during this window.
    pub contributing_blocks: Vec<WorkBlockId>,
}

/// All analysis results computed from the current model/plan state.
#[derive(Debug, Clone, Default, PartialEq, Resource)]
pub struct ScheduleAnalysis {
    pub violations: Vec<DependencyViolation>,
    pub resource_conflicts: Vec<ResourceConflict>,
    /// Zero-float blocks in topological order (from user placement backward pass).
    pub critical_path: Vec<WorkBlockId>,
    /// Total float per block (latest_finish − earliest_finish over user placement).
    pub float: HashMap<WorkBlockId, f32>,
}

/// Check every dependency in `model` against the current user-placed
/// `start_day` / `duration_days` on each `WorkBlock`.
///
/// Constraint semantics (P = predecessor, S = successor, lag in days):
///   FS:  S.start     ≥ P.end   + lag
///   SS:  S.start     ≥ P.start + lag
///   FF:  S.end       ≥ P.end   + lag
///   SF:  S.end       ≥ P.start + lag
///
/// A violation occurs when the required bound exceeds the placed value;
/// `violation_days` is the magnitude of the shortfall.
pub fn analyze_dependencies(model: &Model) -> ScheduleAnalysis {
    let mut violations = Vec::new();

    for (&dep_id, dep) in &model.dependencies {
        let Some(pred) = model.work_blocks.get(&dep.predecessor) else {
            continue;
        };
        let Some(succ) = model.work_blocks.get(&dep.successor) else {
            continue;
        };

        let pred_end = pred.start_day + pred.duration_days;
        let succ_end = succ.start_day + succ.duration_days;
        let lag = dep.lag;

        let violation_days = match dep.dependency_type {
            DependencyType::FinishToStart => pred_end + lag - succ.start_day,
            DependencyType::StartToStart => pred.start_day + lag - succ.start_day,
            DependencyType::FinishToFinish => pred_end + lag - succ_end,
            DependencyType::StartToFinish => pred.start_day + lag - succ_end,
        };

        if violation_days > 0.0 {
            violations.push(DependencyViolation {
                dependency_id: dep_id,
                predecessor: dep.predecessor,
                successor: dep.successor,
                dependency_type: dep.dependency_type,
                lag,
                violation_days,
            });
        }
    }

    ScheduleAnalysis {
        violations,
        resource_conflicts: vec![],
        critical_path: vec![],
        float: HashMap::new(),
    }
}

/// Detect time windows where allocated resource demand exceeds capacity for
/// the given `plan`, based on current `WorkBlock.start_day` / `duration_days`
/// placements.
///
/// Uses a sweep-line over every event point where demand or capacity can
/// change (block starts/ends and availability-segment boundaries), sampling
/// the midpoint of each sub-interval. Sub-intervals with zero demand are
/// skipped. Capacity defaults to 1.0 (full, unconstrained) for any instant
/// not covered by an `AvailabilitySegment`.
pub fn analyze_resources(model: &Model, plan: &Plan) -> Vec<ResourceConflict> {
    // Group allocations by resource.
    let mut by_resource: HashMap<ResourceBlockId, Vec<(WorkBlockId, f32)>> = HashMap::new();
    for alloc in &plan.allocations {
        by_resource
            .entry(alloc.resource_id)
            .or_default()
            .push((alloc.work_block_id, alloc.allocation_factor));
    }

    let mut conflicts = Vec::new();

    for (rb_id, allocs) in &by_resource {
        let rb = model.resource_blocks.get(rb_id);

        // Collect all event points: block starts/ends + availability boundaries.
        let mut events: Vec<f32> = Vec::new();
        for &(wb_id, _) in allocs {
            if let Some(wb) = model.work_blocks.get(&wb_id) {
                if wb.duration_days > 0.0 {
                    events.push(wb.start_day);
                    events.push(wb.start_day + wb.duration_days);
                }
            }
        }
        if let Some(rb) = rb {
            for seg in &rb.availability.segments {
                events.push(seg.start);
                events.push(seg.end);
            }
        }

        events.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        events.dedup_by(|a, b| (*a - *b).abs() < 1e-9);

        for w in events.windows(2) {
            let (t0, t1) = (w[0], w[1]);
            if t1 <= t0 {
                continue;
            }
            let mid = (t0 + t1) * 0.5;

            let mut demand = 0.0_f32;
            let mut contributing = Vec::new();
            for &(wb_id, factor) in allocs {
                if let Some(wb) = model.work_blocks.get(&wb_id) {
                    let end = wb.start_day + wb.duration_days;
                    if wb.start_day <= mid && mid < end {
                        demand += factor;
                        contributing.push(wb_id);
                    }
                }
            }

            if demand <= 0.0 {
                continue;
            }

            let capacity = rb.map(|r| avail_at(r, mid)).unwrap_or(1.0);

            if demand > capacity + 1e-6 {
                conflicts.push(ResourceConflict {
                    resource_id: *rb_id,
                    window_start: t0,
                    window_end: t1,
                    demand,
                    capacity,
                    overload: demand - capacity,
                    contributing_blocks: contributing,
                });
            }
        }
    }

    conflicts
}

/// Availability factor for resource `rb` at instant `t`.
/// Gaps in the availability timeline are treated as factor 1.0.
fn avail_at(rb: &ResourceBlock, t: f32) -> f32 {
    for seg in &rb.availability.segments {
        if seg.start <= t && t < seg.end {
            return seg.factor;
        }
    }
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AvailabilitySegment, AvailabilityTimeline, Estimate, Model, ResourceAllocation,
        ResourceType,
    };

    fn est(d: f32) -> Estimate {
        Estimate { most_likely: d, optimistic: d, pessimistic: d, confidence: 1.0 }
    }

    fn placed(model: &mut Model, name: &str, start: f32, dur: f32) -> WorkBlockId {
        let id = model.create_work_block(name, est(dur));
        let wb = model.work_blocks.get_mut(&id).unwrap();
        wb.start_day = start;
        wb.duration_days = dur;
        id
    }

    fn make_plan(model: &mut Model, allocs: Vec<ResourceAllocation>) -> crate::model::PlanId {
        let wid = model.create_world("w");
        let pid = model.create_plan("p", wid);
        model.plans.get_mut(&pid).unwrap().allocations = allocs;
        pid
    }

    // ── analyze_dependencies tests ──────────────────────────────────────────

    #[test]
    fn fs_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0);
        let b = placed(&mut m, "B", 5.0, 3.0);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn fs_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0);
        let b = placed(&mut m, "B", 3.0, 3.0);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].predecessor, a);
        assert_eq!(v[0].successor, b);
        assert!((v[0].violation_days - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn fs_with_lag_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 3.0);
        let b = placed(&mut m, "B", 4.0, 2.0);
        let dep_id = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep_id).unwrap().lag = 2.0;
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert!((v[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn ss_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 2.0, 4.0);
        let b = placed(&mut m, "B", 2.0, 3.0);
        m.create_dependency(a, b, DependencyType::StartToStart);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn ss_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 3.0, 4.0);
        let b = placed(&mut m, "B", 2.0, 3.0);
        m.create_dependency(a, b, DependencyType::StartToStart);
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert!((v[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn ff_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0);
        let b = placed(&mut m, "B", 1.0, 4.0);
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn ff_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 6.0);
        let b = placed(&mut m, "B", 1.0, 4.0);
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert!((v[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn sf_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 4.0, 2.0);
        let b = placed(&mut m, "B", 0.0, 5.0);
        m.create_dependency(a, b, DependencyType::StartToFinish);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn sf_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 5.0, 2.0);
        let b = placed(&mut m, "B", 0.0, 4.0);
        m.create_dependency(a, b, DependencyType::StartToFinish);
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert!((v[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn no_violations_clean_schedule() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0);
        let b = placed(&mut m, "B", 5.0, 8.0);
        let c = placed(&mut m, "C", 13.0, 4.0);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn missing_block_skipped() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 3.0);
        let b = placed(&mut m, "B", 5.0, 2.0);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.work_blocks.remove(&b);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    // ── analyze_resources tests ─────────────────────────────────────────────

    fn alloc(rb: ResourceBlockId, wb: WorkBlockId, factor: f32) -> ResourceAllocation {
        ResourceAllocation { resource_id: rb, work_block_id: wb, allocation_factor: factor }
    }

    #[test]
    fn no_allocations_no_conflicts() {
        let mut m = Model::default();
        let pid = make_plan(&mut m, vec![]);
        let plan = m.plans[&pid].clone();
        assert!(analyze_resources(&m, &plan).is_empty());
    }

    #[test]
    fn serialized_blocks_no_conflict() {
        // A [0,3) then B [3,6) on R — no overlap, no conflict.
        let mut m = Model::default();
        let wid = m.create_world("w");
        let r = m.create_resource_block("R", ResourceType::Person);
        m.worlds.get_mut(&wid).unwrap().resource_ids.push(r);
        let a = placed(&mut m, "A", 0.0, 3.0);
        let b = placed(&mut m, "B", 3.0, 3.0);
        let pid = make_plan(&mut m, vec![alloc(r, a, 1.0), alloc(r, b, 1.0)]);
        let plan = m.plans[&pid].clone();
        assert!(analyze_resources(&m, &plan).is_empty());
    }

    #[test]
    fn overlapping_full_blocks_conflict() {
        // A [0,5) and B [2,8) both at factor 1.0 → demand 2.0 in [2,5).
        let mut m = Model::default();
        let wid = m.create_world("w");
        let r = m.create_resource_block("R", ResourceType::Person);
        m.worlds.get_mut(&wid).unwrap().resource_ids.push(r);
        let a = placed(&mut m, "A", 0.0, 5.0);
        let b = placed(&mut m, "B", 2.0, 6.0);
        let pid = make_plan(&mut m, vec![alloc(r, a, 1.0), alloc(r, b, 1.0)]);
        let plan = m.plans[&pid].clone();
        let cs = analyze_resources(&m, &plan);
        assert!(!cs.is_empty(), "expected a conflict");
        let c = cs.iter().find(|c| c.resource_id == r).unwrap();
        assert!((c.demand - 2.0).abs() < 1e-5);
        assert!((c.capacity - 1.0).abs() < 1e-5);
        assert!((c.overload - 1.0).abs() < 1e-5);
        assert!(c.contributing_blocks.contains(&a));
        assert!(c.contributing_blocks.contains(&b));
        // Non-overlapping windows should NOT appear as conflicts.
        assert!(!cs.iter().any(|c| c.window_end <= 2.0));
        assert!(!cs.iter().any(|c| c.window_start >= 5.0));
    }

    #[test]
    fn partial_allocations_sum_under_capacity_no_conflict() {
        // Two blocks at 0.5 each, fully overlapping → demand 1.0 = capacity.
        let mut m = Model::default();
        let wid = m.create_world("w");
        let r = m.create_resource_block("R", ResourceType::Person);
        m.worlds.get_mut(&wid).unwrap().resource_ids.push(r);
        let a = placed(&mut m, "A", 0.0, 4.0);
        let b = placed(&mut m, "B", 0.0, 4.0);
        let pid = make_plan(&mut m, vec![alloc(r, a, 0.5), alloc(r, b, 0.5)]);
        let plan = m.plans[&pid].clone();
        assert!(analyze_resources(&m, &plan).is_empty());
    }

    #[test]
    fn partial_allocations_exceed_capacity_conflict() {
        // Two blocks at 0.6 each, overlapping → demand 1.2 > 1.0.
        let mut m = Model::default();
        let wid = m.create_world("w");
        let r = m.create_resource_block("R", ResourceType::Person);
        m.worlds.get_mut(&wid).unwrap().resource_ids.push(r);
        let a = placed(&mut m, "A", 0.0, 4.0);
        let b = placed(&mut m, "B", 0.0, 4.0);
        let pid = make_plan(&mut m, vec![alloc(r, a, 0.6), alloc(r, b, 0.6)]);
        let plan = m.plans[&pid].clone();
        let cs = analyze_resources(&m, &plan);
        assert!(!cs.is_empty());
        assert!((cs[0].demand - 1.2).abs() < 1e-5);
    }

    #[test]
    fn reduced_availability_causes_conflict() {
        // R available at factor 0.5; one block allocated at 1.0 → overload.
        let mut m = Model::default();
        let wid = m.create_world("w");
        let r = m.create_resource_block("R", ResourceType::Person);
        m.worlds.get_mut(&wid).unwrap().resource_ids.push(r);
        m.resource_blocks.get_mut(&r).unwrap().availability = AvailabilityTimeline {
            segments: vec![AvailabilitySegment { start: 0.0, end: 10.0, factor: 0.5 }],
        };
        let a = placed(&mut m, "A", 0.0, 5.0);
        let pid = make_plan(&mut m, vec![alloc(r, a, 1.0)]);
        let plan = m.plans[&pid].clone();
        let cs = analyze_resources(&m, &plan);
        assert!(!cs.is_empty());
        assert!((cs[0].capacity - 0.5).abs() < 1e-5);
        assert!((cs[0].overload - 0.5).abs() < 1e-5);
    }
}
