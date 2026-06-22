use std::collections::HashMap;

use bevy::prelude::{DetectChanges, Res, ResMut, Resource};
use chrono::NaiveDate;

use crate::graph::{CycleError, DependencyGraph};
use crate::model::{
    CalendarConfig, Day, DependencyType, Model, PlanId, WorkBlock, WorkBlockId,
};

/// Converts a working-day position to a calendar date using the plan's calendar.
/// Day 0 = `config.start_date`; positive values advance through working days only.
pub fn working_day_to_date(day: Day, config: &CalendarConfig) -> NaiveDate {
    crate::calendar::day_to_date(day, config)
}

/// Returns the number of calendar days spanned by `effort_days` of work
/// starting at `start_day` (in working-day units).  Accounts for weekends
/// and non-working dates in the plan's calendar.
pub fn calendar_span(start_day: Day, effort_days: Day, config: &CalendarConfig) -> i64 {
    let start_date = working_day_to_date(start_day, config);
    crate::calendar::effort_to_calendar_days(effort_days, start_date, config)
}

/// Snaps a computed start day to the start of the next whole working day.
/// Fractional positions (mid-day) are ceiled so blocks begin at day boundaries.
/// Whole-number positions are returned unchanged.
fn snap_to_day_start(t: Day) -> Day {
    t
}

/// The computed time placement of one work block.
#[derive(Debug, Clone)]
pub struct ScheduledBlock {
    pub work_block_id: WorkBlockId,
    pub start_day: Day,
    pub end_day: Day,
    /// Convenience: end_day - start_day.
    pub duration_days: Day,
}

/// The full output of a scheduler run over a Plan.
#[derive(Debug, Clone, Default, Resource)]
pub struct Schedule {
    /// Placement for every block that was scheduled.
    pub blocks: HashMap<WorkBlockId, ScheduledBlock>,
    /// Day on which the last block finishes.
    pub total_duration_days: Day,
    /// Ordered sequence of block IDs on the critical path (longest path).
    pub critical_path: Vec<WorkBlockId>,
}

/// Output of a backward-pass critical-path analysis over a forward-pass Schedule.
#[derive(Debug, Clone)]
pub struct CriticalPathAnalysis {
    /// Active blocks with zero total float, in topological order.
    pub critical_path: Vec<WorkBlockId>,
    /// Total float (slack) for every active block: `latest_finish − earliest_finish`.
    /// Non-negative in a valid schedule; zero marks a critical block.
    pub float: HashMap<WorkBlockId, Day>,
}

/// Returns placed work blocks (duration_days > 0) sorted by ascending
/// `start_day`, with `id` as a stable tie-breaker. Blocks with
/// `duration_days == 0.0` are omitted to avoid phantom zero-width rows
/// for blocks not yet reachable from any plan.
pub fn sorted_blocks(model: &Model) -> Vec<&WorkBlock> {
    let mut blocks: Vec<&WorkBlock> = model
        .work_blocks
        .values()
        .filter(|wb| wb.duration_days > 0)
        .collect();
    blocks.sort_by(|a, b| a.start_day.cmp(&b.start_day).then(a.id.0.cmp(&b.id.0)));
    blocks
}

/// The blocks shown on the active timeline. With no drill-in (`drill` = `None`)
/// these are the plan's own top-level `root_blocks`; when drilled into a block,
/// they are that block's children. Placed (`duration_days > 0`) only, sorted by
/// ascending `start_day` with `id` as a tie-breaker.
pub fn visible_blocks(
    model: &Model,
    plan_id: PlanId,
    drill: Option<WorkBlockId>,
) -> Vec<&WorkBlock> {
    let ids: Vec<WorkBlockId> = match drill {
        Some(parent) => model.children(parent),
        None => match model.plans.get(&plan_id) {
            Some(plan) => plan.root_blocks.clone(),
            None => return Vec::new(),
        },
    };
    let mut blocks: Vec<&WorkBlock> = ids
        .iter()
        .filter_map(|id| model.work_blocks.get(id))
        .filter(|wb| wb.duration_days > 0)
        .collect();
    blocks.sort_by(|a, b| a.start_day.cmp(&b.start_day).then(a.id.0.cmp(&b.id.0)));
    blocks
}

/// Drill-in navigation path. Empty = the plan's top level; otherwise the last
/// entry is the block currently drilled into, and the timeline shows its
/// children. Earlier entries are the breadcrumb trail back out.
#[derive(Debug, Default, Resource)]
pub struct DrillScope {
    pub path: Vec<WorkBlockId>,
}

impl DrillScope {
    /// The block currently drilled into, if any.
    pub fn current(&self) -> Option<WorkBlockId> {
        self.path.last().copied()
    }
}

/// Cached result of `visible_blocks()`, recomputed only when `Model` changes.
/// All per-frame consumers read from this resource instead of calling
/// `visible_blocks()` directly.
#[derive(Debug, Default, Resource)]
pub struct VisibleBlocks {
    pub ids: Vec<WorkBlockId>,
}

/// Refreshes `VisibleBlocks` when the model changes.
///
/// Only writes to `cache.ids` when the content actually changes, so downstream
/// systems that check `visible_blocks.is_changed()` do not fire on every frame
/// during block drag/resize (where only position changes, not the visible set).
pub fn update_visible_blocks(
    model: Res<Model>,
    schedule: Res<Schedule>,
    drill: Res<DrillScope>,
    mut cache: ResMut<VisibleBlocks>,
) {
    if !model.is_changed() && !schedule.is_changed() && !drill.is_changed() {
        return;
    }
    let new_ids: Vec<WorkBlockId> = match model.main_plan_id() {
        Some(main_id) => visible_blocks(&model, main_id, drill.current())
            .into_iter()
            .map(|wb| wb.id)
            .collect(),
        None => Vec::new(),
    };
    if new_ids != cache.ids {
        cache.ids = new_ids;
    }
}

