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

/// Output of a backward-pass critical-path analysis over a forward-pass Schedule.
#[derive(Debug, Clone)]
pub struct CriticalPathAnalysis {
    /// Active blocks with zero total float, in topological order.
    pub critical_path: Vec<WorkBlockId>,
    /// Total float (slack) for every active block: `latest_finish − earliest_finish`.
    /// Non-negative in a valid schedule; zero marks a critical block.
    pub float: HashMap<WorkBlockId, f32>,
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

    sched.critical_path = backward_pass(graph, &sched).critical_path;

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
    graph: &DependencyGraph,
    schedule: &Schedule,
) -> CriticalPathAnalysis {
    let order = match crate::graph::topological_sort(graph) {
        Ok(o) => o,
        Err(_) => return CriticalPathAnalysis { critical_path: vec![], float: HashMap::new() },
    };

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
        let dur_s = schedule.blocks.get(&s_id).map(|b| b.duration_days).unwrap_or(0.0);
        let ls_s = lf_s - dur_s;

        if let Some(preds) = reverse.get(&s_id) {
            for &(pred_id, dep_type, lag) in preds {
                let dur_p =
                    schedule.blocks.get(&pred_id).map(|b| b.duration_days).unwrap_or(0.0);
                let bound = match dep_type {
                    DependencyType::FinishToStart  => ls_s - lag,
                    DependencyType::StartToStart   => ls_s - lag + dur_p,
                    DependencyType::FinishToFinish => lf_s - lag,
                    DependencyType::StartToFinish  => lf_s - lag + dur_p,
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

    CriticalPathAnalysis { critical_path, float }
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

    // --- backward_pass / float tests ---

    fn analyze(model: &Model, roots: Vec<WorkBlockId>) -> (Schedule, CriticalPathAnalysis) {
        use crate::graph::build_graph;
        let plan = model.plans.values().next().cloned().unwrap();
        let mut p = plan.clone();
        p.root_blocks = roots;
        let graph = build_graph(model, &p);
        let sched = forward_pass(model, &p, &graph).expect("no cycle");
        let analysis = backward_pass(&graph, &sched);
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
        assert!(ana.float.get(&b).unwrap().abs() < 1e-4, "B float should be 0");
        assert!(ana.float.get(&c).unwrap().abs() < 1e-4, "C float should be 0");
        assert!((*ana.float.get(&a).unwrap() - 2.0).abs() < 1e-4, "A float should be 2");
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
}
