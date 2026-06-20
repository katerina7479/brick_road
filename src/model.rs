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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_is_empty() {
        let m = Model::default();
        assert!(m.work_blocks.is_empty());
        assert!(m.resource_blocks.is_empty());
        assert!(m.dependencies.is_empty());
        assert!(m.plans.is_empty());
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
