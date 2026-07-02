use std::collections::{HashMap, HashSet};

use bevy::prelude::Resource;
use chrono::NaiveDate;

use crate::constants::EVENTS_ROW;

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
    /// Optional user-defined HDR color [R, G, B] in linear space.
    /// Values > 1.0 trigger bloom. `None` falls back to the palette default.
    pub color: Option<[f32; 3]>,
    /// Free-form notes displayed on hover; not shown in the block bar.
    pub description: String,
    /// Optional link to an external resource (ticket, doc, …). Empty = none.
    /// Opened via Ctrl/Cmd+O or the inspector's OPEN button.
    pub url: String,
    /// User-set priority: 0=Low, 1=Normal (default), 2=High, 3=Critical.
    /// Conveyed visually as border weight on the block bar.
    pub priority: u8,
    /// Selected t-shirt size label (e.g. "M"), if any. The resolved day count
    /// is always stored in `duration_days`; this tracks which size was chosen.
    pub t_shirt_size: Option<String>,
    /// Roll-up mode: when `true` and the block has children, its `start_day` and
    /// `duration_days` are computed to span its children (a read-only summary
    /// bar). When `false`, the block keeps its own timeline and children sit
    /// inside it without resizing it. Per-block toggle; ignored for leaf blocks.
    pub rollup: bool,
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
    pub non_working_dates: Vec<NonWorkingDate>,
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
    Engineer,
    NewHire,
    Team,
    Equipment,
    Budget,
}

impl ResourceType {
    /// All variants, for type pickers.
    pub const ALL: [ResourceType; 5] = [
        ResourceType::Engineer,
        ResourceType::NewHire,
        ResourceType::Team,
        ResourceType::Equipment,
        ResourceType::Budget,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ResourceType::Engineer => "Engineer",
            ResourceType::NewHire => "New Hire",
            ResourceType::Team => "Team",
            ResourceType::Equipment => "Equipment",
            ResourceType::Budget => "Budget",
        }
    }

    /// Individual-contributor resource types (people), vs Team/Equipment/Budget.
    pub fn is_individual(self) -> bool {
        matches!(self, ResourceType::Engineer | ResourceType::NewHire)
    }
}

/// A specific date when one resource is unavailable (PTO, leave, training).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonWorkingDate {
    pub date: NaiveDate,
    /// Short label shown in the UI (e.g. "PTO", "offsite"). May be empty.
    pub description: String,
}

/// A resource that can be allocated to work blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceBlock {
    pub id: ResourceBlockId,
    pub name: String,
    pub resource_type: ResourceType,
    /// Dates this resource is unavailable, mirroring `CalendarConfig::non_working_dates`
    /// but scoped to one resource rather than the whole project.
    pub non_working_dates: Vec<NonWorkingDate>,
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
    /// The plan this dependency belongs to. Dependencies are branch-local: a dep
    /// added in a branch lane lives in that branch and never affects main, even
    /// between two ghosts. Main's own deps carry main's id.
    pub plan_id: PlanId,
    pub predecessor: WorkBlockId,
    pub successor: WorkBlockId,
    pub dependency_type: DependencyType,
}

/// A proposed future: a named scenario that selects blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub id: PlanId,
    pub name: String,
    /// Top-level work blocks in this plan (roots of the hierarchy).
    pub root_blocks: Vec<WorkBlockId>,
    /// When `Some(d)`, this plan is a future branch: block start_day is
    /// clamped to ≥ d (the working-day offset of "today" at branch creation).
    /// `None` for the baseline plan, which may contain historical blocks.
    pub branch_start_day: Option<Day>,
    /// User-given names for resource rows, keyed by drill scope: `None` is the
    /// plan's top level; `Some(block)` is the rows seen when drilled into that
    /// block. Each scope owns an independent, per-plan ordered list (index =
    /// row number). A row with no entry falls back to a default label.
    pub row_names: HashMap<Option<WorkBlockId>, Vec<String>>,
    /// The vertical lane (resource row) each block occupies *in this plan*.
    /// World-Y = `-row * ROW_HEIGHT`. Per-plan because staffing is the one
    /// dimension a branch owns independently: a fork snapshots main's rows and
    /// then diverges, and main never writes a branch's row again. Absent = 0.
    pub block_rows: HashMap<WorkBlockId, i32>,
}

impl Plan {
    /// The resource-row name for `row` within `scope` (the drilled-into block,
    /// or `None` at top level), or `None` if the user hasn't named it.
    pub fn row_name(&self, scope: Option<WorkBlockId>, row: i32) -> Option<&str> {
        self.row_names
            .get(&scope)
            .and_then(|names| usize::try_from(row).ok().and_then(|i| names.get(i)))
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }

