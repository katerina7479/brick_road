use std::collections::{HashMap, HashSet, VecDeque};

use crate::model::{Day, DependencyType, Model, Plan, WorkBlockId};

/// One directed edge in the dependency graph.
#[derive(Debug, Clone)]
pub struct Edge {
    pub successor: WorkBlockId,
    pub dependency_type: DependencyType,
    /// Lag in days (positive = delay, negative = lead).
    pub lag: Day,
}

/// Directed acyclic graph of active work blocks for one Plan.
///
/// Active blocks are exactly the plan's `root_blocks`.
#[derive(Debug)]
pub struct DependencyGraph {
    pub nodes: HashSet<WorkBlockId>,
    /// predecessor → outgoing edges (successor, type, lag).
    pub edges: HashMap<WorkBlockId, Vec<Edge>>,
    /// in-degree for each node (number of predecessor edges).
    pub in_degree: HashMap<WorkBlockId, usize>,
}

/// Error returned when a cycle is detected during topological sort.
#[derive(Debug)]
pub struct CycleError {
    /// Blocks that are part of (or depend on) the cycle and could not be sorted.
    pub nodes: Vec<WorkBlockId>,
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dependency cycle among {} block(s): {:?}",
            self.nodes.len(),
            self.nodes.iter().map(|id| id.0).collect::<Vec<_>>()
        )
    }
}

/// Builds a `DependencyGraph` for the given Plan.
///
/// Only the plan's `root_blocks` are included. Dependencies that cross into
/// inactive blocks are silently excluded.
pub fn build_graph(model: &Model, plan: &Plan) -> DependencyGraph {
    let nodes = collect_active_blocks(model, plan);

    let mut edges: HashMap<WorkBlockId, Vec<Edge>> = HashMap::new();
    let mut in_degree: HashMap<WorkBlockId, usize> = HashMap::new();

    for &id in &nodes {
        edges.entry(id).or_default();
        in_degree.entry(id).or_insert(0);
    }

    for dep in model.dependencies.values() {
        // Dependencies are branch-local: only this plan's own deps shape its
        // graph, so a branch's deps never affect main's schedule and vice versa.
        if dep.plan_id == plan.id
            && nodes.contains(&dep.predecessor)
            && nodes.contains(&dep.successor)
        {
            edges.entry(dep.predecessor).or_default().push(Edge {
                successor: dep.successor,
                dependency_type: dep.dependency_type,
                lag: dep.lag,
            });
            *in_degree.entry(dep.successor).or_insert(0) += 1;
        }
    }

    DependencyGraph {
        nodes,
        edges,
        in_degree,
    }
}

/// Topological sort of `graph` using Kahn's algorithm.
///
/// Returns blocks in an order where every predecessor appears before its
/// successors. Returns `Err(CycleError)` if a cycle is detected; the error
/// contains the blocks that could not be placed (those in or depending on
/// the cycle).
pub fn topological_sort(graph: &DependencyGraph) -> Result<Vec<WorkBlockId>, CycleError> {
    let mut in_degree = graph.in_degree.clone();

    let mut queue: VecDeque<WorkBlockId> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&id, _)| id)
        .collect();

    // Deterministic ordering within the same in-degree tier (aids testing).
    let mut queue_vec: Vec<WorkBlockId> = queue.drain(..).collect();
    queue_vec.sort_by_key(|id| id.0);
    queue.extend(queue_vec);

    let mut sorted = Vec::with_capacity(graph.nodes.len());

    while let Some(node) = queue.pop_front() {
        sorted.push(node);
        if let Some(edges) = graph.edges.get(&node) {
            let mut new_zeros: Vec<WorkBlockId> = Vec::new();
            for edge in edges {
                let deg = in_degree
                    .get_mut(&edge.successor)
                    .expect("successor in graph");
                *deg -= 1;
                if *deg == 0 {
                    new_zeros.push(edge.successor);
                }
            }
            new_zeros.sort_by_key(|id| id.0);
            queue.extend(new_zeros);
        }
    }

    if sorted.len() != graph.nodes.len() {
        let cycle_nodes = in_degree
            .into_iter()
            .filter(|(_, d)| *d > 0)
            .map(|(id, _)| id)
            .collect();
        return Err(CycleError { nodes: cycle_nodes });
    }

    Ok(sorted)
}