/// Tracks today's position on the timeline as a working-day number.
#[derive(Debug, Default, Resource)]
pub struct TodayMarker {
    pub day: Day,
}

/// Recomputes `TodayMarker` when the model's calendar changes.
///
/// Converts the UTC Unix timestamp to a Gregorian date using the
/// Howard Hinnant algorithm — no `chrono/clock` feature required.
pub fn update_today_marker(model: Res<Model>, mut today: ResMut<TodayMarker>) {
    if !model.is_changed() {
        return;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil date from Unix day count (Hinnant algorithm, public domain).
    let z = (secs / 86400) as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + if m <= 2 { 1 } else { 0 };
    let today_date = NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
    today.day = crate::calendar::today_marker_day(today_date, &model.calendar);
}

/// Propagate dependency constraints to all blocks reachable (transitively)
/// as successors of `root` after `root`'s `start_day` or `duration_days`
/// has changed.
///
/// Successors are visited in topological order so each block is updated after
/// all of its own predecessors. For each successor, `start_day` is set to the
/// maximum bound imposed by ALL of its predecessors (not only those reachable
/// from `root`), clamped to ≥ 0.0. Constraint formulas (P = predecessor,
/// S = successor, lag in days):
///   FS:  S.start = P.start + P.dur + lag
///   SS:  S.start = P.start + lag
///   FF:  S.start = P.start + P.dur + lag − S.dur
///   SF:  S.start = P.start + lag − S.dur
/// The earliest start day `block` may legally have given its predecessors'
/// current positions — the maximum lower bound across its incoming dependencies
/// (B depends on A: B.start ≥ bound(A)). `0` if it has no predecessors.
///
/// `plan` filters which deps apply: `None` considers every dependency (used by
/// the cascade — a block's position is shared, so moving it pushes dependents
/// across plans); `Some(p)` considers only plan `p`'s own deps (used by the drag
/// clamp, so a branch's hypothetical deps never constrain a main edit).
fn lower_bound(model: &Model, block: WorkBlockId, plan: Option<PlanId>) -> Day {
    let succ_dur = model
        .work_blocks
        .get(&block)
        .map(|wb| wb.duration_days)
        .unwrap_or(0);
    model
        .dependencies
        .values()
        .filter(|d| d.successor == block && plan.is_none_or(|p| d.plan_id == p))
        .filter_map(|d| {
            model
                .work_blocks
                .get(&d.predecessor)
                .map(|p| match d.dependency_type {
                    DependencyType::FinishToStart => p.start_day + p.duration_days + d.lag,
                    DependencyType::StartToStart => p.start_day + d.lag,
                    DependencyType::FinishToFinish => {
                        p.start_day + p.duration_days + d.lag - succ_dur
                    }
                    DependencyType::StartToFinish => p.start_day + d.lag - succ_dur,
                })
        })
        .fold(0, |a, b| a.max(b))
        .max(0)
}

/// Lower bound across ALL of `block`'s predecessors (any plan). Used by cascade.
pub fn predecessor_lower_bound(model: &Model, block: WorkBlockId) -> Day {
    lower_bound(model, block, None)
}

/// Lower bound from only `plan`'s own dependencies. Used by the drag clamp so a
/// branch's deps don't constrain a drag in another plan (e.g. main).
pub fn predecessor_lower_bound_in(model: &Model, plan: PlanId, block: WorkBlockId) -> Day {
    lower_bound(model, block, Some(plan))
}

/// Whether `dep` is currently satisfied: the successor starts no earlier than
/// the bound this one dependency imposes. A branch dep can end up violated and
/// unfixable when its successor is a main block (a "rock" the cascade won't
/// move) — the UI highlights those. Missing endpoints count as satisfied.
pub fn dependency_satisfied(model: &Model, dep: &crate::model::Dependency) -> bool {
    let (Some(pred), Some(succ)) = (
        model.work_blocks.get(&dep.predecessor),
        model.work_blocks.get(&dep.successor),
    ) else {
        return true;
    };
    let bound = match dep.dependency_type {
        DependencyType::FinishToStart => pred.start_day + pred.duration_days + dep.lag,
        DependencyType::StartToStart => pred.start_day + dep.lag,
        DependencyType::FinishToFinish => {
            pred.start_day + pred.duration_days + dep.lag - succ.duration_days
        }
        DependencyType::StartToFinish => pred.start_day + dep.lag - succ.duration_days,
    };
    succ.start_day >= bound
}

pub fn cascade_dependencies(model: &mut Model, root: WorkBlockId) {
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut outgoing: HashMap<WorkBlockId, Vec<WorkBlockId>> = HashMap::new();
    // Cascade follows ALL dependencies, across plans. A block's position is
    // shared by id, so moving it should push everything that depends on it —
    // including a branch's dependent when you move a block main shares as a
    // ghost. (Plan-scoping lives in build_graph, for a plan's own schedule.)
    for dep in model.dependencies.values() {
        outgoing
            .entry(dep.predecessor)
            .or_default()
            .push(dep.successor);
    }

    // BFS to collect all transitively reachable successors of root.
    let mut reachable: HashSet<WorkBlockId> = HashSet::new();
    let mut bfs: VecDeque<WorkBlockId> = VecDeque::new();
    if let Some(succs) = outgoing.get(&root) {
        for &s in succs {
            if reachable.insert(s) {
                bfs.push_back(s);
            }
        }
    }
    while let Some(id) = bfs.pop_front() {
        if let Some(succs) = outgoing.get(&id) {
            for &s in succs {
                if reachable.insert(s) {
                    bfs.push_back(s);
                }
            }
        }
    }
    if reachable.is_empty() {
        return;
    }

    // Topological sort of the reachable subgraph via Kahn's algorithm.
    // In-degrees count only edges between reachable nodes (root excluded).
    let mut in_deg: HashMap<WorkBlockId, usize> = reachable.iter().map(|&id| (id, 0)).collect();
    for &id in &reachable {
        if let Some(succs) = outgoing.get(&id) {
            for &s in succs {
                if reachable.contains(&s) {
                    *in_deg.get_mut(&s).unwrap() += 1;
                }
            }
        }
    }
    let mut queue: VecDeque<WorkBlockId> = in_deg
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut order: Vec<WorkBlockId> = Vec::new();
    while let Some(id) = queue.pop_front() {
        order.push(id);
        if let Some(succs) = outgoing.get(&id) {
            for &s in succs {
                if let Some(d) = in_deg.get_mut(&s) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(s);
                    }
                }
            }
        }
    }

    // Main blocks are "rocks in the stream": a branch's dependencies never move
    // them — only main's own deps may. So a main block is pushed by its
    // main-scoped bound; every other (branch-owned) block by its full bound.
    let main_id = model.main_plan_id();
    let main_blocks: HashSet<WorkBlockId> = main_id
        .and_then(|id| model.plans.get(&id))
        .map(|p| p.root_blocks.iter().copied().collect())
        .unwrap_or_default();

    // Apply constraints in topological order. Push-only ("stay put"): a
    // successor moves later only if it now violates its lower bound; any extra
    // gap the user left is preserved (we never pull it earlier to be snug).
    for id in order {
        let bound = match main_id {
            Some(mid) if main_blocks.contains(&id) => predecessor_lower_bound_in(model, mid, id),
            _ => predecessor_lower_bound(model, id),
        };
        if let Some(wb) = model.work_blocks.get_mut(&id) {
            if bound > wb.start_day {
                wb.start_day = bound;
            }
        }
    }
}

