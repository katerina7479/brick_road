use std::collections::HashMap;

use bevy::prelude::Resource;

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

/// Three-point effort estimate in workdays.
#[derive(Debug, Clone)]
pub struct Estimate {
    pub most_likely: f32,
    pub optimistic: f32,
    pub pessimistic: f32,
    /// Subjective confidence that the true value falls in the given range (0.0–1.0).
    pub confidence: f32,
}

/// A unit of work. Leaf blocks carry their own estimate; blocks with variants
/// represent a choice between alternative implementations, each potentially
/// containing further child blocks.
#[derive(Debug, Clone)]
pub struct WorkBlock {
    pub id: WorkBlockId,
    pub name: String,
    /// Effort estimate for this block as a leaf. Ignored by the scheduler when
    /// `variants` is non-empty (rolled up from chosen variant's children instead).
    pub estimate: Estimate,
    /// Alternative implementations of this block (mutually exclusive).
    pub variants: Vec<VariantId>,
}

/// One alternative decomposition of a parent WorkBlock into an ordered sequence
/// of child WorkBlocks.
#[derive(Debug, Clone)]
pub struct Variant {
    pub id: VariantId,
    pub name: String,
    pub parent: WorkBlockId,
    /// Ordered child WorkBlocks that collectively implement this variant.
    pub children: Vec<WorkBlockId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Person,
    Team,
    Equipment,
    Budget,
}

/// A resource that can be allocated to work blocks.
#[derive(Debug, Clone)]
pub struct ResourceBlock {
    pub id: ResourceBlockId,
    pub name: String,
    pub resource_type: ResourceType,
    pub availability: AvailabilityTimeline,
}

/// A contiguous span of time during which a resource is available.
/// Start and end are in days relative to the plan origin.
#[derive(Debug, Clone)]
pub struct AvailabilitySegment {
    pub start: f32,
    pub end: f32,
    /// Fraction of full capacity available in this segment (0.0–1.0).
    pub factor: f32,
}

/// Ordered sequence of availability segments for a resource.
#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone)]
pub struct Dependency {
    pub id: DependencyId,
    pub predecessor: WorkBlockId,
    pub successor: WorkBlockId,
    pub dependency_type: DependencyType,
    /// Optional lag in days (positive = delay, negative = lead).
    pub lag: f32,
}

/// A significant named date in the plan timeline.
/// Date is in days relative to the plan origin.
#[derive(Debug, Clone)]
pub struct Milestone {
    pub id: MilestoneId,
    pub name: String,
    pub date: f32,
}

/// Assignment of a fraction of a resource's capacity to a work block.
#[derive(Debug, Clone)]
pub struct ResourceAllocation {
    pub resource_id: ResourceBlockId,
    pub work_block_id: WorkBlockId,
    /// Fraction of the resource's capacity assigned (0.0–1.0).
    pub allocation_factor: f32,
}

/// Shared reality: the pool of resources (people, teams, equipment, budgets)
/// that plans are evaluated against.
#[derive(Debug, Clone)]
pub struct World {
    pub id: WorldId,
    pub name: String,
    pub resource_ids: Vec<ResourceBlockId>,
}

/// A proposed future: a named scenario that selects blocks and variants
/// and evaluates them against a specific World.
#[derive(Debug, Clone)]
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
}

/// Central data store. All entities are keyed by their ID type.
/// Derives `Resource` so Bevy can manage it as an ECS resource.
#[derive(Debug, Default, Resource)]
pub struct Model {
    next_id: u64,
    pub work_blocks: HashMap<WorkBlockId, WorkBlock>,
    pub variants: HashMap<VariantId, Variant>,
    pub resource_blocks: HashMap<ResourceBlockId, ResourceBlock>,
    pub dependencies: HashMap<DependencyId, Dependency>,
    pub milestones: HashMap<MilestoneId, Milestone>,
    pub worlds: HashMap<WorldId, World>,
    pub plans: HashMap<PlanId, Plan>,
}

impl Model {
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn create_work_block(&mut self, name: impl Into<String>, estimate: Estimate) -> WorkBlockId {
        let id = WorkBlockId(self.alloc_id());
        self.work_blocks.insert(id, WorkBlock { id, name: name.into(), estimate, variants: vec![] });
        id
    }

    pub fn create_variant(&mut self, name: impl Into<String>, parent: WorkBlockId) -> VariantId {
        let id = VariantId(self.alloc_id());
        self.variants.insert(id, Variant { id, name: name.into(), parent, children: vec![] });
        id
    }

    pub fn create_resource_block(
        &mut self,
        name: impl Into<String>,
        resource_type: ResourceType,
    ) -> ResourceBlockId {
        let id = ResourceBlockId(self.alloc_id());
        self.resource_blocks.insert(id, ResourceBlock {
            id,
            name: name.into(),
            resource_type,
            availability: AvailabilityTimeline::default(),
        });
        id
    }

    pub fn create_dependency(
        &mut self,
        predecessor: WorkBlockId,
        successor: WorkBlockId,
        dependency_type: DependencyType,
    ) -> DependencyId {
        let id = DependencyId(self.alloc_id());
        self.dependencies.insert(id, Dependency {
            id,
            predecessor,
            successor,
            dependency_type,
            lag: 0.0,
        });
        id
    }

    pub fn create_milestone(&mut self, name: impl Into<String>, date: f32) -> MilestoneId {
        let id = MilestoneId(self.alloc_id());
        self.milestones.insert(id, Milestone { id, name: name.into(), date });
        id
    }

    pub fn create_world(&mut self, name: impl Into<String>) -> WorldId {
        let id = WorldId(self.alloc_id());
        self.worlds.insert(id, World { id, name: name.into(), resource_ids: vec![] });
        id
    }

    pub fn create_plan(&mut self, name: impl Into<String>, world_id: WorldId) -> PlanId {
        let id = PlanId(self.alloc_id());
        self.plans.insert(id, Plan {
            id,
            name: name.into(),
            world_id,
            root_blocks: vec![],
            selected_variants: HashMap::new(),
            allocations: vec![],
        });
        id
    }
}