    /// Sets the name for `row` within `scope`, growing the list with empty
    /// placeholders as needed so `row` is addressable.
    pub fn set_row_name(&mut self, scope: Option<WorkBlockId>, row: i32, name: String) {
        let Ok(idx) = usize::try_from(row) else {
            return;
        };
        let names = self.row_names.entry(scope).or_default();
        if names.len() <= idx {
            names.resize(idx + 1, String::new());
        }
        names[idx] = name;
    }
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
                color: None,
                description: String::new(),
                url: String::new(),
                priority: 1,
                t_shirt_size: None,
                rollup: false,
            },
        );
        id
    }

    /// Every distinct, non-empty resource name used across all plans' gutter
    /// rows, case-insensitively de-duplicated and sorted. These are the "named
    /// resources" the settings panel types and (for people) gives vacations.
    pub fn named_resources(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .plans
            .values()
            .flat_map(|p| p.row_names.values())
            .flatten()
            .filter(|n| !n.is_empty())
            .cloned()
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        names
    }

    /// The typed resource registered under `name` (case-insensitive), if any.
    pub fn resource_by_name(&self, name: &str) -> Option<&ResourceBlock> {
        self.resource_blocks
            .values()
            .find(|r| r.name.eq_ignore_ascii_case(name))
    }

    /// The type assigned to the resource named `name`, if it's been typed.
    pub fn resource_kind(&self, name: &str) -> Option<ResourceType> {
        self.resource_by_name(name).map(|r| r.resource_type)
    }

    /// Sets the type for `name`, creating its registry entry on first use.
    pub fn set_resource_kind(&mut self, name: &str, kind: ResourceType) {
        if let Some(r) = self
            .resource_blocks
            .values_mut()
            .find(|r| r.name.eq_ignore_ascii_case(name))
        {
            r.resource_type = kind;
        } else {
            self.create_resource_block(name.to_string(), kind);
        }
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
                non_working_dates: vec![],
            },
        );
        id
    }

    /// Creates a dependency belonging to the main plan. Convenience for the
    /// common case (and all existing callers); branch deps use
    /// [`create_dependency_in`]. Falls back to `PlanId(0)` only if no plan
    /// exists yet (the dep then matches no plan's graph until reassigned).
    pub fn create_dependency(
        &mut self,
        predecessor: WorkBlockId,
        successor: WorkBlockId,
        dependency_type: DependencyType,
    ) -> DependencyId {
        let plan_id = self.main_plan_id().unwrap_or(PlanId(0));
        self.create_dependency_in(plan_id, predecessor, successor, dependency_type)
    }

    /// Creates a dependency belonging to `plan_id`. Used for branch-local deps
    /// added in a lane.
    pub fn create_dependency_in(
        &mut self,
        plan_id: PlanId,
        predecessor: WorkBlockId,
        successor: WorkBlockId,
        dependency_type: DependencyType,
    ) -> DependencyId {
        let id = DependencyId(self.alloc_id());
        self.dependencies.insert(
            id,
            Dependency {
                id,
                plan_id,
                predecessor,
                successor,
                dependency_type,
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
                branch_start_day,
                row_names: HashMap::new(),
                block_rows: HashMap::new(),
            },
        );
        id
    }

    /// Sets the internal ID counter. Used by load_model after deserialising
    /// to ensure new IDs don't collide with any already stored in the DB.
    pub fn set_next_id(&mut self, id: u64) {
        self.next_id = id;
    }

    // --- Nesting (children / drill-in) ---

    /// The placed children of `parent` (blocks whose `parent` is `parent`,
    /// `duration_days > 0`), sorted by ascending start day then id.
    pub fn children(&self, parent: WorkBlockId) -> Vec<WorkBlockId> {
        let mut kids: Vec<&WorkBlock> = self
            .work_blocks
            .values()
            .filter(|wb| wb.parent == Some(parent) && wb.duration_days > 0)
            .collect();
        kids.sort_by(|a, b| a.start_day.cmp(&b.start_day).then(a.id.0.cmp(&b.id.0)));
        kids.into_iter().map(|wb| wb.id).collect()
    }

    /// Whether `block` has any child blocks (placed or not).
    pub fn has_children(&self, block: WorkBlockId) -> bool {
        self.work_blocks.values().any(|wb| wb.parent == Some(block))
    }

    /// Creates a child of `parent` at the given placement within `plan` and
    /// returns its id. The lane (`row`) is recorded on `plan`; timing lives on
    /// the shared block.
    pub fn add_child_block(
        &mut self,
        plan: PlanId,
        parent: WorkBlockId,
        name: impl Into<String>,
        start_day: Day,
        duration_days: Day,
        row: i32,
    ) -> WorkBlockId {
        let id = self.create_work_block(name);
        if let Some(wb) = self.work_blocks.get_mut(&id) {
            wb.parent = Some(parent);
            wb.start_day = start_day;
            wb.duration_days = duration_days;
        }
        self.set_block_row(plan, id, row);
        self.recompute_rollup(parent);
        id
    }

    /// If `block` is in roll-up mode and has children, recompute its start/
    /// duration to span its children's extent, then propagate up its ancestors.
    /// Blocks not in roll-up mode (or with no children) keep their own placement.
    pub fn recompute_rollup(&mut self, block: WorkBlockId) {
        let mut current = Some(block);
        while let Some(id) = current {
            let rollup = self
                .work_blocks
                .get(&id)
                .map(|wb| wb.rollup)
                .unwrap_or(false);
            if rollup {
                let kids = self.children(id);
                if !kids.is_empty() {
                    let start = kids
                        .iter()
                        .filter_map(|k| self.work_blocks.get(k))
                        .map(|wb| wb.start_day)
                        .min()
                        .unwrap_or(0);
                    let end = kids
                        .iter()
                        .filter_map(|k| self.work_blocks.get(k))
                        .map(|wb| wb.start_day + wb.duration_days)
                        .max()
                        .unwrap_or(0);
                    // A rolled-up parent's time extent is its children's; its row,
                    // however, belongs to its own level's resource axis (children
                    // live on a different, independent axis) and is left untouched.
                    if let Some(wb) = self.work_blocks.get_mut(&id) {
                        wb.start_day = start;
                        wb.duration_days = (end - start).max(1);
                    }
                }
            }
            current = self.work_blocks.get(&id).and_then(|wb| wb.parent);
        }
    }

    /// Returns `true` if `target` is `ancestor` itself or a transitive
    /// descendant of it (i.e., reachable by following parent links down from
    /// `ancestor`). Used by `reparent` to reject cycles.
    pub(crate) fn is_descendant_or_self(&self, target: WorkBlockId, ancestor: WorkBlockId) -> bool {
        let mut stack = vec![ancestor];
        while let Some(id) = stack.pop() {
            if id == target {
                return true;
            }
            for wb in self.work_blocks.values() {
                if wb.parent == Some(id) {
                    stack.push(wb.id);
                }
            }
        }
        false
    }

    /// Moves `block` under `new_parent`, or detaches it to top-level when
    /// `new_parent` is `None`. Returns `Err` if the move would create a cycle.
    ///
    /// `WorkBlock.parent` is global (not per-plan), so the change applies
    /// across every plan that shares the block. `root_blocks` is updated on
    /// ALL plans accordingly: adopting removes the block everywhere; detaching
    /// adds it to the main plan and propagates to branches via
    /// `link_main_block_to_branches` (same logic as creating a new top-level
    /// block).
    ///
    /// Rollup is recomputed up both the old and new parent chains.
    pub fn reparent(
        &mut self,
        block: WorkBlockId,
        new_parent: Option<WorkBlockId>,
    ) -> Result<(), &'static str> {
        if !self.work_blocks.contains_key(&block) {
            return Err("block not found");
        }
        if let Some(np) = new_parent {
            if self.is_descendant_or_self(np, block) {
                return Err("would create a cycle");
            }
        }
        let old_parent = self.work_blocks.get(&block).and_then(|wb| wb.parent);
        if old_parent.is_none() && new_parent.is_some() {
            // Top-level → child: remove from ALL plans (parent is global).
            for plan in self.plans.values_mut() {
                plan.root_blocks.retain(|&b| b != block);
            }
        } else if old_parent.is_some() && new_parent.is_none() {
            // Child → top-level: add to main and propagate to branches.
            if let Some(main_id) = self.main_plan_id() {
                if let Some(plan) = self.plans.get_mut(&main_id) {
                    if !plan.root_blocks.contains(&block) {
                        plan.root_blocks.push(block);
                    }
                }
                self.link_main_block_to_branches(block);
            }
        }
        if let Some(wb) = self.work_blocks.get_mut(&block) {
            wb.parent = new_parent;
        }
        if let Some(op) = old_parent {
            self.recompute_rollup(op);
        }
        if let Some(np) = new_parent {
            self.recompute_rollup(np);
        }
        Ok(())
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

    /// The vertical lane `block` occupies within `plan`. Defaults to `0` when
    /// the plan has no recorded lane for the block (e.g. a block that was never
    /// placed). This is the single source of truth for a block's row.
    ///
    /// The 0-default is safe in practice: every caller queries blocks that are
    /// members of the given plan and have been explicitly placed (or inherited a
    /// row at fork). A block that has never been assigned a row genuinely belongs
    /// at row 0 (the top of the grid).
    pub fn block_row(&self, plan: PlanId, block: WorkBlockId) -> i32 {
        self.plans
            .get(&plan)
            .and_then(|p| p.block_rows.get(&block).copied())
            .unwrap_or(0)
    }

    /// Sets `block`'s lane within `plan`. No-op if the plan is missing.
    pub fn set_block_row(&mut self, plan: PlanId, block: WorkBlockId, row: i32) {
        if let Some(p) = self.plans.get_mut(&plan) {
            p.block_rows.insert(block, row);
        }
    }

    /// The resource-row name shown for `row` within `scope` in `plan_id`, with
    /// inheritance: a branch repeats main's names by default (the shared "rocks
    /// in the stream") and only diverges where it sets its own. Returns `None`
    /// if neither the plan nor main has named the row.
    pub fn resolved_row_name(
        &self,
        plan_id: PlanId,
        scope: Option<WorkBlockId>,
        row: i32,
    ) -> Option<&str> {
        self.plans
            .get(&plan_id)
            .and_then(|p| p.row_name(scope, row))
    }

    /// The resource name shown for `leaf` (scope = its parent) within `plan`:
    /// the resolved row name, or the row's placeholder label when unnamed.
    /// Shared by the by-resource layout and the flow view so both group work
    /// under the same names the plan view displays.
    pub fn leaf_resource_name(
        &self,
        plan: PlanId,
        leaf: WorkBlockId,
        scope: Option<WorkBlockId>,
    ) -> String {
        let row = self.block_row(plan, leaf);
        self.resolved_row_name(plan, scope, row)
            .map(|s| s.to_string())
            .unwrap_or_else(|| default_row_label(row))
    }

    /// Whether `block` sits on the Events row within `plan`. Events are
    /// plan-local markers (external targets/milestones): they never repeat
    /// into branches — not at fork, not via new-block propagation — and an
    /// accept-as-main keeps main's events untouched.
    pub fn is_event_block(&self, plan: PlanId, block: WorkBlockId) -> bool {
        self.block_row(plan, block) == EVENTS_ROW
    }

    /// Removes from every branch any ghost whose block sits on main's Events
    /// row (membership and lane entry). Events never repeat into plans; this
    /// sweeps ghosts that predate that rule or that appeared by moving a block
    /// onto the Events row after branches inherited it. Branch-local
    /// dependencies touching an event are deliberately kept — a plan block may
    /// depend on a main event (drawn as a cross-space edge), and an old
    /// ghost-dep simply re-anchors to the main event. Returns whether anything
    /// changed (the caller persists).
    pub fn prune_event_ghosts_from_branches(&mut self) -> bool {
        let Some(main_id) = self.main_plan_id() else {
            return false;
        };
        let events: HashSet<WorkBlockId> = self.plans[&main_id]
            .root_blocks
            .iter()
            .copied()
            .filter(|id| self.is_event_block(main_id, *id))
            .collect();
        if events.is_empty() {
            return false;
        }
        let mut changed = false;
        for plan in self.plans.values_mut() {
            if plan.branch_start_day.is_none() {
                continue;
            }
            let roots_before = plan.root_blocks.len();
            plan.root_blocks.retain(|id| !events.contains(id));
            let rows_before = plan.block_rows.len();
            plan.block_rows.retain(|id, _| !events.contains(id));
            changed |=
                plan.root_blocks.len() != roots_before || plan.block_rows.len() != rows_before;
        }
        changed
    }

    /// Forks `main` into a new branch at `fork_day`. The branch inherits main's
    /// blocks from the fork day forward by copying their ids (the blocks stay
    /// shared with main); blocks before the fork are shared trunk and not
    /// copied. Events-row blocks stay main-only. Returns the new branch's id,
    /// or `None` if there is no main plan.
    pub fn fork_main(&mut self, fork_day: Day) -> Option<PlanId> {
        let main_id = self.main_plan_id()?;
        let forward: Vec<WorkBlockId> = self.plans[&main_id]
            .root_blocks
            .iter()
            .copied()
            .filter(|id| {
                !self.is_event_block(main_id, *id)
                    && self
                        .work_blocks
                        .get(id)
                        .is_some_and(|wb| wb.start_day >= fork_day)
            })
            .collect();
        let row_names = self.plans[&main_id].row_names.clone();
        // Snapshot main's lane for each inherited block. After this the branch
        // owns its rows independently — main never writes them again.
        let block_rows: HashMap<WorkBlockId, i32> = forward
            .iter()
            .map(|id| (*id, self.block_row(main_id, *id)))
            .collect();
        let n = self.plans.len() + 1;
        let new_id = self.create_plan(format!("Plan {n}"), Some(fork_day));
        let branch = self.plans.get_mut(&new_id).unwrap();
        branch.root_blocks = forward;
        branch.row_names = row_names;
        branch.block_rows = block_rows;
        Some(new_id)
    }

    /// Links a main block into every branch that forks at or before the block's
    /// start day, as a ghost (shared id).
    ///
    /// **Call only right after creating a *new* block in main.** The copy model
    /// tracks membership (a branch's `root_blocks`) but not removals, so a block
    /// absent from a branch is ambiguous: never-inherited vs removed-by-user.
    /// At creation the id is brand new (neither), so appending is unambiguous.
    /// Calling this on an already-existing block would re-add ghosts a branch
    /// had removed — don't. Only main's own blocks propagate (a branch-owned
    /// block is a no-op).
    pub fn link_main_block_to_branches(&mut self, block_id: WorkBlockId) {
        let Some(main_id) = self.main_plan_id() else {
            return;
        };
        if !self.plans[&main_id].root_blocks.contains(&block_id) {
            return;
        }
        let Some(start) = self.work_blocks.get(&block_id).map(|wb| wb.start_day) else {
            return;
        };
        // The lane to snapshot into each branch is main's current lane for the
        // block (a freshly created block defaults to row 0). Events are
        // main-local and never repeat into branches.
        let main_row = self.block_row(main_id, block_id);
        if main_row == EVENTS_ROW {
            return;
        }
        for plan in self.plans.values_mut() {
            let Some(fork) = plan.branch_start_day else {
                continue; // branches only
            };
            if start >= fork && !plan.root_blocks.contains(&block_id) {
                plan.root_blocks.push(block_id);
                plan.block_rows.insert(block_id, main_row);
            }
        }
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
        }
        if let Some(plan) = self.plans.get_mut(&plan_id) {
            plan.root_blocks.push(id);
        }
        self.set_block_row(plan_id, id, row);
        id
    }

    /// Sets a block's timeline placement: `start_day` on the shared block and
    /// the lane (`row`) within `plan_id`. No-op if missing.
    pub fn set_block_placement(
        &mut self,
        plan_id: PlanId,
        id: WorkBlockId,
        start_day: Day,
        row: i32,
    ) {
        if let Some(wb) = self.work_blocks.get_mut(&id) {
            wb.start_day = start_day;
        }
        self.set_block_row(plan_id, id, row);
    }

    /// Sets a block's duration in working days, clamped to ≥ 1. No-op if missing.
    pub fn set_block_duration(&mut self, id: WorkBlockId, duration_days: Day) {
        if let Some(wb) = self.work_blocks.get_mut(&id) {
            wb.duration_days = duration_days.max(1);
        }
    }

    /// Removes `block_id` from `plan_id`'s membership. If no surviving plan still
    /// references the block, it is fully deleted (with its dependencies and any
    /// child `parent` pointers cleared); if it's still shared with another plan
    /// (e.g. a ghost shared with main) the block itself is kept. This is the one
    /// operation behind both "delete an owned branch block" and "remove a ghost".
    pub fn remove_block_from_plan(&mut self, plan_id: PlanId, block_id: WorkBlockId) {
        if let Some(plan) = self.plans.get_mut(&plan_id) {
            plan.root_blocks.retain(|&id| id != block_id);
            plan.block_rows.remove(&block_id);
        }
        let still_rooted = self
            .plans
            .values()
            .any(|p| p.root_blocks.contains(&block_id));
        if !still_rooted {
            self.work_blocks.remove(&block_id);
            self.dependencies
                .retain(|_, d| d.predecessor != block_id && d.successor != block_id);
            for wb in self.work_blocks.values_mut() {
                if wb.parent == Some(block_id) {
                    wb.parent = None;
                }
            }
        }
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

    /// Promotes branch `branch_id` to be the new main: main adopts the branch's
    /// future and the branch is consumed.
    ///
    /// For a branch `B` forked at day `F`:
    /// - main's trunk (`start_day < F`) is preserved untouched;
    /// - `B`'s `root_blocks` (its owned blocks plus the ghosts it kept) become
    ///   main's future, **promoting** `B`'s owned blocks and **dropping** the
    ///   ghosts `B` removed;
    /// - `B`'s branch-local staffing (`block_rows`, `row_names`) and dependencies
    ///   are promoted onto main (B-wins);
    /// - `B` is then deleted (its future is now main's).
    ///
    /// No-op if `branch_id` is missing, is itself main (`branch_start_day` is
    /// `None`), or there is no main plan.
    ///
    /// **Siblings are intentionally left as-is** — a sibling may still reference a
    /// ghost main just dropped, or miss a block main just promoted; reconciling
    /// them is br-223's job. This method only guarantees the model is internally
    /// consistent enough to `save_model` (no dangling dep `plan_id`, no
    /// `block_rows`/roots referencing deleted blocks). The caller persists via
    /// `db::save_model`, per the auto-save convention.
    pub fn accept_plan_as_main(&mut self, branch_id: PlanId) {
        let Some(branch) = self.plans.get(&branch_id) else {
            return;
        };
        // A `None` fork day means this *is* a baseline plan, not a branch.
        let Some(fork_day) = branch.branch_start_day else {
            return;
        };
        let Some(main_id) = self.main_plan_id() else {
            return;
        };
        if main_id == branch_id {
            return;
        }

        // Snapshot the branch's data before we start mutating plans.
        let b_roots = branch.root_blocks.clone();
        let b_row_names = branch.row_names.clone();
        let b_block_rows = branch.block_rows.clone();

        // --- Step 1: new main membership = trunk (< F) ++ B's future, deduped. ---
        // Events ride along with the trunk: the branch never inherited them
        // (see fork_main), so their absence from B is not a removal.
        let old_main_roots = self.plans[&main_id].root_blocks.clone();
        let mut new_main_roots: Vec<WorkBlockId> = Vec::new();
        let mut seen: HashSet<WorkBlockId> = HashSet::new();
        // Trunk (and events) first, in main's existing order.
        for &id in &old_main_roots {
            let is_trunk = self
                .work_blocks
                .get(&id)
                .is_some_and(|wb| wb.start_day < fork_day);
            if (is_trunk || self.is_event_block(main_id, id)) && seen.insert(id) {
                new_main_roots.push(id);
            }
        }
        // Then B's future, in B's order (owned blocks + kept ghosts).
        for &id in &b_roots {
            if seen.insert(id) {
                new_main_roots.push(id);
            }
        }
        let new_set: HashSet<WorkBlockId> = new_main_roots.iter().copied().collect();
        // Ghosts B removed: old main blocks (>= F) no longer present in main.
        let dropped: Vec<WorkBlockId> = old_main_roots
            .iter()
            .copied()
            .filter(|id| !new_set.contains(id))
            .collect();

        // --- Step 2: promote staffing onto main and apply the new membership. ---
        {
            let main = self.plans.get_mut(&main_id).unwrap();
            // B's lane wins for its blocks; trunk keeps main's existing lane.
            for (id, row) in b_block_rows {
                main.block_rows.insert(id, row);
            }
            // Drop lane entries for blocks that are no longer main's.
            main.block_rows.retain(|id, _| new_set.contains(id));
            // B's staffing (row names) becomes main's. B was forked from main, so
            // its row_names started as a copy of main's and only diverged — trunk
            // names are preserved, B's edits win.
            main.row_names = b_row_names;
            main.root_blocks = new_main_roots;
        }

        // --- Step 3: promote branch-local dependencies, prune the dangling. ---
        for dep in self.dependencies.values_mut() {
            if dep.plan_id == branch_id {
                dep.plan_id = main_id;
            }
        }
        // A main dep is invalid if either endpoint left main. Sibling deps
        // (other plan_ids) are untouched here — that's br-223's concern.
        self.dependencies.retain(|_, d| {
            d.plan_id != main_id
                || (new_set.contains(&d.predecessor) && new_set.contains(&d.successor))
        });

        // --- Step 4: drop orphaned removed-ghosts not rooted in any plan. ---
        // A dropped block still rooted in a sibling stays alive (br-223 handles
        // it); one rooted nowhere is fully removed, mirroring the cleanup in
        // `remove_block_from_plan`.
        for id in dropped {
            let still_rooted = self.plans.values().any(|p| p.root_blocks.contains(&id));
            if !still_rooted {
                self.work_blocks.remove(&id);
                self.dependencies
                    .retain(|_, d| d.predecessor != id && d.successor != id);
                for wb in self.work_blocks.values_mut() {
                    if wb.parent == Some(id) {
                        wb.parent = None;
                    }
                }
            }
        }

        // --- Step 5: consume B. Its owned blocks are now rooted in main, so
        // `delete_plan` (which only deletes blocks rooted nowhere) keeps them. ---
        self.delete_plan(branch_id);

        // --- Step 6: rebase every remaining sibling onto the new main.
        // Pass old_main_roots so rebase can distinguish "new block from B"
        // (not in old main → inherit) from "sibling deliberately removed"
        // (in old main, sibling dropped it → don't re-add). ---
        self.rebase_siblings_onto_main(&old_main_roots);
    }

    /// Rebases every branch (sibling) onto the current state of main.
    ///
    /// Called after [`accept_plan_as_main`] rewrites main. Each sibling
    /// re-derives its membership: it inherits the new main's blocks where
    /// `start_day >= sibling.branch_start_day` (ghosts), while keeping blocks
    /// it owns (in its `root_blocks` but not in new main).
    ///
    /// Removed-ghost semantics: if a sibling had previously removed a ghost
    /// (the block was not in the sibling's `root_blocks`), it stays removed —
    /// that removal was deliberate and is preserved. `old_main_roots` (main's
    /// roots before the accept) lets us distinguish a sibling-removed ghost
    /// (was in old main, sibling dropped it) from a newly promoted block (was
    /// not in old main, block comes from the accepted branch) — only newly
    /// promoted blocks are unconditionally added as new ghosts.
    ///
    /// Newly promoted blocks (from the accepted branch) with
    /// `start_day >= sibling.branch_start_day` are added to the sibling as
    /// new ghosts, seeded with main's lane (`block_rows`).
    ///
    /// Sibling-local deps whose endpoints are no longer in the sibling's
    /// roster are pruned to keep the model internally consistent.
    pub fn rebase_siblings_onto_main(&mut self, old_main_roots: &[WorkBlockId]) {
        let Some(main_id) = self.main_plan_id() else {
            return;
        };

        let new_main_roots: Vec<WorkBlockId> = self.plans[&main_id].root_blocks.clone();
        let new_main_set: HashSet<WorkBlockId> = new_main_roots.iter().copied().collect();
        let old_main_set: HashSet<WorkBlockId> = old_main_roots.iter().copied().collect();
        let main_rows: HashMap<WorkBlockId, i32> = self.plans[&main_id].block_rows.clone();

        let sibling_ids: Vec<PlanId> = self
            .plans
            .values()
            .filter(|p| p.branch_start_day.is_some())
            .map(|p| p.id)
            .collect();

        // Collect the new root sets per sibling before mutating plans (needed
        // later for dep pruning without re-borrowing).
        let mut new_root_sets: HashMap<PlanId, HashSet<WorkBlockId>> = HashMap::new();

        for sib_id in &sibling_ids {
            let sib_id = *sib_id;
            let fork_day = self.plans[&sib_id].branch_start_day.unwrap();
            let old_roots = self.plans[&sib_id].root_blocks.clone();
            let old_roots_set: HashSet<WorkBlockId> = old_roots.iter().copied().collect();

            // Blocks the sibling owns: in its roster but not in new main.
            let owned: Vec<WorkBlockId> = old_roots
                .iter()
                .copied()
                .filter(|id| !new_main_set.contains(id))
                .collect();

            // New roster: main's ghosts (start_day >= fork_day) then owned.
            //
            // A block from new_main is inherited by this sibling only if:
            //   - it qualifies by start_day (>= fork_day), AND
            //   - it was NOT in old main (newly promoted from accepted branch → new
            //     ghost, always add), OR it IS already in the sibling's roster (was
            //     a ghost the sibling kept → preserve).
            //
            // Blocks that were in old main AND the sibling does NOT have them are
            // deliberately-removed ghosts: we honour the sibling's removal.
            let new_roots: Vec<WorkBlockId> = new_main_roots
                .iter()
                .copied()
                .filter(|id| {
                    let start_ok = self
                        .work_blocks
                        .get(id)
                        .is_some_and(|wb| wb.start_day >= fork_day);
                    if !start_ok {
                        return false;
                    }
                    let newly_promoted = !old_main_set.contains(id);
                    let sibling_kept = old_roots_set.contains(id);
                    newly_promoted || sibling_kept
                })
                .chain(owned.iter().copied())
                .collect();

            let new_set: HashSet<WorkBlockId> = new_roots.iter().copied().collect();

            let sib = self.plans.get_mut(&sib_id).unwrap();
            // Prune stale lane entries; seed newly-inherited ghosts from main's lane.
            sib.block_rows.retain(|id, _| new_set.contains(id));
            for &id in &new_roots {
                if new_main_set.contains(&id) {
                    let main_row = main_rows.get(&id).copied().unwrap_or(0);
                    sib.block_rows.entry(id).or_insert(main_row);
                }
            }
            sib.root_blocks = new_roots;

            new_root_sets.insert(sib_id, new_set);
        }

        // Prune sibling-local deps whose endpoints left the sibling's roster.
        self.dependencies.retain(|_, d| {
            if let Some(set) = new_root_sets.get(&d.plan_id) {
                set.contains(&d.predecessor) && set.contains(&d.successor)
            } else {
                true
            }
        });
    }

    /// Dev reset: removes every work block, dependency, and branch plan, leaving
    /// a single empty main plan. Returns main's id (creating one if needed).
    pub fn clear_all_work(&mut self) -> PlanId {
        self.work_blocks.clear();
        self.dependencies.clear();
        let main_id = self
            .main_plan_id()
            .unwrap_or_else(|| self.create_plan("Main", None));
        self.plans.retain(|&id, _| id == main_id);
        if let Some(main) = self.plans.get_mut(&main_id) {
            main.root_blocks.clear();
        }
        main_id
    }
}

