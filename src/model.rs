use std::collections::HashMap;

use bevy::prelude::Resource;
use chrono::NaiveDate;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub u64);
    };
}

id_newtype!(WorkBlockId);
id_newtype!(ResourceBlockId);
id_newtype!(DependencyId);
id_newtype!(PlanId);

/// Timeline position or duration in whole working days from the plan origin.
/// Rendering boundaries must cast: `day as f32 * PIXELS_PER_DAY`.
pub type Day = i32;

/// A unit of work that carries its own duration.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkBlock {
    pub id: WorkBlockId,
    pub name: String,
    /// Optional parent block. Children of a block are blocks whose `parent`
    /// equals that block's id. `None` for top-level blocks.
    pub parent: Option<WorkBlockId>,
    /// User-defined placement: start offset in days from the plan origin.
    /// 0.0 until the user manually positions the block.
    pub start_day: Day,
    /// User-defined placement: duration in days.
    /// 0.0 until the user manually sizes the block.
    pub duration_days: Day,
    /// User-defined vertical lane. World-Y = `-row * ROW_HEIGHT`, so `0` is the
    /// baseline, negative rows sit above it and positive rows below. Freeform:
    /// set on creation and by vertical drag, never derived from sort order.
    pub row: i32,
    /// Optional user-defined HDR color [R, G, B] in linear space.
    /// Values > 1.0 trigger bloom. `None` falls back to the palette default.
    pub color: Option<[f32; 3]>,
    /// Free-form notes displayed on hover; not shown in the block bar.
    pub description: String,
    /// User-set priority: 0=Low, 1=Normal (default), 2=High, 3=Critical.
    /// Conveyed visually as border weight on the block bar.
    pub priority: u8,
    /// Selected t-shirt size label (e.g. "M"), if any. The resolved day count
    /// is always stored in `duration_days`; this tracks which size was chosen.
    pub t_shirt_size: Option<String>,
}

/// A named t-shirt size that maps a label (e.g. "M") to a day count.
#[derive(Debug, Clone, PartialEq)]
pub struct TShirtSize {
    pub label: String,
    pub days: Day,
}

/// Calendar settings for the plan: anchors "day 0" to a real date and defines
/// which days count as working days.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarConfig {
    /// The calendar date that corresponds to day 0 in the timeline.
    pub start_date: NaiveDate,
    /// Number of working days per week (1–7). Default 5 (Mon–Fri).
    pub working_days_per_week: u8,
    /// Specific calendar dates excluded from working-day counting (holidays, shutdowns).
    pub non_working_dates: Vec<NaiveDate>,
    /// RGBA fill colors for Q1–Q4 background bands. Low opacity — background context only.
    pub quarter_colors: [[f32; 4]; 4],
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            working_days_per_week: 5,
            non_working_dates: vec![],
            quarter_colors: [
                [0.22, 0.50, 0.90, 0.10], // Q1 - sky blue
                [0.25, 0.75, 0.50, 0.10], // Q2 - teal green
                [0.95, 0.65, 0.20, 0.10], // Q3 - warm amber
                [0.65, 0.35, 0.88, 0.10], // Q4 - violet
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Person,
    Team,
    Equipment,
    Budget,
}

/// A resource that can be allocated to work blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceBlock {
    pub id: ResourceBlockId,
    pub name: String,
    pub resource_type: ResourceType,
    pub availability: AvailabilityTimeline,
}

/// A contiguous span of time during which a resource is available.
/// Start and end are in days relative to the plan origin.
#[derive(Debug, Clone, PartialEq)]
pub struct AvailabilitySegment {
    pub start: Day,
    pub end: Day,
    /// Fraction of full capacity available in this segment (0.0–1.0).
    pub factor: f32,
}

/// Ordered sequence of availability segments for a resource.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AvailabilityTimeline {
    pub segments: Vec<AvailabilitySegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyType {
    FinishToStart,
    StartToStart,
    FinishToFinish,
    StartToFinish,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    pub id: DependencyId,
    pub predecessor: WorkBlockId,
    pub successor: WorkBlockId,
    pub dependency_type: DependencyType,
    /// Optional lag in days (positive = delay, negative = lead).
    pub lag: Day,
}

/// Assignment of a fraction of a resource's capacity to a work block.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceAllocation {
    pub resource_id: ResourceBlockId,
    pub work_block_id: WorkBlockId,
    /// Fraction of the resource's capacity assigned (0.0–1.0).
    pub allocation_factor: f32,
}