/// Active blocks are exactly the plan's `root_blocks`.
fn collect_active_blocks(_model: &Model, plan: &Plan) -> HashSet<WorkBlockId> {
    plan.root_blocks.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Model, Plan};

    fn empty_plan(model: &mut Model) -> Plan {
        let plan_id = model.create_plan("p", None);
        model.plans.remove(&plan_id).unwrap()
    }

    #[test]
    fn empty_graph_sorts_to_empty() {
        let mut model = Model::default();
        let plan = empty_plan(&mut model);
        let graph = build_graph(&model, &plan);
        let order = topological_sort(&graph).unwrap();
        assert!(order.is_empty());
    }

    #[test]
    fn linear_chain() {
        let mut model = Model::default();
        let plan_id = model.create_plan("p", None);

        let a = model.create_work_block("A");
        let b = model.create_work_block("B");
        let c = model.create_work_block("C");
        model.create_dependency(a, b, DependencyType::FinishToStart);
        model.create_dependency(b, c, DependencyType::FinishToStart);

        let plan = {
            let p = model.plans.get_mut(&plan_id).unwrap();
            p.root_blocks = vec![a, b, c];
            p.clone()
        };

        let graph = build_graph(&model, &plan);
        let order = topological_sort(&graph).unwrap();
        assert_eq!(order, vec![a, b, c]);
    }

    #[test]
    fn diamond() {
        let mut model = Model::default();
        let plan_id = model.create_plan("p", None);

        let a = model.create_work_block("A");
        let b = model.create_work_block("B");
        let c = model.create_work_block("C");
        let d = model.create_work_block("D");
        // A → B, A → C, B → D, C → D
        model.create_dependency(a, b, DependencyType::FinishToStart);
        model.create_dependency(a, c, DependencyType::FinishToStart);
        model.create_dependency(b, d, DependencyType::FinishToStart);
        model.create_dependency(c, d, DependencyType::FinishToStart);

        let plan = {
            let p = model.plans.get_mut(&plan_id).unwrap();
            p.root_blocks = vec![a, b, c, d];
            p.clone()
        };

        let graph = build_graph(&model, &plan);
        let order = topological_sort(&graph).unwrap();

        // A must come first, D must come last.
        assert_eq!(order[0], a);
        assert_eq!(*order.last().unwrap(), d);
        // B and C must both appear before D.
        let pos = |id| order.iter().position(|&x| x == id).unwrap();
        assert!(pos(b) < pos(d));
        assert!(pos(c) < pos(d));
    }

    #[test]
    fn cycle_detected() {
        let mut model = Model::default();
        let plan_id = model.create_plan("p", None);

        let a = model.create_work_block("A");
        let b = model.create_work_block("B");
        let c = model.create_work_block("C");
        model.create_dependency(a, b, DependencyType::FinishToStart);
        model.create_dependency(b, c, DependencyType::FinishToStart);
        model.create_dependency(c, a, DependencyType::FinishToStart); // closes cycle

        let plan = {
            let p = model.plans.get_mut(&plan_id).unwrap();
            p.root_blocks = vec![a, b, c];
            p.clone()
        };

        let graph = build_graph(&model, &plan);
        let result = topological_sort(&graph);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.nodes.len(), 3);
    }

    #[test]
    fn inactive_blocks_excluded() {
        // Blocks not in plan.root_blocks must not appear in the graph,
        // even if dependencies reference them.
        let mut model = Model::default();
        let plan_id = model.create_plan("p", None);

        let a = model.create_work_block("A");
        let b = model.create_work_block("B"); // NOT in plan
        model.create_dependency(a, b, DependencyType::FinishToStart);

        let plan = {
            let p = model.plans.get_mut(&plan_id).unwrap();
            p.root_blocks = vec![a];
            p.clone()
        };

        let graph = build_graph(&model, &plan);
        assert!(graph.nodes.contains(&a));
        assert!(!graph.nodes.contains(&b));
        let order = topological_sort(&graph).unwrap();
        assert_eq!(order, vec![a]);
    }

    #[test]
    fn dependencies_are_branch_local() {
        // A dependency added in a branch must not shape main's graph, even when
        // both endpoints are blocks main also has.
        let mut model = Model::default();
        let main = model.create_plan("main", None);
        let branch = model.create_plan("branch", Some(0));
        let a = model.create_work_block("A");
        let b = model.create_work_block("B");
        // Same two blocks live in both plans; the dep belongs to the branch.
        model.plans.get_mut(&main).unwrap().root_blocks = vec![a, b];
        model.plans.get_mut(&branch).unwrap().root_blocks = vec![a, b];
        model.create_dependency_in(branch, a, b, DependencyType::FinishToStart);

        let main_plan = model.plans[&main].clone();
        let main_graph = build_graph(&model, &main_plan);
        assert_eq!(
            main_graph.in_degree.get(&b),
            Some(&0),
            "branch dep must not appear in main's graph"
        );

        let branch_plan = model.plans[&branch].clone();
        let branch_graph = build_graph(&model, &branch_plan);
        assert_eq!(
            branch_graph.in_degree.get(&b),
            Some(&1),
            "the dep shapes the branch's own graph"
        );
    }
}
