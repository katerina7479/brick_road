use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::graph::{CycleError, DependencyGraph};
use crate::model::{DependencyType, Model, Plan, PlanId, ResourceBlock, ResourceBlockId, WorkBlockId};

/// The computed time placement of one work block.
#[derive(Debug, Clone)]
pub struct ScheduledBlock {
    pub work_block_id: WorkBlockId,
    pub start_day: f32,
    pub end_day: f32,
    /// Convenience: end_day - start_day.
    pub duration_days: f32,
}

/// The full output of a scheduler run over a Plan.
#[derive(Debug, Clone, Resource)]
pub struct Schedule {
    pub plan_id: PlanId,
    /// Placement for every block that was scheduled.
    pub blocks: HashMap<WorkBlockId, ScheduledBlock>,
    /// Day on which the last block finishes.
    pub total_duration_days: f32,
    /// Ordered sequence of block IDs on the critical path (longest path).
    pub critical_path: Vec<WorkBlockId>,
}

impl Schedule {
    pub fn new(plan_id: PlanId) -> Self {
        Self {
            plan_id,
            blocks: HashMap::new(),
            total_duration_days: 0.0,
            critical_path: vec![],
        }
    }
}

/// Compute unconstrained earliest start/end for every active block (Demand
/// Planning mode, PRD §6.1). Uses most-likely estimates; no resource
/// constraints are applied.
///
/// Dependency semantics (P = predecessor, S = successor, lag in days):
///   FS:  start(S) ≥ end(P)   + lag
///   SS:  start(S) ≥ start(P) + lag
///   FF:    end(S) ≥ end(P)   + lag
///   SF:    end(S) ≥ start(P) + lag
///
/// Returns `Err(CycleError)` if the dependency graph contains a cycle.
pub fn forward_pass(
    model: &Model,
    plan: &Plan,
    graph: &DependencyGraph,
) -> Result<Schedule, CycleError> {
    let order = crate::graph::topological_sort(graph)?;

    // Lower bound on start day from FS/SS edges.
    let mut min_start: HashMap<WorkBlockId, f32> =
        graph.nodes.iter().map(|&id| (id, 0.0_f32)).collect();
    // Lower bound on end day from FF/SF edges.
    let mut min_end: HashMap<WorkBlockId, Option<f32>> =
        graph.nodes.iter().map(|&id| (id, None)).collect();
    // Which predecessor drove the tightest constraint for each block
    // (used to reconstruct the critical path).
    let mut driver: HashMap<WorkBlockId, Option<WorkBlockId>> =
        graph.nodes.iter().map(|&id| (id, None)).collect();

    let mut sched = Schedule::new(plan.id);

    for &id in &order {
        let dur = model
            .work_blocks
            .get(&id)
            .map(|wb| wb.estimate.most_likely)
            .unwrap_or(0.0);

        let es_from_start = *min_start.get(&id).unwrap_or(&0.0);
        let es_from_end = min_end
            .get(&id)
            .and_then(|v| *v)
            .map(|me| me - dur)
            .unwrap_or(0.0_f32);

        let earliest_start = f32::max(0.0, f32::max(es_from_start, es_from_end));
        let earliest_end = earliest_start + dur;

        // Propagate constraints to successors, tracking which predecessor
        // produced each tightest constraint.
        if let Some(edges) = graph.edges.get(&id) {
            for edge in edges {
                let s = edge.successor;
                match edge.dependency_type {
                    DependencyType::FinishToStart => {
                        let new = earliest_end + edge.lag;
                        let v = min_start.entry(s).or_insert(0.0);
                        if new > *v {
                            *v = new;
                            *driver.entry(s).or_insert(None) = Some(id);
                        }
                    }
                    DependencyType::StartToStart => {
                        let new = earliest_start + edge.lag;
                        let v = min_start.entry(s).or_insert(0.0);
                        if new > *v {
                            *v = new;
                            *driver.entry(s).or_insert(None) = Some(id);
                        }
                    }
                    DependencyType::FinishToFinish => {
                        let new = earliest_end + edge.lag;
                        let v = min_end.entry(s).or_insert(None);
                        let update = v.map_or(true, |cur| new > cur);
                        if update {
                            *v = Some(new);
                            *driver.entry(s).or_insert(None) = Some(id);
                        }
                    }
                    DependencyType::StartToFinish => {
                        let new = earliest_start + edge.lag;
                        let v = min_end.entry(s).or_insert(None);
                        let update = v.map_or(true, |cur| new > cur);
                        if update {
                            *v = Some(new);
                            *driver.entry(s).or_insert(None) = Some(id);
                        }
                    }
                }
            }
        }

        sched.blocks.insert(id, ScheduledBlock {
            work_block_id: id,
            start_day: earliest_start,
            end_day: earliest_end,
            duration_days: dur,
        });
    }

    sched.total_duration_days = sched
        .blocks
        .values()
        .map(|b| b.end_day)
        .fold(0.0_f32, f32::max);

    sched.critical_path = build_critical_path(&sched.blocks, &driver, sched.total_duration_days);

    Ok(sched)
}