/// Compute unconstrained earliest start/end for every active block (Demand
/// Planning mode, PRD §6.1). Uses each block's `duration_days`; no resource
/// constraints are applied.
///
/// Dependency semantics (P = predecessor, S = successor, lag in days):
///   FS:  start(S) ≥ end(P)   + lag
///   SS:  start(S) ≥ start(P) + lag
///   FF:    end(S) ≥ end(P)   + lag
///   SF:    end(S) ≥ start(P) + lag
///
/// Returns `Err(CycleError)` if the dependency graph contains a cycle.
pub fn forward_pass(model: &Model, graph: &DependencyGraph) -> Result<Schedule, CycleError> {
    let order = crate::graph::topological_sort(graph)?;

    // Lower bound on start day from FS/SS edges.
    let mut min_start: HashMap<WorkBlockId, Day> = graph.nodes.iter().map(|&id| (id, 0)).collect();
    // Lower bound on end day from FF/SF edges.
    let mut min_end: HashMap<WorkBlockId, Option<Day>> =
        graph.nodes.iter().map(|&id| (id, None)).collect();

    let mut sched = Schedule::default();

    for &id in &order {
        let dur = model
            .work_blocks
            .get(&id)
            .map(|wb| wb.duration_days)
            .unwrap_or(0);

        let es_from_start = *min_start.get(&id).unwrap_or(&0);
        let es_from_end = min_end
            .get(&id)
            .and_then(|v| *v)
            .map(|me| me - dur)
            .unwrap_or(0);

        let earliest_start = snap_to_day_start(0.max(es_from_start.max(es_from_end)));
        let earliest_end = earliest_start + dur;

        // Propagate constraints to successors.
        if let Some(edges) = graph.edges.get(&id) {
            for edge in edges {
                let s = edge.successor;
                match edge.dependency_type {
                    DependencyType::FinishToStart => {
                        let new = earliest_end + edge.lag;
                        let v = min_start.entry(s).or_insert(0);
                        if new > *v {
                            *v = new;
                        }
                    }
                    DependencyType::StartToStart => {
                        let new = earliest_start + edge.lag;
                        let v = min_start.entry(s).or_insert(0);
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
        .fold(0, |a, b| a.max(b));

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
    let mut reverse: HashMap<WorkBlockId, Vec<(WorkBlockId, DependencyType, Day)>> =
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
    let mut latest_finish: HashMap<WorkBlockId, Day> =
        graph.nodes.iter().map(|&id| (id, total)).collect();

    // Process in reverse topological order (successors before predecessors).
    for &s_id in order.iter().rev() {
        let lf_s = *latest_finish.get(&s_id).unwrap_or(&total);
        let dur_s = schedule
            .blocks
            .get(&s_id)
            .map(|b| b.duration_days)
            .unwrap_or(0);
        let ls_s = lf_s - dur_s;

        if let Some(preds) = reverse.get(&s_id) {
            for &(pred_id, dep_type, lag) in preds {
                let dur_p = schedule
                    .blocks
                    .get(&pred_id)
                    .map(|b| b.duration_days)
                    .unwrap_or(0);
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
    let float: HashMap<WorkBlockId, Day> = graph
        .nodes
        .iter()
        .map(|&id| {
            let ef = schedule.blocks.get(&id).map(|b| b.end_day).unwrap_or(0);
            let lf = *latest_finish.get(&id).unwrap_or(&total);
            (id, lf - ef)
        })
        .collect();

    // Critical path: zero-float blocks in topological order.
    let critical_path = order
        .iter()
        .filter(|&&id| float.get(&id).is_some_and(|&f| f == 0))
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
        .fold(0, |a, b| a.max(b));

    // Build reverse edge map: successor → [(predecessor, dep_type, lag)].
    let mut reverse: HashMap<WorkBlockId, Vec<(WorkBlockId, DependencyType, Day)>> =
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
    let mut latest_finish: HashMap<WorkBlockId, Day> =
        graph.nodes.iter().map(|&id| (id, total)).collect();

    // Process in reverse topological order (successors before predecessors).
    for &s_id in order.iter().rev() {
        let lf_s = *latest_finish.get(&s_id).unwrap_or(&total);
        let dur_s = model
            .work_blocks
            .get(&s_id)
            .map(|wb| wb.duration_days)
            .unwrap_or(0);
        let ls_s = lf_s - dur_s;

        if let Some(preds) = reverse.get(&s_id) {
            for &(pred_id, dep_type, lag) in preds {
                let dur_p = model
                    .work_blocks
                    .get(&pred_id)
                    .map(|wb| wb.duration_days)
                    .unwrap_or(0);
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
    let float: HashMap<WorkBlockId, Day> = graph
        .nodes
        .iter()
        .map(|&id| {
            let ef = model
                .work_blocks
                .get(&id)
                .map(|wb| wb.start_day + wb.duration_days)
                .unwrap_or(0);
            let lf = *latest_finish.get(&id).unwrap_or(&total);
            (id, lf - ef)
        })
        .collect();

    // Critical path: zero-float blocks in topological order.
    let critical_path = order
        .iter()
        .filter(|&&id| float.get(&id).is_some_and(|&f| f == 0))
        .copied()
        .collect();

    Ok(CriticalPathAnalysis {
        critical_path,
        float,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::model::Model;

    /// Create a work block with the given duration (the field the scheduler reads).
    fn mk(m: &mut Model, name: &str, dur: Day) -> WorkBlockId {
        let id = m.create_work_block(name);
        m.work_blocks.get_mut(&id).unwrap().duration_days = dur;
        id
    }

    /// Build a schedule from the model using the given root blocks.
    fn run(model: &Model, roots: Vec<WorkBlockId>) -> Schedule {
        let plan = model.plans.values().next().cloned().unwrap();
        let mut p = plan.clone();
        p.root_blocks = roots;
        let graph = build_graph(model, &p);
        forward_pass(model, &graph).expect("no cycle")
    }

    fn base() -> (Model, crate::model::PlanId) {
        let mut m = Model::default();
        let pid = m.create_plan("p", None);
        (m, pid)
    }

    #[test]
    fn single_block_starts_at_zero() {
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 5);
        let s = run(&m, vec![a]);
        let b = &s.blocks[&a];
        assert_eq!(b.start_day, 0);
        assert_eq!(b.end_day, 5);
        assert_eq!(b.duration_days, 5);
        assert_eq!(s.total_duration_days, 5);
    }

    #[test]
    fn finish_to_start_chain() {
        // A(3) --FS--> B(2): B.start=3, B.end=5
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&a].start_day, 0);
        assert_eq!(s.blocks[&a].end_day, 3);
        assert_eq!(s.blocks[&b].start_day, 3);
        assert_eq!(s.blocks[&b].end_day, 5);
        assert_eq!(s.total_duration_days, 5);
    }

    #[test]
    fn finish_to_start_with_lag() {
        // A(3) --FS+2--> B(2): B.start ≥ 3+2=5
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 2;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 5);
        assert_eq!(s.blocks[&b].end_day, 7);
    }

    #[test]
    fn negative_lag_lead() {
        // A(3) --FS-1--> B(2): B.start ≥ 3-1=2
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = -1;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 2);
    }

    #[test]
    fn start_to_start() {
        // A(3) --SS--> B(2): B.start ≥ 0 → runs in parallel
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        m.create_dependency(a, b, DependencyType::StartToStart);
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 0);
        assert_eq!(s.blocks[&b].end_day, 2);
    }

    #[test]
    fn start_to_start_with_lag() {
        // A(3) --SS+1--> B(2): B.start ≥ 1
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let dep = m.create_dependency(a, b, DependencyType::StartToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 1;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 1);
    }

    #[test]
    fn finish_to_finish() {
        // A(3) --FF--> B(2): B.end ≥ 3 → B.start=1
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 1);
        assert_eq!(s.blocks[&b].end_day, 3);
    }

    #[test]
    fn start_to_finish_with_lag() {
        // A(3) --SF+4--> B(2): B.end ≥ 0+4=4 → B.start=2
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let dep = m.create_dependency(a, b, DependencyType::StartToFinish);
        m.dependencies.get_mut(&dep).unwrap().lag = 4;
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 2);
        assert_eq!(s.blocks[&b].end_day, 4);
    }

    #[test]
    fn multiple_predecessors_latest_wins() {
        // A(5) --FS--> C(1)  and  B(3) --FS--> C(1): C.start = max(5,3) = 5
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 5);
        let b = mk(&mut m, "B", 3);
        let c = mk(&mut m, "C", 1);
        m.create_dependency(a, c, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let s = run(&m, vec![a, b, c]);
        assert_eq!(s.blocks[&c].start_day, 5);
        assert_eq!(s.total_duration_days, 6);
    }

    #[test]
    fn critical_path_linear_chain() {
        // A --FS--> B --FS--> C: critical path is [A, B, C]
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 2);
        let b = mk(&mut m, "B", 3);
        let c = mk(&mut m, "C", 1);
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
        let a = mk(&mut m, "A", 1);
        let b = mk(&mut m, "B", 5);
        let c = mk(&mut m, "C", 1);
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
        let sched = forward_pass(model, &graph).expect("no cycle");
        let analysis = backward_pass(&order, &graph, &sched);
        (sched, analysis)
    }

    #[test]
    fn float_single_block_is_zero() {
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 5);
        let (_, ana) = analyze(&m, vec![a]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0);
        assert_eq!(ana.critical_path, vec![a]);
    }

    #[test]
    fn float_linear_chain_all_zero() {
        // A(3) --FS--> B(2) --FS--> C(1): all float = 0
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let c = mk(&mut m, "C", 1);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let (_, ana) = analyze(&m, vec![a, b, c]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0);
        assert_eq!(*ana.float.get(&b).unwrap(), 0);
        assert_eq!(*ana.float.get(&c).unwrap(), 0);
        assert_eq!(ana.critical_path, vec![a, b, c]);
    }

    #[test]
    fn float_parallel_branch_has_positive_float() {
        // A(5) --FS--> C(1)   total = 6
        // B(3) --FS--> C(1)
        // B.float = LF_B − EF_B = 5 − 3 = 2
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 5);
        let b = mk(&mut m, "B", 3);
        let c = mk(&mut m, "C", 1);
        m.create_dependency(a, c, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        let (_, ana) = analyze(&m, vec![a, b, c]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0);
        assert_eq!(*ana.float.get(&c).unwrap(), 0);
        assert_eq!(*ana.float.get(&b).unwrap(), 2);
        assert!(ana.critical_path.contains(&a));
        assert!(ana.critical_path.contains(&c));
        assert!(!ana.critical_path.contains(&b));
    }

    #[test]
    fn float_ff_dependency() {
        // A(3) --FF--> B(2): EF_A=3, ES_B=1, EF_B=3, total=3
        // Backward: LF_B=3, LF_A ≤ LF_B − 0 = 3 → float_A = 3−3 = 0
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        let (_, ana) = analyze(&m, vec![a, b]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0);
        assert_eq!(*ana.float.get(&b).unwrap(), 0);
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
        let b = mk(&mut m, "B", 10);
        let a = mk(&mut m, "A", 3);
        let c = mk(&mut m, "C", 5);
        m.create_dependency(b, c, DependencyType::FinishToFinish);
        m.create_dependency(a, c, DependencyType::FinishToStart);
        let (_, ana) = analyze(&m, vec![a, b, c]);
        assert_eq!(*ana.float.get(&b).unwrap(), 0, "B float should be 0");
        assert_eq!(*ana.float.get(&c).unwrap(), 0, "C float should be 0");
        assert_eq!(*ana.float.get(&a).unwrap(), 2, "A float should be 2");
        assert!(ana.critical_path.contains(&b), "B on critical path");
        assert!(ana.critical_path.contains(&c), "C on critical path");
        assert!(!ana.critical_path.contains(&a), "A not on critical path");
    }

    #[test]
    fn float_with_lag() {
        // A(3) --FS+2--> B(2): EF_A=3, ES_B=5, EF_B=7, total=7
        // Backward: LF_B=7, LS_B=5, LF_A ≤ LS_B − 2 = 3 → float_A=3−3=0
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 2;
        let (_, ana) = analyze(&m, vec![a, b]);
        assert_eq!(*ana.float.get(&a).unwrap(), 0);
        assert_eq!(*ana.float.get(&b).unwrap(), 0);
        assert_eq!(ana.critical_path, vec![a, b]);
    }

    // --- analyze_user_placement tests ---

    fn place(model: &mut Model, id: WorkBlockId, start: Day, dur: Day) {
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
        let a = mk(&mut m, "A", 5);
        place(&mut m, a, 0, 5);
        let ana = analyze_placed(&m, vec![a]);
        assert_eq!(ana.float[&a], 0);
        assert_eq!(ana.critical_path, vec![a]);
    }

    #[test]
    fn user_placement_linear_chain_all_critical() {
        // A(0→3) --FS--> B(3→5) --FS--> C(5→6): total = 6, all float = 0
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let c = mk(&mut m, "C", 1);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        place(&mut m, a, 0, 3);
        place(&mut m, b, 3, 2);
        place(&mut m, c, 5, 1);
        let ana = analyze_placed(&m, vec![a, b, c]);
        assert_eq!(ana.float[&a], 0);
        assert_eq!(ana.float[&b], 0);
        assert_eq!(ana.float[&c], 0);
        assert_eq!(ana.critical_path, vec![a, b, c]);
    }

    #[test]
    fn user_placement_parallel_branch_has_float() {
        // A(0→5) --FS--> C(5→6)   total = 6
        // B(0→3) --FS--> C(5→6)   B.float = LF_B(5) − EF_B(3) = 2
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 5);
        let b = mk(&mut m, "B", 3);
        let c = mk(&mut m, "C", 1);
        m.create_dependency(a, c, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        place(&mut m, a, 0, 5);
        place(&mut m, b, 0, 3);
        place(&mut m, c, 5, 1);
        let ana = analyze_placed(&m, vec![a, b, c]);
        assert_eq!(ana.float[&a], 0, "A should be critical");
        assert_eq!(ana.float[&c], 0, "C should be critical");
        assert_eq!(ana.float[&b], 2, "B float should be 2");
        assert!(!ana.critical_path.contains(&b));
        assert!(ana.critical_path.contains(&a));
        assert!(ana.critical_path.contains(&c));
    }

    #[test]
    fn user_placement_float_with_lag() {
        // A(0→3) --FS+2--> B(5→7): LS_B=5, LF_A ≤ 5−2=3 → float_A = 3−3 = 0
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep).unwrap().lag = 2;
        place(&mut m, a, 0, 3);
        place(&mut m, b, 5, 2);
        let ana = analyze_placed(&m, vec![a, b]);
        assert_eq!(ana.float[&a], 0);
        assert_eq!(ana.float[&b], 0);
    }

    #[test]
    fn user_placement_ss_predecessor_has_float() {
        // A(0→3) --SS--> B(1→5): SS requires B.start ≥ A.start (slack = 1 day).
        // total = 5; backward: LS_B = 5−4 = 1; LF_A_bound = LS_B − 0 + dur_A = 1 + 3 = 4
        // float_A = 4 − 3 = 1; float_B = 0 (B is the last block).
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 4);
        m.create_dependency(a, b, DependencyType::StartToStart);
        place(&mut m, a, 0, 3);
        place(&mut m, b, 1, 4);
        let ana = analyze_placed(&m, vec![a, b]);
        assert_eq!(ana.float[&a], 1, "A float should be 1");
        assert_eq!(ana.float[&b], 0, "B float should be 0");
        assert!(ana.critical_path.contains(&b), "B is critical");
        assert!(!ana.critical_path.contains(&a), "A is not critical");
    }

    #[test]
    fn user_placement_ff_both_critical() {
        // A(0→3) --FF--> B(1→3): FF requires B.end ≥ A.end = 3; B.end = 3 (tight).
        // total = 3; backward: LF_A_bound = LF_B − 0 = 3 → float_A = 3−3 = 0; float_B = 0.
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 2);
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        place(&mut m, a, 0, 3);
        place(&mut m, b, 1, 2);
        let ana = analyze_placed(&m, vec![a, b]);
        assert_eq!(ana.float[&a], 0, "A float should be 0");
        assert_eq!(ana.float[&b], 0, "B float should be 0");
        assert!(ana.critical_path.contains(&a), "A is critical");
        assert!(ana.critical_path.contains(&b), "B is critical");
    }

    #[test]
    fn user_placement_sf_with_lag_both_critical() {
        // A(0→3) --SF+4--> B(0→4): SF+4 requires B.end ≥ A.start+4 = 4; B.end = 4 (tight).
        // total = 4; backward: LF_A_bound = LF_B − 4 + dur_A = 4 − 4 + 3 = 3 → float_A = 0.
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 3);
        let b = mk(&mut m, "B", 4);
        let dep = m.create_dependency(a, b, DependencyType::StartToFinish);
        m.dependencies.get_mut(&dep).unwrap().lag = 4;
        place(&mut m, a, 0, 3);
        place(&mut m, b, 0, 4);
        let ana = analyze_placed(&m, vec![a, b]);
        assert_eq!(ana.float[&a], 0, "A float should be 0");
        assert_eq!(ana.float[&b], 0, "B float should be 0");
        assert!(ana.critical_path.contains(&a), "A is critical");
        assert!(ana.critical_path.contains(&b), "B is critical");
    }

    #[test]
    fn sorted_blocks_skips_unplaced() {
        let mut m = Model::default();
        let placed_id = mk(&mut m, "placed", 3);
        // Unplaced: a block with no duration (duration_days == 0) is filtered out.
        let unplaced_id = m.create_work_block("unplaced");
        m.work_blocks.get_mut(&placed_id).unwrap().start_day = 1;
        m.work_blocks.get_mut(&placed_id).unwrap().duration_days = 3;

        let result = sorted_blocks(&m);
        let ids: Vec<WorkBlockId> = result.iter().map(|wb| wb.id).collect();
        assert!(ids.contains(&placed_id), "placed block should appear");
        assert!(
            !ids.contains(&unplaced_id),
            "unplaced block should be filtered out"
        );
    }

    // ── cascade_dependencies tests ──────────────────────────────────────────

    fn placed(m: &mut Model, name: &str, start: Day, dur: Day) -> WorkBlockId {
        let id = m.create_work_block(name);
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = start;
        wb.duration_days = dur;
        id
    }

    #[test]
    fn lower_bound_per_dependency_type() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 2, 5); // start 2, end 7
        let b = placed(&mut m, "B", 0, 3);

        // FS: B.start >= A.end = 7.
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);
        assert_eq!(predecessor_lower_bound(&m, b), 7);
        // lag shifts the bound.
        m.dependencies.get_mut(&dep).unwrap().lag = 2;
        assert_eq!(predecessor_lower_bound(&m, b), 9);
        m.dependencies.get_mut(&dep).unwrap().lag = 0;

        // SS: B.start >= A.start = 2.
        m.dependencies.get_mut(&dep).unwrap().dependency_type = DependencyType::StartToStart;
        assert_eq!(predecessor_lower_bound(&m, b), 2);

        // FF: B.start >= A.end - B.dur = 7 - 3 = 4.
        m.dependencies.get_mut(&dep).unwrap().dependency_type = DependencyType::FinishToFinish;
        assert_eq!(predecessor_lower_bound(&m, b), 4);
    }

    #[test]
    fn scoped_lower_bound_ignores_other_plans_deps() {
        // A branch's dependency must not constrain a main drag: the plan-scoped
        // bound for main is 0, even though the global bound reflects the dep.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let branch = m.create_plan("branch", Some(0));
        let a = placed(&mut m, "A", 0, 5); // ends at 5
        let b = placed(&mut m, "B", 0, 3);
        m.create_dependency_in(branch, a, b, DependencyType::FinishToStart);

        assert_eq!(
            predecessor_lower_bound(&m, b),
            5,
            "global bound sees the branch dep"
        );
        assert_eq!(
            predecessor_lower_bound_in(&m, main, b),
            0,
            "a main drag is not constrained by the branch dep"
        );
        assert_eq!(
            predecessor_lower_bound_in(&m, branch, b),
            5,
            "the branch's own drag is constrained"
        );
    }

    #[test]
    fn dependency_satisfied_detects_violations() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5); // ends at 5
        let b = placed(&mut m, "B", 5, 3); // starts exactly at A's end
        let dep = m.create_dependency(a, b, DependencyType::FinishToStart);

        // FS satisfied: B.start (5) >= A.end (5).
        assert!(dependency_satisfied(&m, &m.dependencies[&dep].clone()));

        // Drag B earlier — now violated.
        m.work_blocks.get_mut(&b).unwrap().start_day = 3;
        assert!(!dependency_satisfied(&m, &m.dependencies[&dep].clone()));

        // Extra slack is still satisfied (deps don't pull snug).
        m.work_blocks.get_mut(&b).unwrap().start_day = 12;
        assert!(dependency_satisfied(&m, &m.dependencies[&dep].clone()));

        // SS: B.start >= A.start.
        m.dependencies.get_mut(&dep).unwrap().dependency_type = DependencyType::StartToStart;
        m.work_blocks.get_mut(&b).unwrap().start_day = 0; // A.start is 0
        assert!(dependency_satisfied(&m, &m.dependencies[&dep].clone()));
        m.work_blocks.get_mut(&a).unwrap().start_day = 2; // now A.start 2 > B.start 0
        assert!(!dependency_satisfied(&m, &m.dependencies[&dep].clone()));
    }

    #[test]
    fn lower_bound_zero_without_predecessors() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 5, 3);
        assert_eq!(predecessor_lower_bound(&m, a), 0);
    }

    #[test]
    fn cascade_is_push_only_keeps_extra_gap() {
        // A dependent with slack stays put when the predecessor moves earlier;
        // it is only pushed when the predecessor would violate it.
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5); // ends at 5
        let b = placed(&mut m, "B", 10, 3); // 5 days of slack past the FS bound
        m.create_dependency(a, b, DependencyType::FinishToStart);

        // Move A earlier — slack only grows; B must not be pulled back.
        m.work_blocks.get_mut(&a).unwrap().start_day = -2;
        cascade_dependencies(&mut m, a);
        assert_eq!(m.work_blocks[&b].start_day, 10, "extra gap preserved");

        // Extend A so its end (12) exceeds B's start — now B is pushed.
        m.work_blocks.get_mut(&a).unwrap().start_day = 0;
        m.work_blocks.get_mut(&a).unwrap().duration_days = 12;
        cascade_dependencies(&mut m, a);
        assert_eq!(
            m.work_blocks[&b].start_day, 12,
            "pushed to the bound when violated"
        );
    }

    #[test]
    fn branch_dep_never_moves_main_block_and_flags_violation() {
        // Main blocks are rocks: a branch dep can't move one. When the branch dep
        // would require moving the main successor, it's left violated (for the
        // UI to highlight) rather than dragging the rock.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let branch = m.create_plan("branch", Some(0));
        let b = placed(&mut m, "B", 5, 3); // a main block
        m.plans.get_mut(&main).unwrap().root_blocks.push(b);
        let a = placed(&mut m, "A", 0, 3); // a branch-owned block
        m.plans.get_mut(&branch).unwrap().root_blocks.push(a);
        let dep = m.create_dependency_in(branch, a, b, DependencyType::FinishToStart);

        // Extend A so it ends at 10 — the FS dep would need B at ≥ 10.
        m.work_blocks.get_mut(&a).unwrap().duration_days = 10;
        cascade_dependencies(&mut m, a);

        assert_eq!(
            m.work_blocks[&b].start_day, 5,
            "main block (rock) is not moved by a branch dep"
        );
        let dep = m.dependencies[&dep].clone();
        assert!(
            !dependency_satisfied(&m, &dep),
            "the unsatisfiable branch dep is flagged violated"
        );
    }

    #[test]
    fn cascade_follows_deps_across_plans() {
        // Cascade is global: moving a block pushes a dependent even when the dep
        // belongs to a branch — so moving a block that main shares with a branch
        // as a ghost pushes the branch's own dependent block.
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 5, 3);
        let branch = m.create_plan("branch", Some(0));
        m.create_dependency_in(branch, a, b, DependencyType::FinishToStart);

        m.work_blocks.get_mut(&a).unwrap().duration_days = 8;
        cascade_dependencies(&mut m, a);
        assert_eq!(
            m.work_blocks[&b].start_day, 8,
            "branch dep cascades when its predecessor moves"
        );
    }

    #[test]
    fn cascade_fs_pushes_successor() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 5, 3); // initially satisfies FS
        m.create_dependency(a, b, DependencyType::FinishToStart);

        // Extend A's duration — B must be pushed.
        m.work_blocks.get_mut(&a).unwrap().duration_days = 8;
        cascade_dependencies(&mut m, a);

        assert_eq!(m.work_blocks[&b].start_day, 8);
    }

    #[test]
    fn cascade_ss_pushes_successor() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 2, 4);
        let b = placed(&mut m, "B", 2, 3);
        m.create_dependency(a, b, DependencyType::StartToStart);

        m.work_blocks.get_mut(&a).unwrap().start_day = 5;
        cascade_dependencies(&mut m, a);

        assert_eq!(m.work_blocks[&b].start_day, 5);
    }

    #[test]
    fn cascade_ff_adjusts_successor_start() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5); // ends at 5
        let b = placed(&mut m, "B", 1, 4); // ends at 5 — satisfies FF
        m.create_dependency(a, b, DependencyType::FinishToFinish);

        // Extend A so it ends at 8 — B (dur=4) must start at 4 to end at 8.
        m.work_blocks.get_mut(&a).unwrap().duration_days = 8;
        cascade_dependencies(&mut m, a);

        assert_eq!(m.work_blocks[&b].start_day, 4);
    }

    #[test]
    fn cascade_sf_adjusts_successor_start() {
        // SF: succ.end >= pred.start + lag  =>  succ.start = pred.start + lag - succ.dur
        let mut m = Model::default();
        let a = placed(&mut m, "A", 4, 2);
        let b = placed(&mut m, "B", 0, 5); // end=5 >= pred.start=4 — satisfies SF
        m.create_dependency(a, b, DependencyType::StartToFinish);

        m.work_blocks.get_mut(&a).unwrap().start_day = 7;
        cascade_dependencies(&mut m, a);

        // succ.start = 7 + 0 - 5 = 2
        assert_eq!(m.work_blocks[&b].start_day, 2);
    }

    #[test]
    fn cascade_transitive_chain() {
        // A → B → C: moving A should cascade through B to C.
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 5, 3);
        let c = placed(&mut m, "C", 8, 2);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);

        m.work_blocks.get_mut(&a).unwrap().duration_days = 10;
        cascade_dependencies(&mut m, a);

        assert_eq!(m.work_blocks[&b].start_day, 10);
        assert_eq!(m.work_blocks[&c].start_day, 13);
    }

    #[test]
    fn cascade_no_successors_is_noop() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        // No dependencies.
        cascade_dependencies(&mut m, a);
        assert_eq!(m.work_blocks[&a].start_day, 0);
    }

    // ── Working-day calendar integration tests ─────────────────────────────

    #[test]
    fn forward_pass_fs_respects_integer_duration() {
        // A(4) --FS--> B(2): B must start at day 4.
        let (mut m, _) = base();
        let a = mk(&mut m, "A", 4);
        let b = mk(&mut m, "B", 2);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let s = run(&m, vec![a, b]);
        assert_eq!(s.blocks[&b].start_day, 4);
        assert_eq!(s.blocks[&b].end_day, 6);
    }

    #[test]
    fn cascade_fs_places_successor_after_predecessor() {
        // A(0→4) --FS--> B: B must start at 4.
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 4);
        let b = placed(&mut m, "B", 0, 2);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        cascade_dependencies(&mut m, a);
        assert_eq!(m.work_blocks[&b].start_day, 4);
    }

    #[test]
    fn cascade_whole_day_end_unchanged() {
        // A ends on day 5; B should start at exactly 5.
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 5, 3);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.work_blocks.get_mut(&a).unwrap().duration_days = 7;
        cascade_dependencies(&mut m, a);
        assert_eq!(m.work_blocks[&b].start_day, 7);
    }

    #[test]
    fn working_day_to_date_uses_calendar() {
        use crate::model::CalendarConfig;
        use chrono::NaiveDate;
        let config = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
            quarter_colors: Default::default(),
        };
        // 5 working days from Monday Jan 6 = Monday Jan 13 (skips weekend).
        let date = working_day_to_date(5, &config);
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 13).unwrap());
    }

    #[test]
    fn calendar_span_accounts_for_weekend() {
        use crate::model::CalendarConfig;
        use chrono::NaiveDate;
        let config = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
            quarter_colors: Default::default(),
        };
        // 5 effort days starting Monday = 7 calendar days (Mon through next Mon).
        assert_eq!(calendar_span(0, 5, &config), 7);
        // 3 effort days starting Monday = 3 calendar days (Mon, Tue, Wed).
        assert_eq!(calendar_span(0, 3, &config), 3);
        // 3 effort days starting Thursday (day 3) = 5 calendar days (Thu–Mon).
        assert_eq!(calendar_span(3, 3, &config), 5);
    }

    // ── visible_blocks tests ────────────────────────────────────────────────────

    fn placed_block(m: &mut Model, name: &str, start: Day, dur: Day) -> WorkBlockId {
        let id = m.create_work_block(name);
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = start;
        wb.duration_days = dur;
        id
    }

    #[test]
    fn visible_blocks_returns_placed_blocks_sorted() {
        let mut m = Model::default();
        let a = placed_block(&mut m, "A", 0, 2);
        let b = placed_block(&mut m, "B", 3, 1);
        // Unplaced block (duration_days == 0) must be omitted.
        let unplaced = m.create_work_block("unplaced");
        let plan = m.create_plan("p", None);
        m.plans.get_mut(&plan).unwrap().root_blocks = vec![a, b, unplaced];
        let ids: Vec<WorkBlockId> = visible_blocks(&m, plan, None)
            .iter()
            .map(|wb| wb.id)
            .collect();
        assert_eq!(ids, vec![a, b]);
    }

    #[test]
    fn visible_blocks_excludes_other_plans_blocks() {
        // A block owned by another plan (e.g. a branch) must not appear in this
        // plan's visible set, even though it lives in the same Model.
        let mut m = Model::default();
        let mine = placed_block(&mut m, "mine", 0, 2);
        let theirs = placed_block(&mut m, "theirs", 1, 2);
        let main = m.create_plan("main", None);
        let branch = m.create_plan("branch", Some(0));
        m.plans.get_mut(&main).unwrap().root_blocks = vec![mine];
        m.plans.get_mut(&branch).unwrap().root_blocks = vec![theirs];
        let ids: Vec<WorkBlockId> = visible_blocks(&m, main, None)
            .iter()
            .map(|wb| wb.id)
            .collect();
        assert_eq!(ids, vec![mine]);
    }
}