/// The default label for resource row `row` (0-based) when the user hasn't
/// named it. The fixed Events row above the resources has its own name.
pub fn default_row_label(row: i32) -> String {
    if row == EVENTS_ROW {
        "Events".to_string()
    } else {
        format!("Resource {}", row + 1)
    }
}

/// The computed by-resource layout for one plan: one group per distinct
/// resource name (real names or placeholder row labels). A group occupies one
/// visual row per concurrent block — overlapping work stacks on sub-rows, so
/// an over-committed resource is immediately visible as a vertical pile.
#[derive(Default)]
pub struct PersonView {
    /// Gutter labels in display order: (resource name, kind when
    /// registered/typed, the group's first visual row). A group's extra
    /// sub-rows (concurrent work) carry no label of their own.
    pub rows: Vec<(String, Option<ResourceType>, i32)>,
    /// Leaf block id → its visual row (group base + overlap sub-lane).
    pub leaf_row: HashMap<WorkBlockId, i32>,
    /// The leaf ids that should be visible in by-person mode (keyed in leaf_row).
    pub visible: Vec<WorkBlockId>,
}

/// First-fit sub-lane assignment for possibly-overlapping `[start, end)`
/// intervals: items are sorted by start and each takes the first lane whose
/// previous occupant has ended. Disjoint work shares lane 0; each additional
/// simultaneous block opens the next lane down.
pub fn assign_sublanes(mut items: Vec<(WorkBlockId, Day, Day)>) -> Vec<(WorkBlockId, i32)> {
    items.sort_by_key(|(id, s, _)| (*s, id.0));
    let mut lane_ends: Vec<Day> = Vec::new();
    let mut out = Vec::new();
    for (id, s, e) in items {
        let lane = match lane_ends.iter().position(|&end| end <= s) {
            Some(l) => l,
            None => {
                lane_ends.push(Day::MIN);
                lane_ends.len() - 1
            }
        };
        lane_ends[lane] = e;
        out.push((id, lane as i32));
    }
    out
}

