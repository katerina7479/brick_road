use std::collections::HashMap;

use crate::graph::{CycleError, DependencyGraph};
use crate::model::{DependencyType, Model, Plan, PlanId, WorkBlockId};

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
#[derive(Debug, Clone)]
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
}
