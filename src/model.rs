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
