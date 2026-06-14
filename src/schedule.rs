use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::graph::{CycleError, DependencyGraph};
use crate::model::{
    DependencyType, Model, Plan, PlanId, ResourceBlock, ResourceBlockId, WorkBlock, WorkBlockId,
};

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

/// Output of a backward-pass critical-path analysis over a forward-pass Schedule.
#[derive(Debug, Clone)]
pub struct CriticalPathAnalysis {
    /// Active blocks with zero total float, in topological order.
    pub critical_path: Vec<WorkBlockId>,
    /// Total float (slack) for every active block: `latest_finish − earliest_finish`.
    /// Non-negative in a valid schedule; zero marks a critical block.
    pub float: HashMap<WorkBlockId, f32>,
}

/// Returns placed work blocks (duration_days > 0) sorted by ascending
/// `start_day`, with `id` as a stable tie-breaker. Blocks with
/// `duration_days == 0.0` are omitted to avoid phantom zero-width rows
/// for blocks not yet reachable from any plan.
pub fn sorted_blocks(model: &Model) -> Vec<&WorkBlock> {
    let mut blocks: Vec<&WorkBlock> = model
        .work_blocks
        .values()
        .filter(|wb| wb.duration_days > 0.0)
        .collect();
    blocks.sort_by(|a, b| {
        a.start_day
            .partial_cmp(&b.start_day)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.0.cmp(&b.id.0))
    });
    blocks
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

        // Propagate constraints to successors.
        if let Some(edges) = graph.edges.get(&id) {
            for edge in edges {
                let s = edge.successor;
                match edge.dependency_type {
                    DependencyType::FinishToStart => {
                        let new = earliest_end + edge.lag;
                        let v = min_start.entry(s).or_insert(0.0);
                        if new > *v {
                            *v = new;
                        }
                    }
                    DependencyType::StartToStart => {
                        let new = earliest_start + edge.lag;
                        let v = min_start.entry(s).or_insert(0.0);
                        if new > *v {
                            *v = new;
                        }
                    }
                    DependencyType::FinishToFinish => {
                        let new = earliest_end + edge.lag;
                        let v = min_end.entry(s).or_insert(None);
                        if v.is_none_or(|cur| new > cur) {
                            *v = Some(new);
                        }
                    }
                    DependencyType::StartToFinish => {
                        let new = earliest_start + edge.lag;
                        let v = min_end.entry(s).or_insert(None);
                        if v.is_none_or(|cur| new > cur) {
                            *v = Some(new);
                        }
                    }
                }
            }
        }

        sched.blocks.insert(
            id,
            ScheduledBlock {
                work_block_id: id,
                start_day: earliest_start,
                end_day: earliest_end,
                duration_days: dur,
            },
        );
    }

    sched.total_duration_days = sched
        .blocks
        .values()
        .map(|b| b.end_day)
        .fold(0.0_f32, f32::max);

    sched.critical_path = backward_pass(&order, graph, &sched).critical_path;

    Ok(sched)
}

