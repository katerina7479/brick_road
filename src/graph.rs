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
/// Active blocks are those reachable from `plan.root_blocks` following
/// the variant selections in `plan.selected_variants`. Blocks that have
/// variants but no selection are included as leaves (they contribute no
/// children to the graph).
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
/// Only blocks reachable from `plan.root_blocks` (following the selected
/// variant at each block) are included. Dependencies that cross into
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
        if nodes.contains(&dep.predecessor) && nodes.contains(&dep.successor) {
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

/// BFS from plan root blocks, expanding children via selected variants.
fn collect_active_blocks(model: &Model, plan: &Plan) -> HashSet<WorkBlockId> {
    let mut active = HashSet::new();
    let mut queue = VecDeque::new();

    for &root in &plan.root_blocks {
        if active.insert(root) {
            queue.push_back(root);
        }
    }

    while let Some(block_id) = queue.pop_front() {
        if let Some(wb) = model.work_blocks.get(&block_id) {
            if !wb.variants.is_empty() {
                if let Some(&var_id) = plan.selected_variants.get(&block_id) {
                    if let Some(variant) = model.variants.get(&var_id) {
                        for &child in &variant.children {
                            if active.insert(child) {
                                queue.push_back(child);
                            }
                        }
                    }
                }
                // Block with variants but no selection: included as leaf.
            }
        }
    }

    active
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
    fn variant_selection_expands_correct_children() {
        let mut model = Model::default();
        let plan_id = model.create_plan("p", None);

        let parent = model.create_work_block("parent");
        let child_a = model.create_work_block("child_a");
        let child_b = model.create_work_block("child_b");
        let var_a = model.create_variant("fast", parent);
        let var_b = model.create_variant("slow", parent);
        model
            .variants
            .get_mut(&var_a)
            .unwrap()
            .children
            .push(child_a);
        model
            .variants
            .get_mut(&var_b)
            .unwrap()
            .children
            .push(child_b);
        model.work_blocks.get_mut(&parent).unwrap().variants = vec![var_a, var_b];

        let plan = {
            let p = model.plans.get_mut(&plan_id).unwrap();
            p.root_blocks = vec![parent];
            p.selected_variants.insert(parent, var_a);
            p.clone()
        };

        let graph = build_graph(&model, &plan);
        assert!(graph.nodes.contains(&parent));
        assert!(graph.nodes.contains(&child_a)); // selected variant's child included
        assert!(!graph.nodes.contains(&child_b)); // unselected variant's child excluded
    }

    #[test]
    fn variant_with_no_selection_is_leaf() {
        // A block with variants but no selection in plan.selected_variants
        // must appear in the graph as a leaf — no children expanded.
        let mut model = Model::default();
        let plan_id = model.create_plan("p", None);

        let parent = model.create_work_block("parent");
        let child = model.create_work_block("child");
        let var_a = model.create_variant("v", parent);
        model.variants.get_mut(&var_a).unwrap().children.push(child);
        model.work_blocks.get_mut(&parent).unwrap().variants = vec![var_a];

        // No selected_variants entry for parent.
        let plan = {
            let p = model.plans.get_mut(&plan_id).unwrap();
            p.root_blocks = vec![parent];
            p.clone()
        };

        let graph = build_graph(&model, &plan);
        assert!(graph.nodes.contains(&parent));
        assert!(!graph.nodes.contains(&child)); // children not expanded without selection
        let order = topological_sort(&graph).unwrap();
        assert_eq!(order, vec![parent]);
    }

    #[test]
    fn inactive_blocks_excluded() {
        // Blocks not in plan.root_blocks and not reachable via variants
        // must not appear in the graph, even if dependencies reference them.
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
}
