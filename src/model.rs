use std::collections::{HashMap, HashSet};

use bevy::prelude::Resource;
use chrono::NaiveDate;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub u64);
    };
}

id_newtype!(WorkBlockId);
id_newtype!(VariantId);
id_newtype!(ResourceBlockId);
id_newtype!(DependencyId);
id_newtype!(MilestoneId);
id_newtype!(WorldId);
id_newtype!(PlanId);

/// Timeline position or duration in whole working days from the plan origin.
/// Rendering boundaries must cast: `day as f32 * PIXELS_PER_DAY`.
pub type Day = i32;

/// Three-point effort estimate in workdays.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    pub most_likely: Day,
    pub optimistic: Day,
    pub pessimistic: Day,
    /// Subjective confidence that the true value falls in the given range (0.0–1.0).
    pub confidence: f32,
}

/// A unit of work. Leaf blocks carry their own estimate; blocks with variants
/// represent a choice between alternative implementations, each potentially
/// containing further child blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkBlock {
    pub id: WorkBlockId,
    pub name: String,
    /// Effort estimate for this block as a leaf. Ignored by the scheduler when
    /// `variants` is non-empty (rolled up from chosen variant's children instead).
    pub estimate: Estimate,
    /// Alternative implementations of this block (mutually exclusive).
    pub variants: Vec<VariantId>,
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

/// Per-confidence-level multipliers that control how wide the uncertainty spread
/// is relative to the most-likely duration.
/// Applied as: optimistic = duration × opt_factor, pessimistic = duration × pes_factor.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfidenceFactors {
    /// Optimistic factor at 50% confidence (default 0.5).
    pub opt_50: f32,
    /// Pessimistic factor at 50% confidence (default 2.0).
    pub pes_50: f32,
    /// Optimistic factor at 75% confidence (default 0.7).
    pub opt_75: f32,
    /// Pessimistic factor at 75% confidence (default 1.4).
    pub pes_75: f32,
}

impl Default for ConfidenceFactors {
    fn default() -> Self {
        Self { opt_50: 0.5, pes_50: 2.0, opt_75: 0.7, pes_75: 1.4 }
    }
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

/// One alternative decomposition of a parent WorkBlock into an ordered sequence
/// of child WorkBlocks.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub id: VariantId,
    pub name: String,
    pub parent: WorkBlockId,
    /// Ordered child WorkBlocks that collectively implement this variant.
    pub children: Vec<WorkBlockId>,
    /// Saved (start_day, duration_days) for each child, snapshotted when this
    /// variant is deactivated and restored when it is re-activated. Only
    /// entries for blocks that were placed (duration_days > 0) are stored.
    pub block_positions: HashMap<WorkBlockId, (Day, Day)>,
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

/// A significant named date in the plan timeline.
/// Date is in days relative to the plan origin.
#[derive(Debug, Clone, PartialEq)]
pub struct Milestone {
    pub id: MilestoneId,
    pub name: String,
    pub date: Day,
}

/// Assignment of a fraction of a resource's capacity to a work block.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceAllocation {
    pub resource_id: ResourceBlockId,
    pub work_block_id: WorkBlockId,
    /// Fraction of the resource's capacity assigned (0.0–1.0).
    pub allocation_factor: f32,
}

/// Shared reality: the pool of resources (people, teams, equipment, budgets)
/// that plans are evaluated against.
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub id: WorldId,
    pub name: String,
    pub resource_ids: Vec<ResourceBlockId>,
}

/// A proposed future: a named scenario that selects blocks and variants
/// and evaluates them against a specific World.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub id: PlanId,
    pub name: String,
    pub world_id: WorldId,
    /// Top-level work blocks in this plan (roots of the hierarchy).
    pub root_blocks: Vec<WorkBlockId>,
    /// For blocks that have variants, the selected variant in this plan.
    pub selected_variants: HashMap<WorkBlockId, VariantId>,
    /// Resource allocations for this plan.
    pub allocations: Vec<ResourceAllocation>,
    /// When `Some(d)`, this plan is a future branch: block start_day is
    /// clamped to ≥ d (the working-day offset of "today" at branch creation).
    /// `None` for the baseline plan, which may contain historical blocks.
    pub branch_start_day: Option<Day>,
    /// The plan this branch forked from, if any. A branch inherits its parent's
    /// effective blocks (live, recursively) minus `removed_inherited`, then its
    /// own `root_blocks`.
    pub parent: Option<PlanId>,
    /// Inherited block ids hidden in this branch — removed in the branch without
    /// affecting the parent.
    pub removed_inherited: HashSet<WorkBlockId>,
}