/// Compute latest start/finish and total float for every block in `schedule`.
///
/// Backward-pass semantics (P = predecessor, S = successor, lag in days).
/// Each edge type gives an upper bound on LF(P):
///   FS:  LF(P) ≤ LS(S) − lag          where LS(S) = LF(S) − dur(S)
///   SS:  LF(P) ≤ LS(S) − lag + dur(P)
///   FF:  LF(P) ≤ LF(S) − lag
///   SF:  LF(P) ≤ LF(S) − lag + dur(P)
///
/// Float (total slack) = LF − EF.  Blocks with zero float are critical.
pub fn backward_pass(
    order: &[WorkBlockId],
    graph: &DependencyGraph,
    schedule: &Schedule,
) -> CriticalPathAnalysis {
    // Build reverse edge map: successor → [(predecessor, dependency_type, lag)].
    let mut reverse: HashMap<WorkBlockId, Vec<(WorkBlockId, DependencyType, f32)>> =
        graph.nodes.iter().map(|&id| (id, Vec::new())).collect();
    for (&pred, edges) in &graph.edges {
        for edge in edges {
            reverse
                .entry(edge.successor)
                .or_default()
                .push((pred, edge.dependency_type, edge.lag));
        }
    }

    let total = schedule.total_duration_days;

    // Initialise LF to project end for every block (unconstrained).
    let mut latest_finish: HashMap<WorkBlockId, f32> =
        graph.nodes.iter().map(|&id| (id, total)).collect();

    // Process in reverse topological order (successors before predecessors).
    for &s_id in order.iter().rev() {
        let lf_s = *latest_finish.get(&s_id).unwrap_or(&total);
        let dur_s = schedule
            .blocks
            .get(&s_id)
            .map(|b| b.duration_days)
            .unwrap_or(0.0);
        let ls_s = lf_s - dur_s;

        if let Some(preds) = reverse.get(&s_id) {
            for &(pred_id, dep_type, lag) in preds {
                let dur_p = schedule
                    .blocks
                    .get(&pred_id)
                    .map(|b| b.duration_days)
                    .unwrap_or(0.0);
                let bound = match dep_type {
                    DependencyType::FinishToStart => ls_s - lag,
                    DependencyType::StartToStart => ls_s - lag + dur_p,
                    DependencyType::FinishToFinish => lf_s - lag,
                    DependencyType::StartToFinish => lf_s - lag + dur_p,
                };
                let v = latest_finish.entry(pred_id).or_insert(total);
                if bound < *v {
                    *v = bound;
                }
            }
        }
    }

    // Float = LF − EF for each block.
    const CRITICAL_EPS: f32 = 1e-4;
    let float: HashMap<WorkBlockId, f32> = graph
        .nodes
        .iter()
        .map(|&id| {
            let ef = schedule.blocks.get(&id).map(|b| b.end_day).unwrap_or(0.0);
            let lf = *latest_finish.get(&id).unwrap_or(&total);
            (id, lf - ef)
        })
        .collect();

    // Critical path: zero-float blocks in topological order.
    let critical_path = order
        .iter()
        .filter(|&&id| float.get(&id).is_some_and(|&f| f.abs() < CRITICAL_EPS))
        .copied()
        .collect();

    CriticalPathAnalysis {
        critical_path,
        float,
    }
}