/// Trace the critical path backward from the block(s) that end at
/// `total_duration`, following driver links, then reverse for topo order.
fn build_critical_path(
    blocks: &HashMap<WorkBlockId, ScheduledBlock>,
    driver: &HashMap<WorkBlockId, Option<WorkBlockId>>,
    total_duration: f32,
) -> Vec<WorkBlockId> {
    // Find the terminal block on the critical path (latest end; lowest id breaks ties).
    let terminal = blocks
        .values()
        .filter(|b| (b.end_day - total_duration).abs() < f32::EPSILON)
        .min_by_key(|b| b.work_block_id.0)
        .map(|b| b.work_block_id);

    let Some(mut cur) = terminal else { return vec![] };

    let mut path = vec![cur];
    while let Some(&Some(pred)) = driver.get(&cur) {
        path.push(pred);
        cur = pred;
    }
    path.reverse();
    path
}

/// Schedule every active block while respecting both dependency constraints
/// and resource capacity limits (Execution Planning mode, PRD §6.2).
///
/// Blocks are processed in topological order. For each block the algorithm:
///   1. Computes the dependency-constrained earliest start (same as `forward_pass`).
///   2. Finds the earliest time ≥ that start where every required resource has
///      sufficient remaining capacity for the full duration of the block.
///   3. Commits that window to the resource calendars.
///
/// Resource demand comes from `plan.allocations`; capacity comes from
/// `ResourceBlock::availability` (gaps in the timeline are treated as factor 1.0).
/// Uses most-likely estimates.
///
/// Returns `Err(CycleError)` if the dependency graph contains a cycle.
pub fn resource_leveled_pass(
    model: &Model,
    plan: &Plan,
    graph: &DependencyGraph,
) -> Result<Schedule, CycleError> {
    let order = crate::graph::topological_sort(graph)?;

    // Dependency constraints — updated as blocks are placed (with actual times).
    let mut dep_min_start: HashMap<WorkBlockId, f32> =
        graph.nodes.iter().map(|&id| (id, 0.0_f32)).collect();
    let mut dep_min_end: HashMap<WorkBlockId, Option<f32>> =
        graph.nodes.iter().map(|&id| (id, None)).collect();

    // Committed resource windows: resource_id → [(start, end, demand)].
    let mut committed: HashMap<ResourceBlockId, Vec<(f32, f32, f32)>> = HashMap::new();

    // Block → [(resource_id, allocation_factor)] from plan.allocations.
    let mut block_demands: HashMap<WorkBlockId, Vec<(ResourceBlockId, f32)>> = HashMap::new();
    for alloc in &plan.allocations {
        block_demands
            .entry(alloc.work_block_id)
            .or_default()
            .push((alloc.resource_id, alloc.allocation_factor));
    }

    let resource_blocks: HashMap<ResourceBlockId, &ResourceBlock> =
        model.resource_blocks.iter().map(|(&id, rb)| (id, rb)).collect();

    let mut sched = Schedule::new(plan.id);

    for &id in &order {
        let dur = model
            .work_blocks
            .get(&id)
            .map(|wb| wb.estimate.most_likely)
            .unwrap_or(0.0);

        // Step 1: dependency-constrained minimum start.
        let dep_start = {
            let from_start = *dep_min_start.get(&id).unwrap_or(&0.0);
            let from_end = dep_min_end
                .get(&id)
                .and_then(|v| *v)
                .map(|me| me - dur)
                .unwrap_or(0.0);
            f32::max(0.0, f32::max(from_start, from_end))
        };

        // Step 2: find earliest resource-feasible start.
        let demands = block_demands.get(&id).map(|v| v.as_slice()).unwrap_or(&[]);
        let actual_start =
            earliest_feasible_start(dep_start, dur, demands, &committed, &resource_blocks);
        let actual_end = actual_start + dur;

        // Step 3: commit resource windows.
        for &(rb_id, factor) in demands {
            committed
                .entry(rb_id)
                .or_default()
                .push((actual_start, actual_end, factor));
        }

        // Propagate dependency constraints to successors using actual placed times.
        if let Some(edges) = graph.edges.get(&id) {
            for edge in edges {
                let s = edge.successor;
                match edge.dependency_type {
                    DependencyType::FinishToStart => {
                        let v = dep_min_start.entry(s).or_insert(0.0);
                        *v = f32::max(*v, actual_end + edge.lag);
                    }
                    DependencyType::StartToStart => {
                        let v = dep_min_start.entry(s).or_insert(0.0);
                        *v = f32::max(*v, actual_start + edge.lag);
                    }
                    DependencyType::FinishToFinish => {
                        let new = actual_end + edge.lag;
                        let v = dep_min_end.entry(s).or_insert(None);
                        *v = Some(v.map_or(new, |cur| f32::max(cur, new)));
                    }
                    DependencyType::StartToFinish => {
                        let new = actual_start + edge.lag;
                        let v = dep_min_end.entry(s).or_insert(None);
                        *v = Some(v.map_or(new, |cur| f32::max(cur, new)));
                    }
                }
            }
        }

        sched.blocks.insert(id, ScheduledBlock {
            work_block_id: id,
            start_day: actual_start,
            end_day: actual_end,
            duration_days: dur,
        });
    }

    sched.total_duration_days = sched
        .blocks
        .values()
        .map(|b| b.end_day)
        .fold(0.0_f32, f32::max);

    // Critical path in a resource-constrained schedule involves resource float,
    // which requires a more involved analysis; left empty here.
    sched.critical_path = vec![];

    Ok(sched)
}