/// A proposed future: a named scenario that selects blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub id: PlanId,
    pub name: String,
    /// Top-level work blocks in this plan (roots of the hierarchy).
    pub root_blocks: Vec<WorkBlockId>,
    /// Resource allocations for this plan.
    pub allocations: Vec<ResourceAllocation>,
    /// When `Some(d)`, this plan is a future branch: block start_day is
    /// clamped to ≥ d (the working-day offset of "today" at branch creation).
    /// `None` for the baseline plan, which may contain historical blocks.
    pub branch_start_day: Option<Day>,
}

/// Central data store. All entities are keyed by their ID type.
/// Derives `Resource` so Bevy can manage it as an ECS resource.
#[derive(Debug, Default, Resource, PartialEq)]
pub struct Model {
    next_id: u64,
    pub work_blocks: HashMap<WorkBlockId, WorkBlock>,
    pub resource_blocks: HashMap<ResourceBlockId, ResourceBlock>,
    pub dependencies: HashMap<DependencyId, Dependency>,
    pub plans: HashMap<PlanId, Plan>,
    pub calendar: CalendarConfig,
    /// Ordered list of t-shirt sizes for estimation. Loaded from DB at startup.
    pub t_shirt_sizes: Vec<TShirtSize>,
}

impl Model {
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn create_work_block(&mut self, name: impl Into<String>) -> WorkBlockId {
        let id = WorkBlockId(self.alloc_id());
        self.work_blocks.insert(
            id,
            WorkBlock {
                id,
                name: name.into(),
                parent: None,
                start_day: 0,
                duration_days: 0,
                row: 0,
                color: None,
                description: String::new(),
                priority: 1,
                t_shirt_size: None,
            },
        );
        id
    }

    pub fn create_resource_block(
        &mut self,
        name: impl Into<String>,
        resource_type: ResourceType,
    ) -> ResourceBlockId {
        let id = ResourceBlockId(self.alloc_id());
        self.resource_blocks.insert(
            id,
            ResourceBlock {
                id,
                name: name.into(),
                resource_type,
                availability: AvailabilityTimeline::default(),
            },
        );
        id
    }

    pub fn create_dependency(
        &mut self,
        predecessor: WorkBlockId,
        successor: WorkBlockId,
        dependency_type: DependencyType,
    ) -> DependencyId {
        let id = DependencyId(self.alloc_id());
        self.dependencies.insert(
            id,
            Dependency {
                id,
                predecessor,
                successor,
                dependency_type,
                lag: 0,
            },
        );
        id
    }

    pub fn create_plan(
        &mut self,
        name: impl Into<String>,
        branch_start_day: Option<Day>,
    ) -> PlanId {
        let id = PlanId(self.alloc_id());
        self.plans.insert(
            id,
            Plan {
                id,
                name: name.into(),
                root_blocks: vec![],
                allocations: vec![],
                branch_start_day,
            },
        );
        id
    }

    /// Sets the internal ID counter. Used by load_model after deserialising
    /// to ensure new IDs don't collide with any already stored in the DB.
    pub fn set_next_id(&mut self, id: u64) {
        self.next_id = id;
    }

    // --- Accessors ---

    pub fn get_work_block(&self, id: WorkBlockId) -> Option<&WorkBlock> {
        self.work_blocks.get(&id)
    }

    pub fn get_resource_block(&self, id: ResourceBlockId) -> Option<&ResourceBlock> {
        self.resource_blocks.get(&id)
    }

    pub fn get_dependency(&self, id: DependencyId) -> Option<&Dependency> {
        self.dependencies.get(&id)
    }

    pub fn get_plan(&self, id: PlanId) -> Option<&Plan> {
        self.plans.get(&id)
    }

    // --- Plan / branch operations ---

    /// The "main" plan: the one root plan (no `branch_start_day`), lowest id for
    /// stability. Every branch forks off this. `None` if there are no plans.
    pub fn main_plan_id(&self) -> Option<PlanId> {
        self.plans
            .values()
            .filter(|p| p.branch_start_day.is_none())
            .min_by_key(|p| p.id.0)
            .map(|p| p.id)
    }

