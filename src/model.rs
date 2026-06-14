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
id_newtype!(VariantId);
id_newtype!(ResourceBlockId);
id_newtype!(DependencyId);
id_newtype!(MilestoneId);
id_newtype!(WorldId);
id_newtype!(PlanId);

/// A duration or position on the timeline, measured in working days from the plan origin.
/// Phase 1 of the f32→i32 migration: tagging all day-valued fields as `Day` makes
/// Phase 2 (flipping the alias to `i32`) a focused mechanical change.
pub type Day = f32;

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
                [0.20, 0.30, 0.60, 0.07], // Q1 - blue
                [0.20, 0.50, 0.25, 0.07], // Q2 - green
                [0.60, 0.40, 0.15, 0.07], // Q3 - amber
                [0.45, 0.20, 0.50, 0.07], // Q4 - purple
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
    pub block_positions: HashMap<WorkBlockId, (f32, f32)>,
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
    pub branch_start_day: Option<f32>,
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
                start_day: 0.0,
                duration_days: 0.0,
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
                lag: 0.0,
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
        branch_start_day: Option<f32>,
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
            most_likely: 3.0,
            optimistic: 1.0,
            pessimistic: 7.0,
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
        let ms_id = m.create_milestone("launch", 90.0);
        let block_a = m.create_work_block("a", est());
        let block_b = m.create_work_block("b", est());
        let dep_id = m.create_dependency(block_a, block_b, DependencyType::FinishToStart);

        assert_eq!(m.get_world(world_id).unwrap().name, "baseline");
        assert_eq!(m.get_plan(plan_id).unwrap().world_id, world_id);
        assert_eq!(m.get_resource_block(res_id).unwrap().name, "Alice");
        assert_eq!(m.get_milestone(ms_id).unwrap().date, 90.0);
        let dep = m.get_dependency(dep_id).unwrap();
        assert_eq!(dep.predecessor, block_a);
        assert_eq!(dep.successor, block_b);
        assert_eq!(dep.dependency_type, DependencyType::FinishToStart);
    }
}