/// Returns the earliest t ≥ `min_start` at which a block of `duration` days
/// can be placed without exceeding any required resource's capacity.
///
/// Candidate start times are: `min_start`, ends of committed intervals, and
/// boundaries of availability segments — the only points where the feasibility
/// of a window can change.
fn earliest_feasible_start(
    min_start: f32,
    duration: f32,
    demands: &[(ResourceBlockId, f32)],
    committed: &HashMap<ResourceBlockId, Vec<(f32, f32, f32)>>,
    resource_blocks: &HashMap<ResourceBlockId, &ResourceBlock>,
) -> f32 {
    if demands.is_empty() || duration <= 0.0 {
        return min_start;
    }

    let mut candidates: Vec<f32> = vec![min_start];
    for &(rb_id, _) in demands {
        if let Some(ivs) = committed.get(&rb_id) {
            for &(_, end, _) in ivs {
                if end > min_start {
                    candidates.push(end);
                }
            }
        }
        if let Some(rb) = resource_blocks.get(&rb_id) {
            for seg in &rb.availability.segments {
                // Availability can improve at the start of a new segment.
                if seg.start > min_start {
                    candidates.push(seg.start);
                }
                if seg.end > min_start {
                    candidates.push(seg.end);
                }
            }
        }
    }

    candidates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    candidates.dedup_by(|a, b| (*a - *b).abs() < 1e-9);

    for t in candidates {
        if t < min_start {
            continue;
        }
        if feasible_at(t, duration, demands, committed, resource_blocks) {
            return t;
        }
    }

    min_start
}

/// Returns true if [t, t+duration) is feasible for all demanded resources.
fn feasible_at(
    t: f32,
    duration: f32,
    demands: &[(ResourceBlockId, f32)],
    committed: &HashMap<ResourceBlockId, Vec<(f32, f32, f32)>>,
    resource_blocks: &HashMap<ResourceBlockId, &ResourceBlock>,
) -> bool {
    let window_end = t + duration;
    for &(rb_id, demand) in demands {
        let avail = resource_blocks
            .get(&rb_id)
            .map(|rb| min_avail_in_window(rb, t, window_end))
            .unwrap_or(1.0);
        let used = max_demand_in_window(
            committed.get(&rb_id).map(|v| v.as_slice()).unwrap_or(&[]),
            t,
            window_end,
        );
        if used + demand > avail + 1e-6 {
            return false;
        }
    }
    true
}