    /// Forks `main` into a new branch at `fork_day`. The branch inherits main's
    /// blocks from the fork day forward by copying their ids (the blocks stay
    /// shared with main); blocks before the fork are shared trunk and not
    /// copied. Returns the new branch's id, or `None` if there is no main plan.
    pub fn fork_main(&mut self, fork_day: Day) -> Option<PlanId> {
        let main_id = self.main_plan_id()?;
        let forward: Vec<WorkBlockId> = self.plans[&main_id]
            .root_blocks
            .iter()
            .copied()
            .filter(|id| {
                self.work_blocks
                    .get(id)
                    .is_some_and(|wb| wb.start_day >= fork_day)
            })
            .collect();
        let n = self.plans.len() + 1;
        let new_id = self.create_plan(format!("Plan {n}"), Some(fork_day));
        self.plans.get_mut(&new_id).unwrap().root_blocks = forward;
        Some(new_id)
    }

    /// Adds a new owned block to `plan_id` at the given placement and returns its
    /// id. Only that plan gains the block — other plans (e.g. main) are
    /// untouched. No-op returning the id even if the plan is missing.
    pub fn add_block_to_plan(
        &mut self,
        plan_id: PlanId,
        name: impl Into<String>,
        start_day: Day,
        duration_days: Day,
        row: i32,
    ) -> WorkBlockId {
        let id = self.create_work_block(name);
        if let Some(wb) = self.work_blocks.get_mut(&id) {
            wb.start_day = start_day;
            wb.duration_days = duration_days;
            wb.row = row;
        }
        if let Some(plan) = self.plans.get_mut(&plan_id) {
            plan.root_blocks.push(id);
        }
        id
    }

    /// Removes a plan and the work blocks it *solely* owned (not referenced by
    /// any surviving plan), along with their dependencies. Blocks shared with
    /// another plan are left intact. Any surviving block whose `parent` was a
    /// removed block has its parent cleared. No-op if `plan_id` is missing.
    pub fn delete_plan(&mut self, plan_id: PlanId) {
        let Some(plan) = self.plans.remove(&plan_id) else {
            return;
        };
        for block in plan.root_blocks {
            let still_rooted = self.plans.values().any(|p| p.root_blocks.contains(&block));
            if !still_rooted {
                self.work_blocks.remove(&block);
                self.dependencies
                    .retain(|_, d| d.predecessor != block && d.successor != block);
                for wb in self.work_blocks.values_mut() {
                    if wb.parent == Some(block) {
                        wb.parent = None;
                    }
                }
            }
        }
    }

    /// Dev reset: removes every work block, dependency, and branch plan, leaving
    /// a single empty main plan. Returns main's id (creating one if needed).
    pub fn clear_all_work(&mut self) -> PlanId {
        self.work_blocks.clear();
        self.dependencies.clear();
        let main_id = self.main_plan_id().unwrap_or_else(|| self.create_plan("Main", None));
        self.plans.retain(|&id, _| id == main_id);
        if let Some(main) = self.plans.get_mut(&main_id) {
            main.root_blocks.clear();
            main.allocations.clear();
        }
        main_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Places a block at `start`/`dur` and roots it in `plan`.
    fn placed(m: &mut Model, plan: PlanId, name: &str, start: Day, dur: Day) -> WorkBlockId {
        m.add_block_to_plan(plan, name, start, dur, 0)
    }

    #[test]
    fn default_model_is_empty() {
        let m = Model::default();
        assert!(m.work_blocks.is_empty());
        assert!(m.resource_blocks.is_empty());
        assert!(m.dependencies.is_empty());
        assert!(m.plans.is_empty());
    }

    // --- branch / plan requirements ---

    #[test]
    fn main_plan_id_is_the_root_plan() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let _branch = m.create_plan("branch", Some(10));
        assert_eq!(m.main_plan_id(), Some(main));
    }

    #[test]
    fn fork_copies_only_blocks_at_or_after_fork_day() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let before = placed(&mut m, main, "before", 5, 3);
        let on = placed(&mut m, main, "on", 20, 3);
        let after = placed(&mut m, main, "after", 40, 3);