/// Compute the critical path and total float using the user's manually-placed
/// `start_day` / `duration_days` on each `WorkBlock` rather than the output
/// of a forward pass. Float is measured relative to the user's own placement,
/// so a block with zero float cannot be delayed without extending the project.
///
/// Reads durations and finish times directly from `model.work_blocks`; a
/// `forward_pass` is not required. Returns `Err(CycleError)` on a dependency
/// cycle.
pub fn analyze_user_placement(
    model: &Model,
    graph: &DependencyGraph,
) -> Result<CriticalPathAnalysis, CycleError> {
    let order = crate::graph::topological_sort(graph)?;

    // Project end = latest finish over all active blocks in user placement.
    let total = graph
        .nodes
        .iter()
        .filter_map(|id| model.work_blocks.get(id))
        .map(|wb| wb.start_day + wb.duration_days)
        .fold(0.0_f32, f32::max);

    // Build reverse edge map: successor → [(predecessor, dep_type, lag)].
    let mut reverse: HashMap<WorkBlockId, Vec<(WorkBlockId, DependencyType, f32)>> =
        graph.nodes.iter().map(|&id| (id, Vec::new())).collect();
    for (&pred, edges) in &graph.edges {
        for edge in edges {
            reverse
                .entry(edge.successor)
                .or_default()
                .push((pred, edge.dependency_type, edge.lag));
        }
    }

    // Initialise LF to project end for every block.
    let mut latest_finish: HashMap<WorkBlockId, f32> =
        graph.nodes.iter().map(|&id| (id, total)).collect();

    // Process in reverse topological order (successors before predecessors).
    for &s_id in order.iter().rev() {
        let lf_s = *latest_finish.get(&s_id).unwrap_or(&total);
        let dur_s = model
            .work_blocks
            .get(&s_id)
            .map(|wb| wb.duration_days)
            .unwrap_or(0.0);
        let ls_s = lf_s - dur_s;

        if let Some(preds) = reverse.get(&s_id) {
            for &(pred_id, dep_type, lag) in preds {
                let dur_p = model
                    .work_blocks
                    .get(&pred_id)
                    .map(|wb| wb.duration_days)
                    .unwrap_or(0.0);
                let bound = match dep_type {
                    DependencyType::FinishToStart => ls_s - lag,
                    DependencyType::StartToStart => ls_s - lag + dur_p,
                    DependencyType::FinishToFinish => lf_s - lag,
                    DependencyType::StartToFinish => lf_s - lag + dur_p,
                };
                let v = latest_finish.entry(pred_id).or_insert(total);
                if bound < *v {
                    *v = bound;
                }
            }
        }
    }

    // Float = LF − EF for each block (EF from user placement).
    const CRITICAL_EPS: f32 = 1e-4;
    let float: HashMap<WorkBlockId, f32> = graph
        .nodes
        .iter()
        .map(|&id| {
            let ef = model
                .work_blocks
                .get(&id)
                .map(|wb| wb.start_day + wb.duration_days)
                .unwrap_or(0.0);
            let lf = *latest_finish.get(&id).unwrap_or(&total);
            (id, lf - ef)
        })
        .collect();

    // Critical path: zero-float blocks in topological order.
    let critical_path = order
        .iter()
        .filter(|&&id| float.get(&id).is_some_and(|&f| f.abs() < CRITICAL_EPS))
        .copied()
        .collect();

    Ok(CriticalPathAnalysis {
        critical_path,
        float,
    })
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

    let resource_blocks: HashMap<ResourceBlockId, &ResourceBlock> = model
        .resource_blocks
        .iter()
        .map(|(&id, rb)| (id, rb))
        .collect();

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

        sched.blocks.insert(
            id,
            ScheduledBlock {
                work_block_id: id,
                start_day: actual_start,
                end_day: actual_end,
                duration_days: dur,
            },
        );
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
///
/// Divides the window into sub-intervals at every point where either the
/// availability factor or the committed demand changes, then checks whether
/// `existing_demand + new_demand ≤ availability` at each sub-interval.
/// This avoids the conservative over-rejection that occurs when peak demand
/// and minimum availability don't coincide at the same time point.
fn feasible_at(
    t: f32,
    duration: f32,
    demands: &[(ResourceBlockId, f32)],
    committed: &HashMap<ResourceBlockId, Vec<(f32, f32, f32)>>,
    resource_blocks: &HashMap<ResourceBlockId, &ResourceBlock>,
) -> bool {
    let window_end = t + duration;
    for &(rb_id, demand) in demands {
        // Collect every time point where availability or committed demand changes
        // within the window. Using these as sub-interval boundaries ensures both
        // functions are piecewise-constant within each sub-interval.
        let mut pts: Vec<f32> = vec![t, window_end];
        if let Some(ivs) = committed.get(&rb_id) {
            for &(is, ie, _) in ivs {
                if is > t && is < window_end {
                    pts.push(is);
                }
                if ie > t && ie < window_end {
                    pts.push(ie);
                }
            }
        }
        if let Some(rb) = resource_blocks.get(&rb_id) {
            for seg in &rb.availability.segments {
                if seg.start > t && seg.start < window_end {
                    pts.push(seg.start);
                }
                if seg.end > t && seg.end < window_end {
                    pts.push(seg.end);
                }
            }
        }
        pts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        pts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);

        for w in pts.windows(2) {
            let mid = (w[0] + w[1]) * 0.5;
            let avail = resource_blocks
                .get(&rb_id)
                .map(|rb| avail_at(rb, mid))
                .unwrap_or(1.0);
            let used = demand_at(
                committed.get(&rb_id).map(|v| v.as_slice()).unwrap_or(&[]),
                mid,
            );
            if used + demand > avail + 1e-6 {
                return false;
            }
        }
    }
    true
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

/// Total committed demand at instant `t`.
fn demand_at(intervals: &[(f32, f32, f32)], t: f32) -> f32 {
    intervals
        .iter()
        .filter(|&&(is, ie, _)| is <= t && t < ie)
        .map(|&(_, _, d)| d)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::model::{Estimate, Model};

    fn est(days: f32) -> Estimate {
        Estimate {
            most_likely: days,
            optimistic: days,
            pessimistic: days,
            confidence: 1.0,
        }
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
        assert_eq!(s.blocks[&a].end_day, 3.0);
        assert_eq!(s.blocks[&b].start_day, 3.0);
        assert_eq!(s.blocks[&b].end_day, 5.0);
        assert_eq!(s.total_duration_days, 5.0);
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
        assert_eq!(s.blocks[&b].end_day, 7.0);
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
        assert_eq!(s.blocks[&b].end_day, 2.0);
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
        assert_eq!(s.blocks[&b].end_day, 3.0);
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
        assert_eq!(s.blocks[&b].end_day, 4.0);
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
        assert_eq!(s.total_duration_days, 6.0);
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

    // --- backward_pass / float tests ---

    fn analyze(model: &Model, roots: Vec<WorkBlockId>) -> (Schedule, CriticalPathAnalysis) {
        use crate::graph::{build_graph, topological_sort};
        let plan = model.plans.values().next().cloned().unwrap();
        let mut p = plan.clone();
        p.root_blocks = roots;
        let graph = build_graph(model, &p);
        let order = topological_sort(&graph).expect("no cycle");
        let sched = forward_pass(model, &p, &graph).expect("no cycle");
        let analysis = backward_pass(&order, &graph, &sched);
        (sched, analysis)
    }

    #[test]
    fn float_single_block_is_zero() {
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(5.0));
        let (_, ana) = analyze(&m, vec![a]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0.0);
        assert_eq!(ana.critical_path, vec![a]);
    }

    #[test]
    fn float_linear_chain_all_zero() {
        // A(3) --FS--> B(2) --FS--> C(1): all float = 0
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let c = m.create_work_block("C", est(1.0));
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let (_, ana) = analyze(&m, vec![a, b, c]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0.0);
        assert_eq!(*ana.float.get(&b).unwrap(), 0.0);
        assert_eq!(*ana.float.get(&c).unwrap(), 0.0);
        assert_eq!(ana.critical_path, vec![a, b, c]);
    }

    #[test]
    fn float_parallel_branch_has_positive_float() {
        // A(5) --FS--> C(1)   total = 6
        // B(3) --FS--> C(1)
        // B.float = LF_B − EF_B = 5 − 3 = 2
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(5.0));
        let b = m.create_work_block("B", est(3.0));
        let c = m.create_work_block("C", est(1.0));
        m.create_dependency(a, c, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let (_, ana) = analyze(&m, vec![a, b, c]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0.0);
        assert_eq!(*ana.float.get(&c).unwrap(), 0.0);
        assert!((*ana.float.get(&b).unwrap() - 2.0).abs() < 1e-4);
        assert!(ana.critical_path.contains(&a));
        assert!(ana.critical_path.contains(&c));
        assert!(!ana.critical_path.contains(&b));
    }

    #[test]
    fn float_ff_dependency() {
        // A(3) --FF--> B(2): EF_A=3, ES_B=1, EF_B=3, total=3
        // Backward: LF_B=3, LF_A ≤ LF_B − 0 = 3 → float_A = 3−3 = 0
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        let (_, ana) = analyze(&m, vec![a, b]);
        assert!(ana.float.get(&a).unwrap().abs() < 1e-4);
        assert!(ana.float.get(&b).unwrap().abs() < 1e-4);
        assert!(ana.critical_path.contains(&a));
        assert!(ana.critical_path.contains(&b));
    }

    #[test]
    fn float_ff_mixed_with_fs_correct_attribution() {
        // B(10) --FF--> C(5)   and   A(3) --FS--> C(5)
        // B created first → lower ID → processed first in topo order.
        // es_from_end = 10−5 = 5 > es_from_start = 3 → C.start = 5, C.end = 10.
        // total = 10.  Backward: LF_B ≤ LF_C − 0 = 10; LF_A ≤ LS_C − 0 = 5.
        // float_B = 10−10 = 0, float_A = 5−3 = 2, float_C = 10−10 = 0.
        // Critical path: B and C only.
        let (mut m, _) = base();
        let b = m.create_work_block("B", est(10.0));
        let a = m.create_work_block("A", est(3.0));
        let c = m.create_work_block("C", est(5.0));
        m.create_dependency(b, c, DependencyType::FinishToFinish);
        m.create_dependency(a, c, DependencyType::FinishToStart);
        let (_, ana) = analyze(&m, vec![a, b, c]);
        assert!(
            ana.float.get(&b).unwrap().abs() < 1e-4,
            "B float should be 0"
        );
        assert!(
            ana.float.get(&c).unwrap().abs() < 1e-4,
            "C float should be 0"
        );
        assert!(
            (*ana.float.get(&a).unwrap() - 2.0).abs() < 1e-4,
            "A float should be 2"
        );
        assert!(ana.critical_path.contains(&b), "B on critical path");
        assert!(ana.critical_path.contains(&c), "C on critical path");
        assert!(!ana.critical_path.contains(&a), "A not on critical path");
    }

    #[test]
    fn float_with_lag() {
        // A(3) --FS+2--> B(2): EF_A=3, ES_B=5, EF_B=7, total=7
        // Backward: LF_B=7, LS_B=5, LF_A ≤ LS_B − 2 = 3 → float_A=3−3=0
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 2.0;
        let (_, ana) = analyze(&m, vec![a, b]);
        assert!(ana.float.get(&a).unwrap().abs() < 1e-4);
        assert!(ana.float.get(&b).unwrap().abs() < 1e-4);
        assert_eq!(ana.critical_path, vec![a, b]);
    }

    // ── resource leveling tests ──────────────────────────────────────────────

    use crate::model::{AvailabilitySegment, ResourceAllocation, ResourceType};

    fn run_leveled(
        model: &Model,
        plan_id: crate::model::PlanId,
        roots: Vec<WorkBlockId>,
    ) -> Schedule {
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
            rb.availability.segments.push(AvailabilitySegment {
                start: 0.0,
                end: 5.0,
                factor: 0.0,
            });
            rb.availability.segments.push(AvailabilitySegment {
                start: 5.0,
                end: 100.0,
                factor: 1.0,
            });
        }
        let a = m.create_work_block("A", est(2.0));
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, a, 1.0);
        }
        let s = run_leveled(&m, pid, vec![a]);
        assert_eq!(s.blocks[&a].start_day, 5.0);
        assert_eq!(s.blocks[&a].end_day, 7.0);
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
    fn per_point_check_avoids_conservative_delay() {
        // Resource R: low availability [0,5) at factor 0.3, full capacity [5,100).
        // C uses R at 0.8 — committed to [5,8) because R is below 0.8 before day 5.
        // B needs R at 0.2, dep constraint forces start ≥ 3, duration = 5 → window [3,8).
        //
        // Conservative check (old): min_avail=0.3, max_demand=0.8 → 1.0 > 0.3 → rejects day 3.
        // Per-point sweep (new):
        //   [3,5): avail=0.3, used=0.0 → 0.2 ≤ 0.3 ✓
        //   [5,8): avail=1.0, used=0.8 → 1.0 ≤ 1.0 ✓
        //   → accepts day 3.
        let (mut m, pid) = base();
        let r = m.create_resource_block("R", ResourceType::Person);
        {
            let rb = m.resource_blocks.get_mut(&r).unwrap();
            rb.availability.segments.push(AvailabilitySegment {
                start: 0.0,
                end: 5.0,
                factor: 0.3,
            });
            rb.availability.segments.push(AvailabilitySegment {
                start: 5.0,
                end: 100.0,
                factor: 1.0,
            });
        }
        let a = m.create_work_block("A", est(3.0)); // no resource; creates dep constraint for B
        let c = m.create_work_block("C", est(3.0)); // uses R at 0.8 → scheduled [5,8)
        let b = m.create_work_block("B", est(5.0)); // needs R at 0.2; dep B.start ≥ 3
        m.create_dependency(a, b, DependencyType::FinishToStart);
        {
            let plan = m.plans.get_mut(&pid).unwrap();
            add_alloc(plan, r, c, 0.8);
            add_alloc(plan, r, b, 0.2);
        }
        // Topo order by id: a, c, b.
        // a: no resource → [0,3).
        // c: R avail=0.3 in [0,5) < 0.8 → must wait until day 5 → [5,8).
        // b: dep_start=3, R committed [(5,8,0.8)]; per-point sweep accepts [3,8) → starts at 3.
        let s = run_leveled(&m, pid, vec![a, c, b]);
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

    // --- analyze_user_placement tests ---

    fn place(model: &mut Model, id: WorkBlockId, start: f32, dur: f32) {
        let wb = model.work_blocks.get_mut(&id).unwrap();
        wb.start_day = start;
        wb.duration_days = dur;
    }

    fn analyze_placed(model: &Model, roots: Vec<WorkBlockId>) -> CriticalPathAnalysis {
        let plan = model.plans.values().next().cloned().unwrap();
        let mut p = plan;
        p.root_blocks = roots;
        let graph = build_graph(model, &p);
        analyze_user_placement(model, &graph).expect("no cycle")
    }

    #[test]
    fn user_placement_single_block_zero_float() {
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(5.0));
        place(&mut m, a, 0.0, 5.0);
        let ana = analyze_placed(&m, vec![a]);
        assert!((ana.float[&a]).abs() < 1e-4);
        assert_eq!(ana.critical_path, vec![a]);
    }

    #[test]
    fn user_placement_linear_chain_all_critical() {
        // A(0→3) --FS--> B(3→5) --FS--> C(5→6): total = 6, all float = 0
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let c = m.create_work_block("C", est(1.0));
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        place(&mut m, a, 0.0, 3.0);
        place(&mut m, b, 3.0, 2.0);
        place(&mut m, c, 5.0, 1.0);
        let ana = analyze_placed(&m, vec![a, b, c]);
        assert!(ana.float[&a].abs() < 1e-4);
        assert!(ana.float[&b].abs() < 1e-4);
        assert!(ana.float[&c].abs() < 1e-4);
        assert_eq!(ana.critical_path, vec![a, b, c]);
    }

    #[test]
    fn user_placement_parallel_branch_has_float() {
        // A(0→5) --FS--> C(5→6)   total = 6
        // B(0→3) --FS--> C(5→6)   B.float = LF_B(5) − EF_B(3) = 2
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(5.0));
        let b = m.create_work_block("B", est(3.0));
        let c = m.create_work_block("C", est(1.0));
        m.create_dependency(a, c, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        place(&mut m, a, 0.0, 5.0);
        place(&mut m, b, 0.0, 3.0);
        place(&mut m, c, 5.0, 1.0);
        let ana = analyze_placed(&m, vec![a, b, c]);
        assert!(ana.float[&a].abs() < 1e-4, "A should be critical");
        assert!(ana.float[&c].abs() < 1e-4, "C should be critical");
        assert!((ana.float[&b] - 2.0).abs() < 1e-4, "B float should be 2");
        assert!(!ana.critical_path.contains(&b));
        assert!(ana.critical_path.contains(&a));
        assert!(ana.critical_path.contains(&c));
    }

    #[test]
    fn user_placement_float_with_lag() {
        // A(0→3) --FS+2--> B(5→7): LS_B=5, LF_A ≤ 5−2=3 → float_A = 3−3 = 0
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 2.0;
        place(&mut m, a, 0.0, 3.0);
        place(&mut m, b, 5.0, 2.0);
        let ana = analyze_placed(&m, vec![a, b]);
        assert!(ana.float[&a].abs() < 1e-4);
        assert!(ana.float[&b].abs() < 1e-4);
    }

    #[test]
    fn user_placement_ss_predecessor_has_float() {
        // A(0→3) --SS--> B(1→5): SS requires B.start ≥ A.start (slack = 1 day).
        // total = 5; backward: LS_B = 5−4 = 1; LF_A_bound = LS_B − 0 + dur_A = 1 + 3 = 4
        // float_A = 4 − 3 = 1; float_B = 0 (B is the last block).
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(4.0));
        m.create_dependency(a, b, DependencyType::StartToStart);
        place(&mut m, a, 0.0, 3.0);
        place(&mut m, b, 1.0, 4.0);
        let ana = analyze_placed(&m, vec![a, b]);
        assert!((ana.float[&a] - 1.0).abs() < 1e-4, "A float should be 1");
        assert!(ana.float[&b].abs() < 1e-4, "B float should be 0");
        assert!(ana.critical_path.contains(&b), "B is critical");
        assert!(!ana.critical_path.contains(&a), "A is not critical");
    }

    #[test]
    fn user_placement_ff_both_critical() {
        // A(0→3) --FF--> B(1→3): FF requires B.end ≥ A.end = 3; B.end = 3 (tight).
        // total = 3; backward: LF_A_bound = LF_B − 0 = 3 → float_A = 3−3 = 0; float_B = 0.
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(2.0));
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        place(&mut m, a, 0.0, 3.0);
        place(&mut m, b, 1.0, 2.0);
        let ana = analyze_placed(&m, vec![a, b]);
        assert!(ana.float[&a].abs() < 1e-4, "A float should be 0");
        assert!(ana.float[&b].abs() < 1e-4, "B float should be 0");
        assert!(ana.critical_path.contains(&a), "A is critical");
        assert!(ana.critical_path.contains(&b), "B is critical");
    }

    #[test]
    fn user_placement_sf_with_lag_both_critical() {
        // A(0→3) --SF+4--> B(0→4): SF+4 requires B.end ≥ A.start+4 = 4; B.end = 4 (tight).
        // total = 4; backward: LF_A_bound = LF_B − 4 + dur_A = 4 − 4 + 3 = 3 → float_A = 0.
        let (mut m, _) = base();
        let a = m.create_work_block("A", est(3.0));
        let b = m.create_work_block("B", est(4.0));
        let dep = m.create_dependency(a, b, DependencyType::StartToFinish);
        m.dependencies.get_mut(&dep).unwrap().lag = 4.0;
        place(&mut m, a, 0.0, 3.0);
        place(&mut m, b, 0.0, 4.0);
        let ana = analyze_placed(&m, vec![a, b]);
        assert!(ana.float[&a].abs() < 1e-4, "A float should be 0");
        assert!(ana.float[&b].abs() < 1e-4, "B float should be 0");
        assert!(ana.critical_path.contains(&a), "A is critical");
        assert!(ana.critical_path.contains(&b), "B is critical");
    }

    #[test]
    fn sorted_blocks_skips_unplaced() {
        let mut m = Model::default();
        let placed_id = m.create_work_block("placed", est(3.0));
        let unplaced_id = m.create_work_block("unplaced", est(2.0));
        m.work_blocks.get_mut(&placed_id).unwrap().start_day = 1.0;
        m.work_blocks.get_mut(&placed_id).unwrap().duration_days = 3.0;

        let result = sorted_blocks(&m);
        let ids: Vec<WorkBlockId> = result.iter().map(|wb| wb.id).collect();
        assert!(ids.contains(&placed_id), "placed block should appear");
        assert!(!ids.contains(&unplaced_id), "unplaced block should be filtered out");
    }
}