/// Minimum resource availability factor over [start, end).
/// Gaps in the availability timeline are treated as factor 1.0.
fn min_avail_in_window(rb: &ResourceBlock, start: f32, end: f32) -> f32 {
    if rb.availability.segments.is_empty() {
        return 1.0;
    }
    let mut min = 1.0_f32;
    for seg in &rb.availability.segments {
        if seg.start < end && seg.end > start {
            min = f32::min(min, seg.factor);
        }
    }
    min
}

/// Maximum simultaneous demand from committed intervals over [start, end).
/// Uses a sweep-line to handle overlapping intervals correctly.
fn max_demand_in_window(intervals: &[(f32, f32, f32)], start: f32, end: f32) -> f32 {
    let mut events: Vec<(f32, f32)> = Vec::new();
    for &(is, ie, demand) in intervals {
        if is >= end || ie <= start {
            continue;
        }
        events.push((f32::max(is, start), demand));
        events.push((f32::min(ie, end), -demand));
    }
    if events.is_empty() {
        return 0.0;
    }
    events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut current = 0.0_f32;
    let mut peak = 0.0_f32;
    for (_, delta) in events {
        current += delta;
        if current > peak {
            peak = current;
        }
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::model::{Estimate, Model};

    fn est(days: f32) -> Estimate {
        Estimate { most_likely: days, optimistic: days, pessimistic: days, confidence: 1.0 }
    }

    /// Build a schedule from the model using the given root blocks.
    fn run(model: &Model, roots: Vec<WorkBlockId>) -> Schedule {
        let plan = model.plans.values().next().cloned().unwrap();
        let mut p = plan.clone();
        p.root_blocks = roots;
        let graph = build_graph(model, &p);
        forward_pass(model, &p, &graph).expect("no cycle")
    }

    fn base() -> (Model, crate::model::PlanId) {
        let mut m = Model::default();
        let wid = m.create_world("w");
        let pid = m.create_plan("p", wid);
        (m, pid)
    }

    #[test]
    fn single_block_starts_at_zero() {
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(5.0));
        let s = run(&m, vec![a]);
        let b = &s.blocks[&a];
        assert_eq!(b.start_day, 0.0);
        assert_eq!(b.end_day, 5.0);
        assert_eq!(b.duration_days, 5.0);
        assert_eq!(s.total_duration_days, 5.0);
    }

    #[test]
    fn finish_to_start_chain() {
        // A(3) --FS--> B(2): B.start=3, B.end=5
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&a].start_day, 0.0);
        assert_eq!(s.blocks[&a].end_day,   3.0);
        assert_eq!(s.blocks[&b].start_day, 3.0);
        assert_eq!(s.blocks[&b].end_day,   5.0);
        assert_eq!(s.total_duration_days,  5.0);
    }

    #[test]
    fn finish_to_start_with_lag() {
        // A(3) --FS+2--> B(2): B.start ≥ 3+2=5
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 2.0;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 5.0);
        assert_eq!(s.blocks[&b].end_day,   7.0);
    }

    #[test]
    fn negative_lag_lead() {
        // A(3) --FS-1--> B(2): B.start ≥ 3-1=2
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = -1.0;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 2.0);
    }

    #[test]
    fn start_to_start() {
        // A(3) --SS--> B(2): B.start ≥ 0 → runs in parallel
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        m.create_dependency(a, b, DependencyType::StartToStart);
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 0.0);
        assert_eq!(s.blocks[&b].end_day,   2.0);
    }

    #[test]
    fn start_to_start_with_lag() {
        // A(3) --SS+1--> B(2): B.start ≥ 1
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let dep = m.create_dependency(a, b, DependencyType::StartToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 1.0;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 1.0);
    }

    #[test]
    fn finish_to_finish() {
        // A(3) --FF--> B(2): B.end ≥ 3 → B.start=1
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 1.0);
        assert_eq!(s.blocks[&b].end_day,   3.0);
    }

    #[test]
    fn start_to_finish_with_lag() {
        // A(3) --SF+4--> B(2): B.end ≥ 0+4=4 → B.start=2
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let dep = m.create_dependency(a, b, DependencyType::StartToFinish);
        m.dependencies.get_mut(&dep).unwrap().lag = 4.0;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 2.0);
        assert_eq!(s.blocks[&b].end_day,   4.0);
    }

    #[test]
    fn multiple_predecessors_latest_wins() {
        // A(5) --FS--> C(1)  and  B(3) --FS--> C(1): C.start = max(5,3) = 5
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(5.0));
        let b = m.create_work_block("B", est(3.0));
        let c = m.create_work_block("C", est(1.0));
        m.create_dependency(a, c, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let s = run(&m, vec![a, b, c]);
        assert_eq!(s.blocks[&c].start_day, 5.0);
        assert_eq!(s.total_duration_days,  6.0);
    }

    #[test]
    fn critical_path_linear_chain() {
        // A --FS--> B --FS--> C: critical path is [A, B, C]
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(2.0));
        let b = m.create_work_block("B", est(3.0));
        let c = m.create_work_block("C", est(1.0));
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let s = run(&m, vec![a, b, c]);
        assert_eq!(s.critical_path, vec![a, b, c]);
    }

    #[test]
    fn critical_path_longer_branch_wins() {
        // A(1) --FS--> C(1)
        // B(5) --FS--> C(1)
        // C's critical predecessor is B (longer).
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(1.0));
        let b = m.create_work_block("B", est(5.0));
        let c = m.create_work_block("C", est(1.0));
        m.create_dependency(a, c, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let s = run(&m, vec![a, b, c]);
        assert!(s.critical_path.contains(&b));
        assert!(s.critical_path.contains(&c));
        assert!(!s.critical_path.contains(&a));
    }

    // ── resource leveling tests ──────────────────────────────────────────────

    use crate::model::{AvailabilitySegment, ResourceAllocation, ResourceType};

    fn run_leveled(model: &Model, plan_id: crate::model::PlanId, roots: Vec<WorkBlockId>) -> Schedule {
        let mut plan = model.plans[&plan_id].clone();
        plan.root_blocks = roots;
        let graph = build_graph(model, &plan);
        resource_leveled_pass(model, &plan, &graph).expect("no cycle")
    }

    fn add_alloc(plan: &mut crate::model::Plan, rb: ResourceBlockId, wb: WorkBlockId, factor: f32) {
        plan.allocations.push(ResourceAllocation {
            resource_id: rb,
            work_block_id: wb,
            allocation_factor: factor,
        });
    }

    #[test]
    fn no_constraints_matches_forward_pass() {
        // Without any resource allocations, leveled == unconstrained.
        let (mut m, pid) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let s = run_leveled(&m, pid, vec![a, b]);
        assert_eq!(s.blocks[&a].start_day, 0.0);
        assert_eq!(s.blocks[&b].start_day, 3.0);
    }

    #[test]
    fn two_blocks_serialized_by_resource() {
        // A(2) and B(3) both need resource R at full capacity → B can't start until A ends.
        let (mut m, pid) = base();
        let r = m.create_resource_block("R", ResourceType::Person);
        let a = m.create_work_block("A", est(2.0));
        let b = m.create_work_block("B", est(3.0));
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, a, 1.0);
            add_alloc(plan, r, b, 1.0);
        }
        // No dependency between A and B; topo order is by id (A < B).
        let s = run_leveled(&m, pid, vec![a, b]);
        // A schedules first at [0, 2), B must wait until A finishes.
        assert_eq!(s.blocks[&a].start_day, 0.0);
        assert_eq!(s.blocks[&b].start_day, 2.0);
        assert_eq!(s.total_duration_days, 5.0);
    }

    #[test]
    fn partial_allocations_allow_overlap() {
        // A and B each need 0.5 of resource R (total 1.0 ≤ capacity 1.0) → can run in parallel.
        let (mut m, pid) = base();
        let r = m.create_resource_block("R", ResourceType::Person);
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(3.0));
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, a, 0.5);
            add_alloc(plan, r, b, 0.5);
        }
        let s = run_leveled(&m, pid, vec![a, b]);
        assert_eq!(s.blocks[&a].start_day, 0.0);
        assert_eq!(s.blocks[&b].start_day, 0.0); // parallel — no conflict
        assert_eq!(s.total_duration_days, 3.0);
    }

    #[test]
    fn overallocation_serializes() {
        // A needs 0.7, B needs 0.5 → combined 1.2 > 1.0 → must serialize.
        let (mut m, pid) = base();
        let r = m.create_resource_block("R", ResourceType::Person);
        let a = m.create_work_block("A", est(2.0));
        let b = m.create_work_block("B", est(2.0));
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, a, 0.7);
            add_alloc(plan, r, b, 0.5);
        }
        let s = run_leveled(&m, pid, vec![a, b]);
        assert_eq!(s.blocks[&a].start_day, 0.0);
        assert_eq!(s.blocks[&b].start_day, 2.0); // delayed until A finishes
    }

    #[test]
    fn availability_segment_delays_block() {
        // Resource R is unavailable (factor=0.0) for days [0,5) and available after.
        // Block A needs R → must start at day 5.
        let (mut m, pid) = base();
        let r = m.create_resource_block("R", ResourceType::Person);
        {
            let rb = m.resource_blocks.get_mut(&r).unwrap();
            rb.availability.segments.push(AvailabilitySegment { start: 0.0, end: 5.0, factor: 0.0 });
            rb.availability.segments.push(AvailabilitySegment { start: 5.0, end: 100.0, factor: 1.0 });
        }
        let a = m.create_work_block("A", est(2.0));
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, a, 1.0);
        }
        let s = run_leveled(&m, pid, vec![a]);
        assert_eq!(s.blocks[&a].start_day, 5.0);
        assert_eq!(s.blocks[&a].end_day,   7.0);
    }

    #[test]
    fn dependency_plus_resource_conflict() {
        // Topo order (by WorkBlockId) is a < b < c.
        // c has no deps and schedules first in leveling, occupying R [0, 3).
        // b has a FS dep on a (min_start = 3) and needs R; at day 3 R is free,
        // so b starts at 3 — the resource conflict does NOT delay b here.
        let (mut m, pid) = base();
        let r = m.create_resource_block("R", ResourceType::Person);
        let a = m.create_work_block("A", est(3.0)); // no resource
        let b = m.create_work_block("B", est(2.0)); // needs R
        let c = m.create_work_block("C", est(3.0)); // occupies R [3,6)
        m.create_dependency(a, b, DependencyType::FinishToStart);
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, b, 1.0);
            add_alloc(plan, r, c, 1.0);
        }
        // Topo order: a, c (both roots with no deps on each other), then b
        // a and c can start at 0; c occupies R [0,3).
        // b depends on a (must start ≥ 3), and c starts at 0 occupying R.
        // Wait — c has no dep on a/b. Let's think:
        // Actually topo order (by id) is a < b < c if a, b, c are created in order.
        // a(0→3), b(dep: start≥3; resource: R occupied by c?)
        // c: no deps, no resource conflict at start → schedules at 0, occupies R [0,3).
        // b: dep_min_start=3, R occupied [0,3) by c — so at day 3 R is free → b starts at 3.
        let s = run_leveled(&m, pid, vec![a, b, c]);
        assert_eq!(s.blocks[&a].start_day, 0.0);
        // c schedules before b (lower id); c at [0,3); b dep=3, R free at 3 → b at 3.
        assert_eq!(s.blocks[&b].start_day, 3.0);
    }

    #[test]
    fn three_blocks_one_resource_chain() {
        // A(1), B(1), C(1) all need R at full capacity → serialize: [0,1), [1,2), [2,3).
        let (mut m, pid) = base();
        let r = m.create_resource_block("R", ResourceType::Person);
        let a = m.create_work_block("A", est(1.0));
        let b = m.create_work_block("B", est(1.0));
        let c = m.create_work_block("C", est(1.0));
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, a, 1.0);
            add_alloc(plan, r, b, 1.0);
            add_alloc(plan, r, c, 1.0);
        }
        let s = run_leveled(&m, pid, vec![a, b, c]);
        let sa = s.blocks[&a].start_day;
        let sb = s.blocks[&b].start_day;
        let sc = s.blocks[&c].start_day;
        // All must be serialized; each starts when the previous ends.
        let mut starts = [sa, sb, sc];
        starts.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert_eq!(starts, [0.0, 1.0, 2.0]);
        assert_eq!(s.total_duration_days, 3.0);
    }
}