        let branch = m.fork_main(20).unwrap();
        let roots = &m.plans[&branch].root_blocks;
        assert!(!roots.contains(&before), "pre-fork block is not inherited");
        assert!(roots.contains(&on), "block exactly at the fork day is inherited");
        assert!(roots.contains(&after), "post-fork block is inherited");
    }

    #[test]
    fn fork_is_a_branch_at_the_fork_day() {
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let branch = m.fork_main(15).unwrap();
        assert_eq!(m.plans[&branch].branch_start_day, Some(15));
        assert_ne!(m.main_plan_id(), Some(branch), "the fork is not main");
    }

    #[test]
    fn fork_shares_blocks_by_id_with_main() {
        // The branch copies ids, not blocks — a forked block is the same
        // WorkBlock as main's, so there's exactly one block, referenced twice.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let b = placed(&mut m, main, "b", 0, 3);
        let branch = m.fork_main(0).unwrap();
        assert!(m.plans[&main].root_blocks.contains(&b));
        assert!(m.plans[&branch].root_blocks.contains(&b));
        assert_eq!(m.work_blocks.len(), 1, "no duplicate block was created");
    }

    #[test]
    fn adding_a_block_to_a_branch_does_not_touch_main() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let branch = m.fork_main(0).unwrap();
        let owned = m.add_block_to_plan(branch, "owned", 5, 5, 1);
        assert!(m.plans[&branch].root_blocks.contains(&owned));
        assert!(
            !m.plans[&main].root_blocks.contains(&owned),
            "a branch-owned block must not appear in main"
        );
    }

    #[test]
    fn delete_plan_keeps_shared_blocks_removes_exclusive() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let shared = placed(&mut m, main, "shared", 0, 3);
        let branch = m.fork_main(0).unwrap(); // branch shares `shared`
        let owned = m.add_block_to_plan(branch, "owned", 5, 3, 0);

        m.delete_plan(branch);
        assert!(!m.plans.contains_key(&branch), "branch removed");
        assert!(m.work_blocks.contains_key(&shared), "block shared with main kept");
        assert!(!m.work_blocks.contains_key(&owned), "block only the branch owned removed");
        assert!(m.plans[&main].root_blocks.contains(&shared));
    }

    #[test]
    fn clear_all_keeps_one_empty_main() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        placed(&mut m, main, "a", 0, 3);
        let branch = m.fork_main(0).unwrap();
        placed(&mut m, branch, "b", 5, 3);

        let kept = m.clear_all_work();
        assert_eq!(kept, main, "main is preserved as the active plan");
        assert!(m.work_blocks.is_empty(), "all blocks wiped");
        assert!(m.dependencies.is_empty(), "all links wiped");
        assert_eq!(m.plans.len(), 1, "only main remains");
        assert!(m.plans[&main].root_blocks.is_empty(), "main is emptied");
    }

    #[test]
    fn create_and_retrieve_work_block() {
        let mut m = Model::default();
        let id = m.create_work_block("task A");
        let block = m.get_work_block(id).unwrap();
        assert_eq!(block.name, "task A");
        assert_eq!(block.id, id);
        assert!(block.parent.is_none());
    }

    #[test]
    fn ids_are_unique() {
        let mut m = Model::default();
        let a = m.create_work_block("a");
        let b = m.create_work_block("b");
        assert_ne!(a, b);
    }

    #[test]
    fn parent_linking() {
        let mut m = Model::default();
        let block_id = m.create_work_block("parent");
        let child_id = m.create_work_block("child");

        m.work_blocks.get_mut(&child_id).unwrap().parent = Some(block_id);

        let child = m.get_work_block(child_id).unwrap();
        assert_eq!(child.parent, Some(block_id));
    }

    #[test]
    fn missing_id_returns_none() {
        let m = Model::default();
        assert!(m.get_work_block(WorkBlockId(999)).is_none());
    }

    #[test]
    fn create_and_retrieve_all_entity_types() {
        let mut m = Model::default();
        let plan_id = m.create_plan("plan A", None);
        let res_id = m.create_resource_block("Alice", ResourceType::Person);
        let block_a = m.create_work_block("a");
        let block_b = m.create_work_block("b");
        let dep_id = m.create_dependency(block_a, block_b, DependencyType::FinishToStart);

        assert_eq!(m.get_plan(plan_id).unwrap().name, "plan A");
        assert_eq!(m.get_resource_block(res_id).unwrap().name, "Alice");
        let dep = m.get_dependency(dep_id).unwrap();
        assert_eq!(dep.predecessor, block_a);
        assert_eq!(dep.successor, block_b);
        assert_eq!(dep.dependency_type, DependencyType::FinishToStart);
    }
}