/// Central data store. All entities are keyed by their ID type.
/// Derives `Resource` so Bevy can manage it as an ECS resource.
#[derive(Debug, Default, Resource, PartialEq)]
pub struct Model {
    next_id: u64,
    pub work_blocks: HashMap<WorkBlockId, WorkBlock>,
    pub variants: HashMap<VariantId, Variant>,
    pub resource_blocks: HashMap<ResourceBlockId, ResourceBlock>,
    pub dependencies: HashMap<DependencyId, Dependency>,
    pub milestones: HashMap<MilestoneId, Milestone>,
    pub worlds: HashMap<WorldId, World>,
    pub plans: HashMap<PlanId, Plan>,
    pub calendar: CalendarConfig,
    /// Ordered list of t-shirt sizes for estimation. Loaded from DB at startup.
    pub t_shirt_sizes: Vec<TShirtSize>,
    /// User-configurable uncertainty spread factors per confidence level.
    pub confidence_factors: ConfidenceFactors,
}

impl Model {
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn create_work_block(
        &mut self,
        name: impl Into<String>,
        estimate: Estimate,
    ) -> WorkBlockId {
        let id = WorkBlockId(self.alloc_id());
        self.work_blocks.insert(
            id,
            WorkBlock {
                id,
                name: name.into(),
                estimate,
                variants: vec![],
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

    pub fn create_variant(&mut self, name: impl Into<String>, parent: WorkBlockId) -> VariantId {
        let id = VariantId(self.alloc_id());
        self.variants.insert(
            id,
            Variant {
                id,
                name: name.into(),
                parent,
                children: vec![],
                block_positions: HashMap::new(),
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

    pub fn create_milestone(&mut self, name: impl Into<String>, date: Day) -> MilestoneId {
        let id = MilestoneId(self.alloc_id());
        self.milestones.insert(
            id,
            Milestone {
                id,
                name: name.into(),
                date,
            },
        );
        id
    }

    pub fn create_world(&mut self, name: impl Into<String>) -> WorldId {
        let id = WorldId(self.alloc_id());
        self.worlds.insert(
            id,
            World {
                id,
                name: name.into(),
                resource_ids: vec![],
            },
        );
        id
    }

    pub fn create_plan(
        &mut self,
        name: impl Into<String>,
        world_id: WorldId,
        branch_start_day: Option<Day>,
    ) -> PlanId {
        let id = PlanId(self.alloc_id());
        self.plans.insert(
            id,
            Plan {
                id,
                name: name.into(),
                world_id,
                root_blocks: vec![],
                selected_variants: HashMap::new(),
                allocations: vec![],
                branch_start_day,
                parent: None,
                removed_inherited: HashSet::new(),
            },
        );
        id
    }

    /// The effective top-level blocks of a plan: its parent's effective blocks
    /// (live, recursively) from the fork point forward, minus this plan's
    /// `removed_inherited`, then its own `root_blocks`. Inherited blocks come
    /// first, then the plan's own additions.
    ///
    /// A branch diverges *forward* from its `branch_start_day`: it shares the
    /// trunk before the fork (those blocks are not part of the branch) and only
    /// inherits parent blocks that start on or after that day.
    pub fn effective_root_blocks(&self, plan_id: PlanId) -> Vec<WorkBlockId> {
        let Some(plan) = self.plans.get(&plan_id) else {
            return Vec::new();
        };
        self.effective_root_blocks_of(plan)
    }

    /// Like [`effective_root_blocks`], but resolves from a `Plan` *reference*
    /// rather than looking the plan up by id. Callers that work with a modified
    /// clone of a plan (e.g. swapping in a different `root_blocks` set before
    /// scheduling) must use this so the clone's own fields are honored; the
    /// parent chain is still resolved live from the model by id.
    pub fn effective_root_blocks_of(&self, plan: &Plan) -> Vec<WorkBlockId> {
        let mut out: Vec<WorkBlockId> = Vec::new();
        let mut seen: HashSet<WorkBlockId> = HashSet::new();
        if let Some(parent) = plan.parent {
            if parent != plan.id {
                for id in self.effective_root_blocks(parent) {
                    // Only inherit the parent's timeline from the fork point
                    // forward. Blocks before `branch_start_day` are shared trunk
                    // history and not part of this branch.
                    if let Some(fork_day) = plan.branch_start_day {
                        if let Some(wb) = self.work_blocks.get(&id) {
                            if wb.start_day < fork_day {
                                continue;
                            }
                        }
                    }
                    if !plan.removed_inherited.contains(&id) && seen.insert(id) {
                        out.push(id);
                    }
                }
            }
        }
        for &id in &plan.root_blocks {
            if seen.insert(id) {
                out.push(id);
            }
        }
        out
    }

    /// Whether `block` is a top-level block this plan inherits live from its
    /// parent branch — present in the parent's effective roots and not owned by
    /// `plan_id` itself. Plans with no parent never inherit; a block the plan
    /// has added to its own `root_blocks` is owned, not inherited. Used to
    /// decide whether deleting a block in a branch should *hide* it (add to
    /// `removed_inherited`) rather than destroy the shared block globally.
    pub fn is_inherited(&self, plan_id: PlanId, block: WorkBlockId) -> bool {
        let Some(plan) = self.plans.get(&plan_id) else {
            return false;
        };
        let Some(parent) = plan.parent else {
            return false;
        };
        if plan.root_blocks.contains(&block) {
            return false;
        }
        self.effective_root_blocks(parent).contains(&block)
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

    pub fn get_variant(&self, id: VariantId) -> Option<&Variant> {
        self.variants.get(&id)
    }

    pub fn get_resource_block(&self, id: ResourceBlockId) -> Option<&ResourceBlock> {
        self.resource_blocks.get(&id)
    }

    pub fn get_dependency(&self, id: DependencyId) -> Option<&Dependency> {
        self.dependencies.get(&id)
    }

    pub fn get_milestone(&self, id: MilestoneId) -> Option<&Milestone> {
        self.milestones.get(&id)
    }

    pub fn get_world(&self, id: WorldId) -> Option<&World> {
        self.worlds.get(&id)
    }

    pub fn get_plan(&self, id: PlanId) -> Option<&Plan> {
        self.plans.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn est() -> Estimate {
        Estimate {
            most_likely: 3,
            optimistic: 1,
            pessimistic: 7,
            confidence: 0.8,
        }
    }

    #[test]
    fn default_model_is_empty() {
        let m = Model::default();
        assert!(m.work_blocks.is_empty());
        assert!(m.variants.is_empty());
        assert!(m.resource_blocks.is_empty());
        assert!(m.dependencies.is_empty());
        assert!(m.milestones.is_empty());
        assert!(m.worlds.is_empty());
        assert!(m.plans.is_empty());
    }

    #[test]
    fn create_and_retrieve_work_block() {
        let mut m = Model::default();
        let id = m.create_work_block("task A", est());
        let block = m.get_work_block(id).unwrap();
        assert_eq!(block.name, "task A");
        assert_eq!(block.id, id);
        assert!(block.variants.is_empty());
    }

    #[test]
    fn ids_are_unique() {
        let mut m = Model::default();
        let a = m.create_work_block("a", est());
        let b = m.create_work_block("b", est());
        let v = m.create_variant("v", a);
        let w = m.create_world("w");
        assert_ne!(a, b);
        assert_ne!(a.0, v.0);
        assert_ne!(v.0, w.0);
    }

    #[test]
    fn variant_linking() {
        let mut m = Model::default();
        let block_id = m.create_work_block("parent", est());
        let var_id = m.create_variant("fast path", block_id);
        let child_id = m.create_work_block("child", est());

        m.work_blocks
            .get_mut(&block_id)
            .unwrap()
            .variants
            .push(var_id);
        m.variants.get_mut(&var_id).unwrap().children.push(child_id);

        let block = m.get_work_block(block_id).unwrap();
        assert_eq!(block.variants, vec![var_id]);

        let variant = m.get_variant(var_id).unwrap();
        assert_eq!(variant.parent, block_id);
        assert_eq!(variant.children, vec![child_id]);
    }

    #[test]
    fn missing_id_returns_none() {
        let m = Model::default();
        assert!(m.get_work_block(WorkBlockId(999)).is_none());
        assert!(m.get_variant(VariantId(999)).is_none());
        assert!(m.get_world(WorldId(999)).is_none());
    }

    #[test]
    fn create_and_retrieve_all_entity_types() {
        let mut m = Model::default();
        let world_id = m.create_world("baseline");
        let plan_id = m.create_plan("plan A", world_id, None);
        let res_id = m.create_resource_block("Alice", ResourceType::Person);
        let ms_id = m.create_milestone("launch", 90);
        let block_a = m.create_work_block("a", est());
        let block_b = m.create_work_block("b", est());
        let dep_id = m.create_dependency(block_a, block_b, DependencyType::FinishToStart);

        assert_eq!(m.get_world(world_id).unwrap().name, "baseline");
        assert_eq!(m.get_plan(plan_id).unwrap().world_id, world_id);
        assert_eq!(m.get_resource_block(res_id).unwrap().name, "Alice");
        assert_eq!(m.get_milestone(ms_id).unwrap().date, 90);
        let dep = m.get_dependency(dep_id).unwrap();
        assert_eq!(dep.predecessor, block_a);
        assert_eq!(dep.successor, block_b);
        assert_eq!(dep.dependency_type, DependencyType::FinishToStart);
    }

    /// Places a block at `start_day` and roots it in `plan`.
    fn place(m: &mut Model, plan: PlanId, name: &str, start: Day) -> WorkBlockId {
        let id = m.create_work_block(name, est());
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = start;
        wb.duration_days = 5;
        m.plans.get_mut(&plan).unwrap().root_blocks.push(id);
        id
    }

    /// A branch inherits the parent's blocks from the fork point forward, but
    /// not the shared trunk before it.
    #[test]
    fn branch_inherits_only_forward_of_fork() {
        let mut m = Model::default();
        let w = m.create_world("w");
        let main = m.create_plan("main", w, None);
        let before = place(&mut m, main, "before", 0);
        let after = place(&mut m, main, "after", 10);

        let branch = m.create_plan("branch", w, Some(5));
        m.plans.get_mut(&branch).unwrap().parent = Some(main);
        let own = place(&mut m, branch, "own", 8);

        let eff = m.effective_root_blocks(branch);
        assert!(!eff.contains(&before), "trunk before the fork is not inherited");
        assert!(eff.contains(&after), "blocks at/after the fork are inherited");
        assert!(eff.contains(&own), "branch's own blocks are included");
    }

    /// `removed_inherited` hides an inherited block in the branch only.
    #[test]
    fn branch_can_hide_inherited_block() {
        let mut m = Model::default();
        let w = m.create_world("w");
        let main = m.create_plan("main", w, None);
        let a = place(&mut m, main, "a", 10);
        let b = place(&mut m, main, "b", 12);

        let branch = m.create_plan("branch", w, Some(0));
        m.plans.get_mut(&branch).unwrap().parent = Some(main);
        m.plans.get_mut(&branch).unwrap().removed_inherited.insert(a);

        let eff = m.effective_root_blocks(branch);
        assert!(!eff.contains(&a), "hidden inherited block is excluded from branch");
        assert!(eff.contains(&b), "other inherited blocks remain");
        // The parent is untouched — hiding is per-branch.
        assert!(m.effective_root_blocks(main).contains(&a));
    }

    /// `is_inherited` is true only for top-level blocks a branch shares live
    /// with its parent, never for the parent's own view or owned additions.
    #[test]
    fn is_inherited_distinguishes_shared_from_owned() {
        let mut m = Model::default();
        let w = m.create_world("w");
        let main = m.create_plan("main", w, None);
        let shared = place(&mut m, main, "shared", 10);

        let branch = m.create_plan("branch", w, Some(0));
        m.plans.get_mut(&branch).unwrap().parent = Some(main);
        let owned = place(&mut m, branch, "owned", 11);

        assert!(m.is_inherited(branch, shared), "shared parent block is inherited");
        assert!(!m.is_inherited(branch, owned), "branch's own block is not inherited");
        assert!(!m.is_inherited(main, shared), "a root plan never inherits");
    }
}