/// Computes the by-resource layout for `plan_id`: finds every leaf block (no
/// children) reachable from the plan's root_blocks, resolves each leaf's row
/// name via `resolved_row_name` (placeholder row labels for unnamed rows),
/// and groups distinct names into sorted row indices. Every resource gets a
/// row — teams and individuals alike, registered (typed) or not; the type
/// only drives the gutter dot.
///
/// Returns an empty `PersonView` if the plan doesn't exist.
pub fn person_view_layout(model: &Model, plan_id: PlanId) -> PersonView {
    let Some(plan) = model.plans.get(&plan_id) else {
        return PersonView::default();
    };

    // Walk plan's root_blocks depth-first to collect leaves.
    let mut leaves: Vec<WorkBlockId> = Vec::new();
    let mut stack: Vec<WorkBlockId> = plan.root_blocks.to_vec();
    while let Some(id) = stack.pop() {
        let children = model.children(id);
        if children.is_empty() {
            leaves.push(id);
        } else {
            stack.extend(children);
        }
    }

    // Resolve each leaf's resource by row name, falling back to the row's
    // placeholder label ("Resource 3") so unnamed rows still get a group —
    // placeholder or not, the work should show somewhere.
    let mut person_leaves: Vec<(WorkBlockId, String, Option<ResourceType>)> = Vec::new();
    for leaf in leaves {
        let Some(wb) = model.work_blocks.get(&leaf) else {
            continue;
        };
        let name = model.leaf_resource_name(plan_id, leaf, wb.parent);
        let kind = model.resource_kind(&name);
        person_leaves.push((leaf, name, kind));
    }

    // Collect distinct resource names, sorted case-insensitively.
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut groups: Vec<(String, Option<ResourceType>)> = Vec::new();
    for (_, name, kind) in &person_leaves {
        if seen_names.insert(name.clone()) {
            groups.push((name.clone(), *kind));
        }
    }
    groups.sort_by_key(|a| a.0.to_lowercase());

    // Stack each group's overlapping blocks on sub-rows (first-fit lanes), so
    // concurrent work piles up visibly instead of drawing on top of itself.
    let mut rows: Vec<(String, Option<ResourceType>, i32)> = Vec::new();
    let mut leaf_row: HashMap<WorkBlockId, i32> = HashMap::new();
    let mut next_row: i32 = 0;
    for (name, kind) in groups {
        let intervals: Vec<(WorkBlockId, Day, Day)> = person_leaves
            .iter()
            .filter(|(_, n, _)| *n == name)
            .filter_map(|(id, _, _)| {
                model
                    .work_blocks
                    .get(id)
                    .map(|wb| (*id, wb.start_day, wb.start_day + wb.duration_days))
            })
            .collect();
        let lanes = assign_sublanes(intervals);
        let height = lanes.iter().map(|(_, l)| l + 1).max().unwrap_or(1);
        for (id, lane) in lanes {
            leaf_row.insert(id, next_row + lane);
        }
        rows.push((name, kind, next_row));
        next_row += height;
    }

    let visible: Vec<WorkBlockId> = leaf_row.keys().copied().collect();

    PersonView {
        rows,
        leaf_row,
        visible,
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
        assert!(
            roots.contains(&on),
            "block exactly at the fork day is inherited"
        );
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
    fn fork_does_not_inherit_events_row_blocks() {
        // An event (block on the Events row) qualifies by start_day but must
        // stay main-only at fork.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let work = placed(&mut m, main, "work", 5, 5);
        let event = m.add_block_to_plan(main, "GA Launch", 10, 1, EVENTS_ROW);
        let branch = m.fork_main(0).unwrap();
        assert!(m.plans[&branch].root_blocks.contains(&work));
        assert!(
            !m.plans[&branch].root_blocks.contains(&event),
            "events must not repeat into the branch"
        );
        assert!(!m.plans[&branch].block_rows.contains_key(&event));
        assert!(m.plans[&main].root_blocks.contains(&event));
    }

    #[test]
    fn new_event_does_not_propagate_to_branches() {
        // Creating an event in main after a branch exists must not link it
        // through as a ghost.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let branch = m.fork_main(0).unwrap();
        let event = m.add_block_to_plan(main, "Beta cutoff", 5, 1, EVENTS_ROW);
        m.link_main_block_to_branches(event);
        assert!(
            !m.plans[&branch].root_blocks.contains(&event),
            "an event must not propagate to existing branches"
        );
        // A normal block from the same starting state still propagates.
        let work = placed(&mut m, main, "work", 5, 5);
        m.link_main_block_to_branches(work);
        assert!(m.plans[&branch].root_blocks.contains(&work));
    }

    #[test]
    fn prune_removes_stale_event_ghosts_from_branches() {
        // Simulate a pre-rule save: the branch inherited a block that later
        // became (or already was) a main event, plus a branch-local dep on it.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let event = placed(&mut m, main, "GA Launch", 10, 1);
        let work = placed(&mut m, main, "work", 5, 5);
        let branch = m.fork_main(0).unwrap();
        assert!(m.plans[&branch].root_blocks.contains(&event));
        m.create_dependency_in(branch, work, event, DependencyType::FinishToStart);
        // The block moves onto main's Events row.
        m.set_block_row(main, event, EVENTS_ROW);

        assert!(m.prune_event_ghosts_from_branches());

        assert!(
            !m.plans[&branch].root_blocks.contains(&event),
            "the event's ghost must leave the branch"
        );
        assert!(!m.plans[&branch].block_rows.contains_key(&event));
        assert!(
            m.dependencies
                .values()
                .any(|d| d.plan_id == branch && d.predecessor == work && d.successor == event),
            "the branch dep survives, re-anchored to the main event"
        );
        assert!(
            m.plans[&branch].root_blocks.contains(&work),
            "other ghosts stay"
        );
        assert!(
            m.plans[&main].root_blocks.contains(&event),
            "main keeps the event"
        );
        // Idempotent: a second sweep finds nothing.
        assert!(!m.prune_event_ghosts_from_branches());
    }

    #[test]
    fn accept_keeps_main_events_and_siblings_do_not_gain_them() {
        // The branch never inherited main's event, so its absence from the
        // branch is not a removal: accept must keep the event in main, and the
        // post-accept rebase must not leak it into siblings.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let event = m.add_block_to_plan(main, "GA Launch", 10, 1, EVENTS_ROW);
        let accepted = m.fork_main(0).unwrap();
        let sibling = m.fork_main(0).unwrap();

        m.accept_plan_as_main(accepted);

        assert!(
            m.work_blocks.contains_key(&event),
            "accept must not delete main's event as a dropped ghost"
        );
        assert!(m.plans[&main].root_blocks.contains(&event));
        assert_eq!(
            m.block_row(main, event),
            EVENTS_ROW,
            "the event keeps its Events-row lane"
        );
        assert!(
            !m.plans[&sibling].root_blocks.contains(&event),
            "rebase must not add the event to siblings"
        );
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
    fn new_main_block_propagates_to_branches_at_or_after_fork() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let branch = m.fork_main(10).unwrap();

        // A new main block after the fork links into the branch as a ghost.
        let after = placed(&mut m, main, "after", 15, 3);
        m.link_main_block_to_branches(after);
        assert!(m.plans[&branch].root_blocks.contains(&after));

        // One exactly at the fork day also links (consistent with fork_main's >=).
        let at = placed(&mut m, main, "at", 10, 3);
        m.link_main_block_to_branches(at);
        assert!(m.plans[&branch].root_blocks.contains(&at));

        // One before the fork does not.
        let before = placed(&mut m, main, "before", 4, 3);
        m.link_main_block_to_branches(before);
        assert!(!m.plans[&branch].root_blocks.contains(&before));
    }

    #[test]
    fn propagation_only_links_branches_after_the_block() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let early = m.fork_main(10).unwrap();
        let late = m.fork_main(50).unwrap();
        let mid = placed(&mut m, main, "mid", 20, 3);
        m.link_main_block_to_branches(mid);
        assert!(
            m.plans[&early].root_blocks.contains(&mid),
            "fork before the block gets it"
        );
        assert!(
            !m.plans[&late].root_blocks.contains(&mid),
            "fork after the block does not"
        );
    }

    #[test]
    fn propagation_ignores_branch_owned_blocks() {
        // A block owned by a branch must not propagate to other branches or main.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let a = m.fork_main(0).unwrap();
        let b = m.fork_main(0).unwrap();
        let owned = m.add_block_to_plan(a, "owned", 5, 3, 0);
        m.link_main_block_to_branches(owned);
        assert!(!m.plans[&b].root_blocks.contains(&owned));
        assert!(!m.plans[&main].root_blocks.contains(&owned));
    }

    #[test]
    fn propagation_is_idempotent_for_an_already_linked_block() {
        // Linking a block a branch already holds must not duplicate it. (This is
        // the only safe re-call; see the method docs — link is otherwise
        // creation-only because removals aren't tracked.)
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let branch = m.fork_main(10).unwrap();
        let blk = placed(&mut m, main, "blk", 20, 3);
        m.link_main_block_to_branches(blk);
        m.link_main_block_to_branches(blk);
        let count = m.plans[&branch]
            .root_blocks
            .iter()
            .filter(|&&id| id == blk)
            .count();
        assert_eq!(count, 1, "block appears once, not duplicated");
    }

    #[test]
    fn remove_owned_block_from_branch_deletes_it() {
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let branch = m.fork_main(0).unwrap();
        let owned = m.add_block_to_plan(branch, "owned", 5, 3, 0);
        m.remove_block_from_plan(branch, owned);
        assert!(!m.plans[&branch].root_blocks.contains(&owned));
        assert!(
            !m.work_blocks.contains_key(&owned),
            "owned block fully deleted"
        );
    }

    #[test]
    fn remove_ghost_from_branch_keeps_shared_block() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let shared = placed(&mut m, main, "shared", 0, 3);
        let branch = m.fork_main(0).unwrap(); // branch inherits `shared`
        m.remove_block_from_plan(branch, shared);
        assert!(
            !m.plans[&branch].root_blocks.contains(&shared),
            "ghost removed from branch"
        );
        assert!(
            m.work_blocks.contains_key(&shared),
            "shared block kept (still in main)"
        );
        assert!(m.plans[&main].root_blocks.contains(&shared));
    }

    #[test]
    fn removing_a_ghost_from_one_branch_keeps_it_in_others() {
        // A main block inherited by two branches: removing it from one branch
        // leaves it in the other branch and in main.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let shared = placed(&mut m, main, "shared", 0, 3);
        let a = m.fork_main(0).unwrap();
        let b = m.fork_main(0).unwrap();
        assert!(m.plans[&a].root_blocks.contains(&shared));
        assert!(m.plans[&b].root_blocks.contains(&shared));

        m.remove_block_from_plan(a, shared);
        assert!(
            !m.plans[&a].root_blocks.contains(&shared),
            "removed from branch a"
        );
        assert!(
            m.plans[&b].root_blocks.contains(&shared),
            "still in branch b"
        );
        assert!(
            m.plans[&main].root_blocks.contains(&shared),
            "still in main"
        );
        assert!(m.work_blocks.contains_key(&shared), "block kept");
    }

    #[test]
    fn set_block_duration_clamps_to_at_least_one() {
        let mut m = Model::default();
        let id = m.create_work_block("b");
        m.set_block_duration(id, 0);
        assert_eq!(m.work_blocks[&id].duration_days, 1);
        m.set_block_duration(id, 7);
        assert_eq!(m.work_blocks[&id].duration_days, 7);
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
        assert!(
            m.work_blocks.contains_key(&shared),
            "block shared with main kept"
        );
        assert!(
            !m.work_blocks.contains_key(&owned),
            "block only the branch owned removed"
        );
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
        assert_eq!(kept, main, "main plan is preserved");
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
    fn children_are_placed_and_sorted() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let p = m.create_work_block("p");
        let b = m.add_child_block(pl, p, "b", 10, 3, 0);
        let a = m.add_child_block(pl, p, "a", 2, 3, 0);
        m.create_work_block("unrelated");
        // A child block with no duration is excluded (unplaced).
        let _empty = m.add_child_block(pl, p, "empty", 0, 0, 1);
        assert_eq!(
            m.children(p),
            vec![a, b],
            "placed children, sorted by start"
        );
        assert!(m.has_children(p));
    }

    #[test]
    fn rollup_spans_children_when_enabled() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let p = m.create_work_block("p");
        m.work_blocks.get_mut(&p).unwrap().rollup = true;
        m.add_child_block(pl, p, "a", 5, 3, 0); // [5, 8)
        m.add_child_block(pl, p, "b", 10, 4, 1); // [10, 14)
                                                 // add_child_block recomputes the rollup: parent spans 5 -> 14.
        assert_eq!(m.work_blocks[&p].start_day, 5);
        assert_eq!(m.work_blocks[&p].duration_days, 9);
    }

    #[test]
    fn rollup_off_keeps_own_timeline() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let p = m.create_work_block("p");
        let wb = m.work_blocks.get_mut(&p).unwrap();
        wb.start_day = 0;
        wb.duration_days = 2;
        wb.rollup = false; // independent
        m.add_child_block(pl, p, "a", 5, 3, 0);
        // Not rolled up: the parent keeps its own placement.
        assert_eq!(m.work_blocks[&p].start_day, 0);
        assert_eq!(m.work_blocks[&p].duration_days, 2);
    }

    #[test]
    fn rollup_keeps_parent_row_independent_of_children() {
        // Children live on a different resource axis than the parent, so rolling
        // up spans the parent in *time* but never moves it off its own row.
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let p = m.create_work_block("p");
        m.work_blocks.get_mut(&p).unwrap().rollup = true;
        m.set_block_row(pl, p, 1);
        m.add_child_block(pl, p, "top", 0, 3, 0); // [0, 3)
        m.add_child_block(pl, p, "bottom", 4, 3, 2); // [4, 7)
        assert_eq!(m.block_row(pl, p), 1); // unchanged
        assert_eq!(m.work_blocks[&p].start_day, 0);
        assert_eq!(m.work_blocks[&p].duration_days, 7);
    }

    #[test]
    fn rollup_propagates_up_ancestors() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let gp = m.create_work_block("gp");
        m.work_blocks.get_mut(&gp).unwrap().rollup = true;
        let p = m.add_child_block(pl, gp, "p", 0, 1, 0);
        m.work_blocks.get_mut(&p).unwrap().rollup = true;
        m.add_child_block(pl, p, "leaf", 8, 4, 0); // [8, 12)
        m.recompute_rollup(p);
        // p spans its leaf (8..12); gp spans p (8..12).
        assert_eq!(m.work_blocks[&p].start_day, 8);
        assert_eq!(m.work_blocks[&p].duration_days, 4);
        assert_eq!(m.work_blocks[&gp].start_day, 8);
        assert_eq!(m.work_blocks[&gp].duration_days, 4);
    }

    // ── reparent ──────────────────────────────────────────────────────────────

    #[test]
    fn reparent_moves_child_to_new_parent() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let a = placed(&mut m, pl, "a", 0, 5);
        let b = placed(&mut m, pl, "b", 0, 5);
        let child = m.add_child_block(pl, a, "child", 0, 3, 0);
        // Move child from a to b.
        m.reparent(child, Some(b)).unwrap();
        assert_eq!(m.work_blocks[&child].parent, Some(b));
        assert!(!m.children(a).contains(&child));
        assert!(m.children(b).contains(&child));
    }

    #[test]
    fn reparent_detach_adds_to_root_blocks() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let parent = placed(&mut m, pl, "parent", 0, 5);
        let child = m.add_child_block(pl, parent, "child", 0, 3, 0);
        assert!(!m.plans[&pl].root_blocks.contains(&child));
        m.reparent(child, None).unwrap();
        assert_eq!(m.work_blocks[&child].parent, None);
        assert!(m.plans[&pl].root_blocks.contains(&child));
    }

    #[test]
    fn reparent_to_child_removes_from_root_blocks() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let parent = placed(&mut m, pl, "parent", 0, 5);
        let top = placed(&mut m, pl, "top", 0, 3);
        assert!(m.plans[&pl].root_blocks.contains(&top));
        m.reparent(top, Some(parent)).unwrap();
        assert_eq!(m.work_blocks[&top].parent, Some(parent));
        assert!(!m.plans[&pl].root_blocks.contains(&top));
    }

    #[test]
    fn reparent_rejects_self_cycle() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let a = placed(&mut m, pl, "a", 0, 5);
        assert!(m.reparent(a, Some(a)).is_err());
    }

    #[test]
    fn reparent_rejects_ancestor_as_descendant() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let gp = placed(&mut m, pl, "gp", 0, 5);
        let p = m.add_child_block(pl, gp, "p", 0, 3, 0);
        let c = m.add_child_block(pl, p, "c", 0, 2, 0);
        // Making gp a child of c would create a cycle gp→p→c→gp.
        assert!(m.reparent(gp, Some(c)).is_err());
    }

    #[test]
    fn reparent_recomputes_rollup_on_old_and_new_parent() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let old_p = placed(&mut m, pl, "old_p", 0, 10);
        let new_p = placed(&mut m, pl, "new_p", 20, 10);
        m.work_blocks.get_mut(&old_p).unwrap().rollup = true;
        m.work_blocks.get_mut(&new_p).unwrap().rollup = true;
        let child = m.add_child_block(pl, old_p, "child", 2, 4, 0);
        // old_p should now span [2,6) from rollup.
        assert_eq!(m.work_blocks[&old_p].start_day, 2);
        m.reparent(child, Some(new_p)).unwrap();
        // old_p has no children → rollup recompute is a no-op (keeps own placement).
        // new_p gains child at [2,6) → new_p should roll up to span it.
        assert_eq!(m.work_blocks[&new_p].start_day, 2);
        assert_eq!(m.work_blocks[&new_p].duration_days, 4);
    }

    #[test]
    fn reparent_cross_plan_removes_from_all_root_blocks() {
        // A top-level block shared between main and a branch must be removed
        // from BOTH plans' root_blocks when reparented to a child.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let parent = placed(&mut m, main, "parent", 0, 5);
        let shared = placed(&mut m, main, "shared", 0, 3);
        // Fork so the branch also inherits `shared`.
        let branch = m.fork_main(0).unwrap();
        assert!(m.plans[&main].root_blocks.contains(&shared));
        assert!(m.plans[&branch].root_blocks.contains(&shared));
        // Reparent `shared` to become a child of `parent`.
        m.reparent(shared, Some(parent)).unwrap();
        // Must be gone from both plans.
        assert!(!m.plans[&main].root_blocks.contains(&shared));
        assert!(!m.plans[&branch].root_blocks.contains(&shared));
        assert_eq!(m.work_blocks[&shared].parent, Some(parent));
    }

    #[test]
    fn reparent_detach_propagates_to_branches() {
        // Detaching a child to top-level adds it to main and propagates to
        // branches that start at or before the block's start_day.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let root = placed(&mut m, main, "root", 0, 5);
        let child = m.add_child_block(main, root, "child", 1, 3, 0);
        let branch = m.fork_main(0).unwrap();
        // child is a child (not in root_blocks) before the call.
        assert!(!m.plans[&main].root_blocks.contains(&child));
        m.reparent(child, None).unwrap();
        // Now top-level in main.
        assert!(m.plans[&main].root_blocks.contains(&child));
        // And propagated to the branch (start_day=1 >= fork_day=0).
        assert!(m.plans[&branch].root_blocks.contains(&child));
    }

    #[test]
    fn is_descendant_or_self_detects_transitive() {
        let mut m = Model::default();
        let pl = m.create_plan("p", None);
        let gp = placed(&mut m, pl, "gp", 0, 5);
        let p = m.add_child_block(pl, gp, "p", 0, 3, 0);
        let c = m.add_child_block(pl, p, "c", 0, 2, 0);
        assert!(m.is_descendant_or_self(gp, gp)); // self
        assert!(m.is_descendant_or_self(p, gp)); // direct child
        assert!(m.is_descendant_or_self(c, gp)); // grandchild
        assert!(!m.is_descendant_or_self(gp, c)); // ancestor is NOT a descendant
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
        let res_id = m.create_resource_block("Alice", ResourceType::Engineer);
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

    // --- accept_plan_as_main ---

    #[test]
    fn accept_promotes_owned_branch_block_into_main() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let trunk = placed(&mut m, main, "trunk", 0, 5); // < F
        let ghost = placed(&mut m, main, "ghost", 20, 5); // >= F, branch keeps
        let branch = m.fork_main(10).unwrap(); // F = 10
        let owned = m.add_block_to_plan(branch, "owned", 15, 5, 0); // branch-owned, >= F

        m.accept_plan_as_main(branch);

        let roots = &m.plans[&main].root_blocks;
        assert!(roots.contains(&owned), "owned block is promoted into main");
        assert!(
            m.work_blocks.contains_key(&owned),
            "owned block survives consume"
        );
        assert!(roots.contains(&trunk), "trunk is preserved");
        assert!(roots.contains(&ghost), "kept ghost is preserved");
    }

    #[test]
    fn accept_drops_ghost_the_branch_removed() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let keep = placed(&mut m, main, "keep", 20, 5);
        let drop = placed(&mut m, main, "drop", 30, 5);
        let branch = m.fork_main(10).unwrap(); // inherits both ghosts
        m.remove_block_from_plan(branch, drop); // branch removes one

        m.accept_plan_as_main(branch);

        let roots = &m.plans[&main].root_blocks;
        assert!(roots.contains(&keep), "kept ghost stays in main");
        assert!(!roots.contains(&drop), "removed ghost is dropped from main");
        assert!(
            !m.work_blocks.contains_key(&drop),
            "dropped ghost is deleted when no sibling holds it"
        );
    }

    #[test]
    fn accept_leaves_the_trunk_untouched() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let trunk = placed(&mut m, main, "trunk", 2, 5); // < F
        m.set_block_row(main, trunk, 3);
        let branch = m.fork_main(10).unwrap();

        m.accept_plan_as_main(branch);

        assert!(m.plans[&main].root_blocks.contains(&trunk));
        assert!(m.work_blocks.contains_key(&trunk));
        assert_eq!(m.block_row(main, trunk), 3, "trunk lane is preserved");
    }

    #[test]
    fn accept_promotes_branch_rows_and_row_names() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let ghost = placed(&mut m, main, "ghost", 20, 5);
        let branch = m.fork_main(10).unwrap();
        // Branch re-lanes the ghost and names that row.
        m.set_block_row(branch, ghost, 4);
        m.plans
            .get_mut(&branch)
            .unwrap()
            .set_row_name(None, 4, "Alice".to_string());

        m.accept_plan_as_main(branch);

        assert_eq!(m.block_row(main, ghost), 4, "branch lane promoted to main");
        assert_eq!(
            m.plans[&main].row_name(None, 4),
            Some("Alice"),
            "branch row name promoted to main"
        );
    }

    #[test]
    fn accept_promotes_branch_deps_and_prunes_dangling() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let a = placed(&mut m, main, "a", 15, 5);
        let b = placed(&mut m, main, "b", 25, 5);
        let doomed = placed(&mut m, main, "doomed", 35, 5);
        let branch = m.fork_main(10).unwrap(); // inherits a, b, doomed
                                               // A branch-local dep between two kept blocks → promoted to main.
        let kept_dep = m.create_dependency_in(branch, a, b, DependencyType::FinishToStart);
        // A branch-local dep touching the removed ghost → endpoint dropped → pruned.
        let dangling = m.create_dependency_in(branch, a, doomed, DependencyType::FinishToStart);
        m.remove_block_from_plan(branch, doomed);

        m.accept_plan_as_main(branch);

        let kept = m.dependencies.get(&kept_dep).expect("kept dep survives");
        assert_eq!(kept.plan_id, main, "branch dep is rewritten to main's id");
        assert!(
            !m.dependencies.contains_key(&dangling),
            "dep whose endpoint left main is pruned"
        );
    }

    #[test]
    fn accept_consumes_the_branch() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let _ghost = placed(&mut m, main, "ghost", 20, 5);
        let branch = m.fork_main(10).unwrap();
        let owned = m.add_block_to_plan(branch, "owned", 15, 5, 0);

        m.accept_plan_as_main(branch);

        assert!(!m.plans.contains_key(&branch), "branch plan is consumed");
        assert!(
            m.work_blocks.contains_key(&owned),
            "branch-owned block is kept (now main's)"
        );
        assert_eq!(m.main_plan_id(), Some(main), "main is still main");
    }

    #[test]
    fn accept_keeps_a_dropped_ghost_a_sibling_still_holds() {
        // A removed ghost still rooted in a sibling branch must not be hard
        // deleted — sibling reconciliation is br-223's job.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let shared = placed(&mut m, main, "shared", 20, 5);
        let accepted = m.fork_main(10).unwrap(); // inherits shared
        let sibling = m.fork_main(10).unwrap(); // also inherits shared
        m.remove_block_from_plan(accepted, shared); // accepted branch removes it

        m.accept_plan_as_main(accepted);

        assert!(
            !m.plans[&main].root_blocks.contains(&shared),
            "main drops the ghost"
        );
        assert!(
            m.plans[&sibling].root_blocks.contains(&shared),
            "sibling still holds it"
        );
        assert!(
            m.work_blocks.contains_key(&shared),
            "block stays alive because a sibling still roots it"
        );
    }

    #[test]
    fn accept_on_baseline_plan_is_a_noop() {
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let b = placed(&mut m, main, "b", 5, 5);

        m.accept_plan_as_main(main); // main has no branch_start_day → no-op

        assert_eq!(m.main_plan_id(), Some(main));
        assert!(m.plans.contains_key(&main));
        assert!(m.work_blocks.contains_key(&b));
    }

    // ── rebase_siblings_onto_main ────────────────────────────────────────────

    #[test]
    fn rebase_sibling_inherits_promoted_block_from_accepted_branch() {
        // Accepted branch B has an owned block; after accept it lands in main.
        // Sibling S (fork_day <= owned.start_day) must inherit it as a new ghost.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let accepted = m.fork_main(0).unwrap();
        let sibling = m.fork_main(0).unwrap();
        // B creates its own block (not in main, not in sibling).
        let b_owned = m.add_block_to_plan(accepted, "b_owned", 10, 5, 0);

        m.accept_plan_as_main(accepted);

        assert!(
            m.plans[&main].root_blocks.contains(&b_owned),
            "promoted block is in main"
        );
        assert!(
            m.plans[&sibling].root_blocks.contains(&b_owned),
            "sibling inherits the promoted block as a ghost"
        );
    }

    #[test]
    fn rebase_sibling_keeps_its_own_owned_block() {
        // S has a block it created itself (not in old main). After rebase it
        // must still be in S's root_blocks.
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let accepted = m.fork_main(0).unwrap();
        let sibling = m.fork_main(0).unwrap();
        let s_owned = m.add_block_to_plan(sibling, "s_owned", 15, 3, 0);

        m.accept_plan_as_main(accepted);

        assert!(
            m.plans[&sibling].root_blocks.contains(&s_owned),
            "sibling keeps its own block after rebase"
        );
    }

    #[test]
    fn rebase_sibling_removed_ghost_stays_removed() {
        // S had deliberately removed a ghost. After accept (B kept it), S's
        // removal is preserved — the block re-enters main but S stays opted-out.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let shared = placed(&mut m, main, "shared", 10, 5);
        let accepted = m.fork_main(0).unwrap(); // inherits shared, keeps it
        let sibling = m.fork_main(0).unwrap(); // inherits shared, then removes it
        m.remove_block_from_plan(sibling, shared);

        m.accept_plan_as_main(accepted); // B keeps shared → in new main

        assert!(
            m.plans[&main].root_blocks.contains(&shared),
            "shared is in new main"
        );
        assert!(
            !m.plans[&sibling].root_blocks.contains(&shared),
            "sibling's deliberate removal is preserved"
        );
    }

    #[test]
    fn rebase_sibling_ex_ghost_dropped_by_both_becomes_sibling_owned() {
        // S and B both see a ghost. B removes it (drops from new main). S
        // keeps it. After rebase: S still holds it (now S-owned, no longer a
        // ghost of main).
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let shared = placed(&mut m, main, "shared", 20, 5);
        let accepted = m.fork_main(10).unwrap(); // inherits shared
        let sibling = m.fork_main(10).unwrap(); // also inherits shared
        m.remove_block_from_plan(accepted, shared); // B removes it

        m.accept_plan_as_main(accepted);

        assert!(
            !m.plans[&main].root_blocks.contains(&shared),
            "main dropped the ghost"
        );
        assert!(
            m.plans[&sibling].root_blocks.contains(&shared),
            "sibling keeps it as its own"
        );
        assert!(
            m.work_blocks.contains_key(&shared),
            "block stays alive — sibling still roots it"
        );
    }

    #[test]
    fn rebase_sibling_forked_after_accepted_does_not_inherit_earlier_blocks() {
        // Accepted B (fork=5) has an owned block at start_day=7.
        // Sibling S (fork=10) must NOT inherit that block (7 < 10).
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let accepted = m.fork_main(5).unwrap();
        let sibling = m.fork_main(10).unwrap();
        let b_early = m.add_block_to_plan(accepted, "early", 7, 3, 0);
        let b_late = m.add_block_to_plan(accepted, "late", 12, 3, 0);

        m.accept_plan_as_main(accepted);

        assert!(
            !m.plans[&sibling].root_blocks.contains(&b_early),
            "sibling (fork=10) does not inherit block at start_day=7"
        );
        assert!(
            m.plans[&sibling].root_blocks.contains(&b_late),
            "sibling (fork=10) does inherit block at start_day=12"
        );
    }

    #[test]
    fn rebase_sibling_owned_block_coexists_with_promoted_main_block() {
        // S has its own block at start_day=20; B also has an owned block at
        // start_day=20. After accept they are different IDs and both appear in S.
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let accepted = m.fork_main(0).unwrap();
        let sibling = m.fork_main(0).unwrap();
        let s_own = m.add_block_to_plan(sibling, "s_block", 20, 5, 0);
        let b_own = m.add_block_to_plan(accepted, "b_block", 20, 5, 0);

        m.accept_plan_as_main(accepted);

        assert!(
            m.plans[&sibling].root_blocks.contains(&s_own),
            "sibling keeps its own block"
        );
        assert!(
            m.plans[&sibling].root_blocks.contains(&b_own),
            "sibling also inherits the promoted block"
        );
    }

    #[test]
    fn rebase_sibling_keeps_dep_when_block_becomes_owned() {
        // S has a branch-local dep between two ghosts. Accepted branch B removes
        // one ghost. S kept it → it becomes S-owned after rebase. Dep stays valid.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let a = placed(&mut m, main, "a", 5, 5);
        let b_block = placed(&mut m, main, "b_blk", 15, 5);
        let accepted = m.fork_main(0).unwrap();
        let sibling = m.fork_main(0).unwrap();
        // Sibling adds a dep between its two ghosts.
        let dep = m.create_dependency_in(sibling, a, b_block, DependencyType::FinishToStart);
        // Accepted branch removes b_block; sibling does not.
        m.remove_block_from_plan(accepted, b_block);

        m.accept_plan_as_main(accepted);

        // Sibling kept b_block (B's removal doesn't force sibling to drop it).
        assert!(
            m.plans[&sibling].root_blocks.contains(&b_block),
            "sibling keeps b_block as owned after B removes it from main"
        );
        // Dep is still valid — both endpoints are in sibling's roster.
        assert!(
            m.dependencies.contains_key(&dep),
            "dep remains valid: b_block is still in sibling"
        );
    }

    #[test]
    fn rebase_sibling_dep_pruned_when_both_removed_block_and_block_deleted() {
        // Both S and B remove a ghost → block is hard-deleted in accept step 4
        // (no plan roots it). Any dep referencing it is cleaned up during accept.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let a = placed(&mut m, main, "a", 5, 5);
        let shared = placed(&mut m, main, "shared", 15, 5);
        let accepted = m.fork_main(0).unwrap();
        let sibling = m.fork_main(0).unwrap();
        // Sibling removes shared, then adds a dep (tests that cleanup runs even
        // for deps whose endpoint the sibling had removed before accept).
        m.remove_block_from_plan(sibling, shared);
        let dep = m.create_dependency_in(sibling, a, shared, DependencyType::FinishToStart);
        // Accepted also removes shared — block will be hard-deleted.
        m.remove_block_from_plan(accepted, shared);

        m.accept_plan_as_main(accepted);

        assert!(
            !m.work_blocks.contains_key(&shared),
            "shared is deleted (no plan roots it)"
        );
        assert!(
            !m.dependencies.contains_key(&dep),
            "dep referencing deleted block is pruned"
        );
    }

    #[test]
    fn rebase_sibling_lane_seeded_from_main_for_new_ghost() {
        // A promoted block gets main's lane (block_row) in the sibling.
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let accepted = m.fork_main(0).unwrap();
        let sibling = m.fork_main(0).unwrap();
        let b_owned = m.add_block_to_plan(accepted, "b_owned", 10, 5, 3); // lane 3 in B

        m.accept_plan_as_main(accepted);

        // Main got B's lane (block_row = 3). Sibling should inherit that.
        assert_eq!(
            m.plans[&sibling]
                .block_rows
                .get(&b_owned)
                .copied()
                .unwrap_or(0),
            3,
            "sibling inherits main's lane for the newly promoted ghost"
        );
    }

    #[test]
    fn rebase_no_siblings_is_a_noop() {
        // Accepting the only branch (no siblings) should not panic.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let _trunk = placed(&mut m, main, "t", 0, 5);
        let accepted = m.fork_main(0).unwrap();
        let _b_own = m.add_block_to_plan(accepted, "b_own", 10, 5, 0);

        m.accept_plan_as_main(accepted); // no other branches → no-op sibling pass
        assert_eq!(m.main_plan_id(), Some(main));
    }

    #[test]
    fn ghost_row_change_is_branch_local() {
        // Reassigning a ghost's row in a branch must not touch main's row,
        // a sibling branch's row, or the WorkBlock's start_day/duration.
        let mut m = Model::default();
        let main = m.create_plan("main", None);
        let ghost = placed(&mut m, main, "g", 5, 3);
        m.set_block_row(main, ghost, 0);

        let branch_a = m.fork_main(1).unwrap();
        let branch_b = m.fork_main(1).unwrap();

        // Reassign the ghost in branch_a only.
        m.set_block_row(branch_a, ghost, 2);

        assert_eq!(m.block_row(main, ghost), 0, "main row unchanged");
        assert_eq!(m.block_row(branch_b, ghost), 0, "sibling row unchanged");
        assert_eq!(m.block_row(branch_a, ghost), 2, "branch_a row updated");
        // Shared WorkBlock timing must not change.
        assert_eq!(m.work_blocks[&ghost].start_day, 5);
        assert_eq!(m.work_blocks[&ghost].duration_days, 3);
    }

    // --- person_view_layout / is_individual ---

    #[test]
    fn is_individual_true_for_engineer_and_newhire() {
        assert!(ResourceType::Engineer.is_individual());
        assert!(ResourceType::NewHire.is_individual());
    }

    #[test]
    fn is_individual_false_for_team_equipment_budget() {
        assert!(!ResourceType::Team.is_individual());
        assert!(!ResourceType::Equipment.is_individual());
        assert!(!ResourceType::Budget.is_individual());
    }

    #[test]
    fn person_view_layout_empty_plan_returns_empty() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let pv = person_view_layout(&m, plan);
        assert!(pv.rows.is_empty());
        assert!(pv.leaf_row.is_empty());
        assert!(pv.visible.is_empty());
    }

    #[test]
    fn person_view_layout_leaf_under_engineer_included() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let block = m.add_block_to_plan(plan, "Task A", 0, 5, 0);
        m.set_resource_kind("Alice", ResourceType::Engineer);
        m.plans
            .get_mut(&plan)
            .unwrap()
            .set_row_name(None, 0, "Alice".to_string());
        let pv = person_view_layout(&m, plan);
        assert_eq!(pv.rows.len(), 1);
        assert_eq!(pv.rows[0].0, "Alice");
        assert_eq!(pv.rows[0].1, Some(ResourceType::Engineer));
        assert_eq!(pv.rows[0].2, 0, "first group starts at row 0");
        assert_eq!(pv.leaf_row.get(&block), Some(&0));
        assert!(pv.visible.contains(&block));
    }

    #[test]
    fn person_view_layout_team_assigned_included() {
        // Teams are resources too: a team-staffed plan must not render empty.
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let block = m.add_block_to_plan(plan, "Task B", 0, 5, 0);
        m.set_resource_kind("Backend Team", ResourceType::Team);
        m.plans
            .get_mut(&plan)
            .unwrap()
            .set_row_name(None, 0, "Backend Team".to_string());
        let pv = person_view_layout(&m, plan);
        assert_eq!(pv.rows.len(), 1);
        assert_eq!(pv.rows[0].0, "Backend Team");
        assert_eq!(pv.rows[0].1, Some(ResourceType::Team));
        assert!(pv.visible.contains(&block));
    }

    #[test]
    fn person_view_layout_unassigned_grouped_under_placeholder() {
        // An unnamed row's work still shows, grouped under the placeholder
        // label the plan view displays for that row.
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let block = m.add_block_to_plan(plan, "Task C", 0, 5, 2);
        let pv = person_view_layout(&m, plan);
        assert_eq!(pv.rows.len(), 1);
        assert_eq!(pv.rows[0].0, "Resource 3");
        assert_eq!(pv.rows[0].1, None, "placeholder rows carry no type");
        assert!(pv.visible.contains(&block));
    }

    #[test]
    fn person_view_layout_stacks_overlapping_work() {
        // Two blocks on one resource at the same time occupy two sub-rows;
        // a third that starts after the first ends reuses the top lane.
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        m.set_resource_kind("Alice", ResourceType::Engineer);
        m.plans
            .get_mut(&plan)
            .unwrap()
            .set_row_name(None, 0, "Alice".to_string());
        let a = m.add_block_to_plan(plan, "A", 0, 5, 0); // days 0-5
        let b = m.add_block_to_plan(plan, "B", 2, 5, 0); // overlaps A
        let c = m.add_block_to_plan(plan, "C", 5, 3, 0); // starts as A ends
        let pv = person_view_layout(&m, plan);
        assert_eq!(pv.rows.len(), 1, "one group");
        assert_eq!(pv.leaf_row[&a], 0);
        assert_eq!(pv.leaf_row[&b], 1, "concurrent work stacks below");
        assert_eq!(pv.leaf_row[&c], 0, "disjoint work reuses the top lane");
    }

    #[test]
    fn person_view_layout_group_heights_offset_later_groups() {
        // Alice has 2 concurrent blocks → her group is 2 rows tall; Zara's
        // group starts below it, not at index 1.
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        m.set_resource_kind("Alice", ResourceType::Engineer);
        m.set_resource_kind("Zara", ResourceType::Engineer);
        {
            let p = m.plans.get_mut(&plan).unwrap();
            p.set_row_name(None, 0, "Alice".to_string());
            p.set_row_name(None, 1, "Zara".to_string());
        }
        let _a1 = m.add_block_to_plan(plan, "A1", 0, 5, 0);
        let _a2 = m.add_block_to_plan(plan, "A2", 0, 5, 0);
        let z = m.add_block_to_plan(plan, "Z", 0, 5, 1);
        let pv = person_view_layout(&m, plan);
        assert_eq!(pv.rows[0].0, "Alice");
        assert_eq!(pv.rows[0].2, 0);
        assert_eq!(pv.rows[1].0, "Zara");
        assert_eq!(pv.rows[1].2, 2, "Zara starts below Alice's 2-row group");
        assert_eq!(pv.leaf_row[&z], 2);
    }

    #[test]
    fn assign_sublanes_first_fit() {
        let id = |n: u64| WorkBlockId(n);
        // [0,5) and [2,7) overlap; [5,8) fits back into lane 0.
        let lanes = assign_sublanes(vec![(id(1), 0, 5), (id(2), 2, 7), (id(3), 5, 8)]);
        let lane_of = |n: u64| lanes.iter().find(|(i, _)| *i == id(n)).unwrap().1;
        assert_eq!(lane_of(1), 0);
        assert_eq!(lane_of(2), 1);
        assert_eq!(lane_of(3), 0);
        // Three-way overlap opens a third lane.
        let lanes = assign_sublanes(vec![(id(1), 0, 9), (id(2), 1, 9), (id(3), 2, 9)]);
        assert_eq!(lanes.iter().map(|(_, l)| *l).max(), Some(2));
    }

    #[test]
    fn person_view_layout_rows_sorted_case_insensitively() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        let b0 = m.add_block_to_plan(plan, "Task 0", 0, 3, 0);
        let b1 = m.add_block_to_plan(plan, "Task 1", 0, 3, 1);
        m.set_resource_kind("Zara", ResourceType::Engineer);
        m.set_resource_kind("Alice", ResourceType::NewHire);
        {
            let p = m.plans.get_mut(&plan).unwrap();
            p.set_row_name(None, 0, "Zara".to_string());
            p.set_row_name(None, 1, "Alice".to_string());
        }
        let pv = person_view_layout(&m, plan);
        assert_eq!(pv.rows.len(), 2);
        assert_eq!(pv.rows[0].0, "Alice");
        assert_eq!(pv.rows[1].0, "Zara");
        assert_eq!(pv.leaf_row[&b0], 1, "Zara is row 1 after sort");
        assert_eq!(pv.leaf_row[&b1], 0, "Alice is row 0 after sort");
    }

    #[test]
    fn person_view_layout_container_not_a_leaf() {
        let mut m = Model::default();
        let plan = m.create_plan("main", None);
        // parent has children → it's a container, not a leaf
        let parent = m.add_block_to_plan(plan, "Container", 0, 10, 0);
        m.set_resource_kind("Alice", ResourceType::Engineer);
        // Child's scope is Some(parent), so the row name must be set there.
        let child = m.add_child_block(plan, parent, "Leaf", 0, 5, 0);
        m.plans
            .get_mut(&plan)
            .unwrap()
            .set_row_name(Some(parent), 0, "Alice".to_string());
        let pv = person_view_layout(&m, plan);
        // Only child is a leaf; parent is a container → only child visible.
        assert!(pv.visible.contains(&child));
        assert!(!pv.visible.contains(&parent));
    }
}
