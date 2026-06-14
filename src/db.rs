use std::collections::HashMap;

use rusqlite::{Connection, Result};

use crate::model::{
    AvailabilitySegment, AvailabilityTimeline, Dependency, DependencyId, DependencyType, Estimate,
    Milestone, MilestoneId, Model, Plan, PlanId, ResourceAllocation, ResourceBlock,
    ResourceBlockId, ResourceType, Variant, VariantId, WorkBlock, WorkBlockId, World, WorldId,
};

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(CREATE_TABLES_SQL)?;
    // SQLite has no ADD COLUMN IF NOT EXISTS. Run each migration and ignore
    // the "duplicate column name" error that fires when it already exists.
    for sql in [
        "ALTER TABLE work_blocks ADD COLUMN start_day REAL NOT NULL DEFAULT 0",
        "ALTER TABLE work_blocks ADD COLUMN duration_days REAL NOT NULL DEFAULT 0",
    ] {
        match conn.execute_batch(sql) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Persists the complete Model to SQLite in a single transaction.
///
/// All existing rows are deleted and reinserted, so the DB reflects
/// the model exactly after this call (including deletions).
pub fn save_model(conn: &Connection, model: &Model) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    // Clear in reverse FK order so no constraint is violated.
    tx.execute_batch(
        "
        DELETE FROM plan_milestone_targets;
        DELETE FROM resource_allocations;
        DELETE FROM plan_variant_selections;
        DELETE FROM plan_root_blocks;
        DELETE FROM plans;
        DELETE FROM milestones;
        DELETE FROM dependencies;
        DELETE FROM variant_children;
        DELETE FROM variants;
        DELETE FROM work_blocks;
        DELETE FROM availability_segments;
        DELETE FROM resource_blocks;
        DELETE FROM worlds;
    ",
    )?;

    // worlds
    for world in model.worlds.values() {
        tx.execute(
            "INSERT INTO worlds (id, name) VALUES (?1, ?2)",
            (world.id.0, &world.name),
        )?;
    }

    // Build resource_block_id → world_id from World.resource_ids.
    let mut rb_to_world: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for world in model.worlds.values() {
        for &rb_id in &world.resource_ids {
            rb_to_world.insert(rb_id.0, world.id.0);
        }
    }

    // resource_blocks + availability_segments
    for rb in model.resource_blocks.values() {
        let world_id = rb_to_world.get(&rb.id.0).copied().ok_or_else(|| {
            rusqlite::Error::InvalidParameterName(format!(
                "ResourceBlock {} is not in any World.resource_ids",
                rb.id.0
            ))
        })?;
        tx.execute(
            "INSERT INTO resource_blocks (id, world_id, name, resource_type)
             VALUES (?1, ?2, ?3, ?4)",
            (
                rb.id.0,
                world_id,
                &rb.name,
                resource_type_str(rb.resource_type),
            ),
        )?;
        for (order, seg) in rb.availability.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO availability_segments
                     (resource_block_id, start_day, end_day, factor, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    rb.id.0,
                    seg.start as f64,
                    seg.end as f64,
                    seg.factor as f64,
                    order as i64,
                ),
            )?;
        }
    }

    // work_blocks
    for wb in model.work_blocks.values() {
        tx.execute(
            "INSERT INTO work_blocks
                 (id, name, estimate_most_likely, estimate_optimistic,
                  estimate_pessimistic, estimate_confidence,
                  start_day, duration_days)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                wb.id.0,
                &wb.name,
                wb.estimate.most_likely as f64,
                wb.estimate.optimistic as f64,
                wb.estimate.pessimistic as f64,
                wb.estimate.confidence as f64,
                wb.start_day as f64,
                wb.duration_days as f64,
            ),
        )?;
    }

    // Build variant → sort_order from each WorkBlock's ordered variants vec.
    let mut variant_sort_order: std::collections::HashMap<u64, i64> =
        std::collections::HashMap::new();
    for wb in model.work_blocks.values() {
        for (order, &var_id) in wb.variants.iter().enumerate() {
            variant_sort_order.insert(var_id.0, order as i64);
        }
    }

    // variants + variant_children
    for v in model.variants.values() {
        let order = variant_sort_order.get(&v.id.0).copied().unwrap_or(0);
        tx.execute(
            "INSERT INTO variants (id, name, parent_work_block_id, sort_order)
             VALUES (?1, ?2, ?3, ?4)",
            (v.id.0, &v.name, v.parent.0, order),
        )?;
        for (order, &child_id) in v.children.iter().enumerate() {
            tx.execute(
                "INSERT INTO variant_children (variant_id, child_work_block_id, sort_order)
                 VALUES (?1, ?2, ?3)",
                (v.id.0, child_id.0, order as i64),
            )?;
        }
    }

    // dependencies
    for dep in model.dependencies.values() {
        tx.execute(
            "INSERT INTO dependencies
                 (id, predecessor_id, successor_id, dependency_type, lag_days)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                dep.id.0,
                dep.predecessor.0,
                dep.successor.0,
                dependency_type_str(dep.dependency_type),
                dep.lag as f64,
            ),
        )?;
    }

    // milestones
    for ms in model.milestones.values() {
        tx.execute(
            "INSERT INTO milestones (id, name, date_day) VALUES (?1, ?2, ?3)",
            (ms.id.0, &ms.name, ms.date as f64),
        )?;
    }

    // plans + child join tables
    for plan in model.plans.values() {
        tx.execute(
            "INSERT INTO plans (id, name, world_id) VALUES (?1, ?2, ?3)",
            (plan.id.0, &plan.name, plan.world_id.0),
        )?;

        for (order, &wb_id) in plan.root_blocks.iter().enumerate() {
            tx.execute(
                "INSERT INTO plan_root_blocks (plan_id, work_block_id, sort_order)
                 VALUES (?1, ?2, ?3)",
                (plan.id.0, wb_id.0, order as i64),
            )?;
        }

        for (&wb_id, &var_id) in &plan.selected_variants {
            tx.execute(
                "INSERT INTO plan_variant_selections (plan_id, work_block_id, variant_id)
                 VALUES (?1, ?2, ?3)",
                (plan.id.0, wb_id.0, var_id.0),
            )?;
        }

        for alloc in &plan.allocations {
            tx.execute(
                "INSERT INTO resource_allocations
                     (plan_id, resource_block_id, work_block_id, allocation_factor)
                 VALUES (?1, ?2, ?3, ?4)",
                (
                    plan.id.0,
                    alloc.resource_id.0,
                    alloc.work_block_id.0,
                    alloc.allocation_factor as f64,
                ),
            )?;
        }
    }

    tx.commit()
}

/// Reconstructs a complete Model from SQLite.
///
/// Restores `next_id` to `max(all persisted IDs) + 1` so the first
/// `create_*` call on the reloaded model cannot produce a duplicate ID.
pub fn load_model(conn: &Connection) -> Result<Model> {
    let mut model = Model::default();
    let mut max_id: u64 = 0;

    // Helper: track largest ID seen so far.
    macro_rules! bump {
        ($id:expr) => {
            let id = $id as u64;
            if id >= max_id {
                max_id = id + 1;
            }
        };
    }

    // worlds
    {
        let mut stmt = conn.prepare("SELECT id, name FROM worlds")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            bump!(id);
            model.worlds.insert(
                WorldId(id as u64),
                World {
                    id: WorldId(id as u64),
                    name,
                    resource_ids: vec![],
                },
            );
        }
    }

    // resource_blocks  (also populate world.resource_ids; ORDER BY id keeps the vec deterministic)
    {
        let mut stmt = conn
            .prepare("SELECT id, world_id, name, resource_type FROM resource_blocks ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, world_id, name, rt_str) = row?;
            let resource_type = parse_resource_type(&rt_str)?;
            bump!(id);
            if let Some(world) = model.worlds.get_mut(&WorldId(world_id as u64)) {
                world.resource_ids.push(ResourceBlockId(id as u64));
            }
            model.resource_blocks.insert(
                ResourceBlockId(id as u64),
                ResourceBlock {
                    id: ResourceBlockId(id as u64),
                    name,
                    resource_type,
                    availability: AvailabilityTimeline::default(),
                },
            );
        }
    }

    // availability_segments  (ORDER BY guarantees segment ordering)
    {
        let mut stmt = conn.prepare(
            "SELECT resource_block_id, start_day, end_day, factor
             FROM availability_segments
             ORDER BY resource_block_id, sort_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        for row in rows {
            let (rb_id, start, end, factor) = row?;
            if let Some(rb) = model
                .resource_blocks
                .get_mut(&ResourceBlockId(rb_id as u64))
            {
                rb.availability.segments.push(AvailabilitySegment {
                    start: start as f32,
                    end: end as f32,
                    factor: factor as f32,
                });
            }
        }
    }

    // work_blocks
    {
        let mut stmt = conn.prepare(
            "SELECT id, name, estimate_most_likely, estimate_optimistic,
                    estimate_pessimistic, estimate_confidence,
                    start_day, duration_days
             FROM work_blocks",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, f64>(7)?,
            ))
        })?;
        for row in rows {
            let (id, name, ml, opt, pes, conf, start_day, duration_days) = row?;
            bump!(id);
            model.work_blocks.insert(
                WorkBlockId(id as u64),
                WorkBlock {
                    id: WorkBlockId(id as u64),
                    name,
                    estimate: Estimate {
                        most_likely: ml as f32,
                        optimistic: opt as f32,
                        pessimistic: pes as f32,
                        confidence: conf as f32,
                    },
                    variants: vec![],
                    start_day: start_day as f32,
                    duration_days: duration_days as f32,
                },
            );
        }
    }

    // variants — ORDER BY preserves wb.variants ordering; children populated below
    {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_work_block_id
             FROM variants
             ORDER BY parent_work_block_id, sort_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, parent_id) = row?;
            bump!(id);
            let var_id = VariantId(id as u64);
            let parent = WorkBlockId(parent_id as u64);
            model.variants.insert(
                var_id,
                Variant {
                    id: var_id,
                    name,
                    parent,
                    children: vec![],
                },
            );
            if let Some(wb) = model.work_blocks.get_mut(&parent) {
                wb.variants.push(var_id);
            }
        }
    }

    // variant_children → populate variant.children (order preserved)
    {
        let mut stmt = conn.prepare(
            "SELECT variant_id, child_work_block_id
             FROM variant_children
             ORDER BY variant_id, sort_order",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (var_id, child_id) = row?;
            if let Some(v) = model.variants.get_mut(&VariantId(var_id as u64)) {
                v.children.push(WorkBlockId(child_id as u64));
            }
        }
    }

    // dependencies
    {
        let mut stmt = conn.prepare(
            "SELECT id, predecessor_id, successor_id, dependency_type, lag_days
             FROM dependencies",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?;
        for row in rows {
            let (id, pred, succ, dt_str, lag) = row?;
            let dependency_type = parse_dependency_type(&dt_str)?;
            bump!(id);
            model.dependencies.insert(
                DependencyId(id as u64),
                Dependency {
                    id: DependencyId(id as u64),
                    predecessor: WorkBlockId(pred as u64),
                    successor: WorkBlockId(succ as u64),
                    dependency_type,
                    lag: lag as f32,
                },
            );
        }
    }

    // milestones
    {
        let mut stmt = conn.prepare("SELECT id, name, date_day FROM milestones")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, date) = row?;
            bump!(id);
            model.milestones.insert(
                MilestoneId(id as u64),
                Milestone {
                    id: MilestoneId(id as u64),
                    name,
                    date: date as f32,
                },
            );
        }
    }

    // plans
    {
        let mut stmt = conn.prepare("SELECT id, name, world_id FROM plans")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, world_id) = row?;
            bump!(id);
            model.plans.insert(
                PlanId(id as u64),
                Plan {
                    id: PlanId(id as u64),
                    name,
                    world_id: WorldId(world_id as u64),
                    root_blocks: vec![],
                    selected_variants: HashMap::new(),
                    allocations: vec![],
                },
            );
        }
    }

    // plan_root_blocks (order preserved)
    {
        let mut stmt = conn.prepare(
            "SELECT plan_id, work_block_id
             FROM plan_root_blocks
             ORDER BY plan_id, sort_order",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (plan_id, wb_id) = row?;
            if let Some(plan) = model.plans.get_mut(&PlanId(plan_id as u64)) {
                plan.root_blocks.push(WorkBlockId(wb_id as u64));
            }
        }
    }

    // plan_variant_selections
    {
        let mut stmt =
            conn.prepare("SELECT plan_id, work_block_id, variant_id FROM plan_variant_selections")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (plan_id, wb_id, var_id) = row?;
            if let Some(plan) = model.plans.get_mut(&PlanId(plan_id as u64)) {
                plan.selected_variants
                    .insert(WorkBlockId(wb_id as u64), VariantId(var_id as u64));
            }
        }
    }

    // resource_allocations
    {
        let mut stmt = conn.prepare(
            "SELECT plan_id, resource_block_id, work_block_id, allocation_factor
             FROM resource_allocations",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        for row in rows {
            let (plan_id, rb_id, wb_id, factor) = row?;
            if let Some(plan) = model.plans.get_mut(&PlanId(plan_id as u64)) {
                plan.allocations.push(ResourceAllocation {
                    resource_id: ResourceBlockId(rb_id as u64),
                    work_block_id: WorkBlockId(wb_id as u64),
                    allocation_factor: factor as f32,
                });
            }
        }
    }

    model.set_next_id(max_id);
    validate_model(&model)?;
    Ok(model)
}

/// Checks referential integrity invariants that the DB's FK constraints don't
/// fully capture in-memory.  Called automatically by `load_model`; callers may
/// also invoke it after mutating a Model in application code.
pub fn validate_model(model: &Model) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    for (wb_id, wb) in &model.work_blocks {
        for &var_id in &wb.variants {
            if !model.variants.contains_key(&var_id) {
                errors.push(format!(
                    "WorkBlock {} lists Variant {} which does not exist",
                    wb_id.0, var_id.0
                ));
            }
        }
    }

    for (var_id, v) in &model.variants {
        if !model.work_blocks.contains_key(&v.parent) {
            errors.push(format!(
                "Variant {} has parent WorkBlock {} which does not exist",
                var_id.0, v.parent.0
            ));
        }
        for &child_id in &v.children {
            if !model.work_blocks.contains_key(&child_id) {
                errors.push(format!(
                    "Variant {} child WorkBlock {} does not exist",
                    var_id.0, child_id.0
                ));
            }
        }
    }

    for (dep_id, dep) in &model.dependencies {
        if !model.work_blocks.contains_key(&dep.predecessor) {
            errors.push(format!(
                "Dependency {} predecessor WorkBlock {} does not exist",
                dep_id.0, dep.predecessor.0
            ));
        }
        if !model.work_blocks.contains_key(&dep.successor) {
            errors.push(format!(
                "Dependency {} successor WorkBlock {} does not exist",
                dep_id.0, dep.successor.0
            ));
        }
    }

    for (world_id, world) in &model.worlds {
        for &rb_id in &world.resource_ids {
            if !model.resource_blocks.contains_key(&rb_id) {
                errors.push(format!(
                    "World {} lists ResourceBlock {} which does not exist",
                    world_id.0, rb_id.0
                ));
            }
        }
    }

    for (plan_id, plan) in &model.plans {
        if !model.worlds.contains_key(&plan.world_id) {
            errors.push(format!(
                "Plan {} references World {} which does not exist",
                plan_id.0, plan.world_id.0
            ));
        }
        for &wb_id in &plan.root_blocks {
            if !model.work_blocks.contains_key(&wb_id) {
                errors.push(format!(
                    "Plan {} root_block WorkBlock {} does not exist",
                    plan_id.0, wb_id.0
                ));
            }
        }
        for (&wb_id, &var_id) in &plan.selected_variants {
            if !model.work_blocks.contains_key(&wb_id) {
                errors.push(format!(
                    "Plan {} selected_variants key WorkBlock {} does not exist",
                    plan_id.0, wb_id.0
                ));
            }
            match model.variants.get(&var_id) {
                None => errors.push(format!(
                    "Plan {} selected Variant {} does not exist",
                    plan_id.0, var_id.0
                )),
                Some(v) if v.parent != wb_id => errors.push(format!(
                    "Plan {} selects Variant {} for WorkBlock {} but Variant's parent is {}",
                    plan_id.0, var_id.0, wb_id.0, v.parent.0
                )),
                Some(_) => {}
            }
        }
        for alloc in &plan.allocations {
            if !model.resource_blocks.contains_key(&alloc.resource_id) {
                errors.push(format!(
                    "Plan {} allocation ResourceBlock {} does not exist",
                    plan_id.0, alloc.resource_id.0
                ));
            }
            if !model.work_blocks.contains_key(&alloc.work_block_id) {
                errors.push(format!(
                    "Plan {} allocation WorkBlock {} does not exist",
                    plan_id.0, alloc.work_block_id.0
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidParameterName(errors.join("; ")))
    }
}

fn parse_resource_type(s: &str) -> Result<ResourceType> {
    match s {
        "Person" => Ok(ResourceType::Person),
        "Team" => Ok(ResourceType::Team),
        "Equipment" => Ok(ResourceType::Equipment),
        "Budget" => Ok(ResourceType::Budget),
        other => Err(rusqlite::Error::InvalidParameterName(format!(
            "Unknown resource_type: {other}"
        ))),
    }
}

fn parse_dependency_type(s: &str) -> Result<DependencyType> {
    match s {
        "FinishToStart" => Ok(DependencyType::FinishToStart),
        "StartToStart" => Ok(DependencyType::StartToStart),
        "FinishToFinish" => Ok(DependencyType::FinishToFinish),
        "StartToFinish" => Ok(DependencyType::StartToFinish),
        other => Err(rusqlite::Error::InvalidParameterName(format!(
            "Unknown dependency_type: {other}"
        ))),
    }
}

fn resource_type_str(rt: ResourceType) -> &'static str {
    match rt {
        ResourceType::Person => "Person",
        ResourceType::Team => "Team",
        ResourceType::Equipment => "Equipment",
        ResourceType::Budget => "Budget",
    }
}

fn dependency_type_str(dt: DependencyType) -> &'static str {
    match dt {
        DependencyType::FinishToStart => "FinishToStart",
        DependencyType::StartToStart => "StartToStart",
        DependencyType::FinishToFinish => "FinishToFinish",
        DependencyType::StartToFinish => "StartToFinish",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AvailabilitySegment, DependencyType, Estimate, Plan, ResourceAllocation, ResourceType,
        Variant, VariantId, WorkBlockId, WorldId,
    };
    use rusqlite::Connection;

    fn open_in_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    fn est(ml: f32, opt: f32, pes: f32, conf: f32) -> Estimate {
        Estimate {
            most_likely: ml,
            optimistic: opt,
            pessimistic: pes,
            confidence: conf,
        }
    }

    #[test]
    fn empty_model_round_trip() {
        let conn = open_in_memory();
        let m = Model::default();
        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();
        assert_eq!(m, loaded);
    }

    #[test]
    fn sparse_model_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();
        m.create_milestone("kickoff", 0.0);
        m.create_milestone("launch", 120.0);
        m.create_work_block("prep", est(5.0, 3.0, 10.0, 0.8));

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        assert_eq!(m.milestones, loaded.milestones);
        assert_eq!(m.work_blocks, loaded.work_blocks);
        assert!(loaded.worlds.is_empty());
        assert!(loaded.plans.is_empty());
        assert!(loaded.variants.is_empty());
        assert!(loaded.dependencies.is_empty());
        assert!(loaded.resource_blocks.is_empty());
    }

    #[test]
    fn full_model_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();

        let world_id = m.create_world("baseline");

        // Resources created in ascending ID order so ORDER BY id on reload preserves world.resource_ids.
        let rb1 = m.create_resource_block("Alice", ResourceType::Person);
        m.resource_blocks
            .get_mut(&rb1)
            .unwrap()
            .availability
            .segments
            .push(AvailabilitySegment {
                start: 0.0,
                end: 100.0,
                factor: 1.0,
            });
        m.resource_blocks
            .get_mut(&rb1)
            .unwrap()
            .availability
            .segments
            .push(AvailabilitySegment {
                start: 100.0,
                end: 200.0,
                factor: 0.5,
            });
        let rb2 = m.create_resource_block("Team Alpha", ResourceType::Team);
        m.worlds.get_mut(&world_id).unwrap().resource_ids.push(rb1);
        m.worlds.get_mut(&world_id).unwrap().resource_ids.push(rb2);

        let wb_a = m.create_work_block("Design", est(3.0, 1.0, 7.0, 0.75));
        let wb_b = m.create_work_block("Implement", est(10.0, 5.0, 20.0, 0.5));
        let wb_child = m.create_work_block("Sub-task", est(4.0, 2.0, 8.0, 0.75));

        let v1 = m.create_variant("fast", wb_b);
        let v2 = m.create_variant("thorough", wb_b);
        m.work_blocks.get_mut(&wb_b).unwrap().variants.push(v1);
        m.work_blocks.get_mut(&wb_b).unwrap().variants.push(v2);
        m.variants.get_mut(&v1).unwrap().children.push(wb_child);

        let dep_id = m.create_dependency(wb_a, wb_b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep_id).unwrap().lag = 1.5;

        m.create_milestone("launch", 90.0);

        let plan_id = m.create_plan("alpha", world_id);
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(wb_a);
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(wb_b);
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .selected_variants
            .insert(wb_b, v1);
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .allocations
            .push(ResourceAllocation {
                resource_id: rb1,
                work_block_id: wb_a,
                allocation_factor: 1.0,
            });
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .allocations
            .push(ResourceAllocation {
                resource_id: rb2,
                work_block_id: wb_b,
                allocation_factor: 0.5,
            });

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        assert_eq!(m.work_blocks, loaded.work_blocks);
        assert_eq!(m.variants, loaded.variants);
        assert_eq!(m.dependencies, loaded.dependencies);
        assert_eq!(m.milestones, loaded.milestones);
        assert_eq!(m.worlds, loaded.worlds);
        assert_eq!(m.resource_blocks, loaded.resource_blocks);

        // Allocations have no sort_order column, so compare as sorted sets.
        assert_eq!(m.plans.len(), loaded.plans.len());
        for (pid, orig) in &m.plans {
            let got = loaded.plans.get(pid).expect("plan missing after load");
            assert_eq!(orig.name, got.name);
            assert_eq!(orig.world_id, got.world_id);
            assert_eq!(orig.root_blocks, got.root_blocks);
            assert_eq!(orig.selected_variants, got.selected_variants);
            let mut a = orig.allocations.clone();
            let mut b = got.allocations.clone();
            a.sort_by_key(|x| (x.resource_id.0, x.work_block_id.0));
            b.sort_by_key(|x| (x.resource_id.0, x.work_block_id.0));
            assert_eq!(a, b, "plan allocations mismatch");
        }
    }

    #[test]
    fn validate_accepts_valid_model() {
        let mut m = Model::default();
        let w = m.create_world("w");
        let rb = m.create_resource_block("Alice", ResourceType::Person);
        m.worlds.get_mut(&w).unwrap().resource_ids.push(rb);
        let wb_a = m.create_work_block("a", est(1.0, 0.5, 2.0, 1.0));
        let wb_b = m.create_work_block("b", est(1.0, 0.5, 2.0, 1.0));
        let v = m.create_variant("v", wb_a);
        m.work_blocks.get_mut(&wb_a).unwrap().variants.push(v);
        let _dep = m.create_dependency(wb_a, wb_b, DependencyType::FinishToStart);
        let plan_id = m.create_plan("p", w);
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(wb_a);
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .selected_variants
            .insert(wb_a, v);
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .allocations
            .push(ResourceAllocation {
                resource_id: rb,
                work_block_id: wb_a,
                allocation_factor: 1.0,
            });
        assert!(validate_model(&m).is_ok());
    }

    #[test]
    fn validate_catches_orphan_variant_parent() {
        let mut m = Model::default();
        let wb_id = m.create_work_block("wb", est(1.0, 0.5, 2.0, 1.0));
        let v_id = m.create_variant("v", wb_id);
        m.work_blocks.get_mut(&wb_id).unwrap().variants.push(v_id);
        m.variants.insert(
            VariantId(999),
            Variant {
                id: VariantId(999),
                name: "orphan".into(),
                parent: WorkBlockId(888),
                children: vec![],
            },
        );
        let err = validate_model(&m).unwrap_err().to_string();
        assert!(
            err.contains("888"),
            "expected missing parent ID 888 in: {err}"
        );
    }

    #[test]
    fn validate_catches_plan_bad_world() {
        let mut m = Model::default();
        m.create_world("real");
        m.plans.insert(
            crate::model::PlanId(99),
            Plan {
                id: crate::model::PlanId(99),
                name: "bad".into(),
                world_id: WorldId(888),
                root_blocks: vec![],
                selected_variants: Default::default(),
                allocations: vec![],
            },
        );
        let err = validate_model(&m).unwrap_err().to_string();
        assert!(
            err.contains("888"),
            "expected missing world ID 888 in: {err}"
        );
    }

    #[test]
    fn validate_catches_mismatched_variant_selection() {
        let mut m = Model::default();
        let world_id = m.create_world("w");
        let wb_a = m.create_work_block("a", est(1.0, 0.5, 2.0, 1.0));
        let wb_b = m.create_work_block("b", est(1.0, 0.5, 2.0, 1.0));
        let v = m.create_variant("v", wb_a);
        m.work_blocks.get_mut(&wb_a).unwrap().variants.push(v);
        let plan_id = m.create_plan("p", world_id);
        // Select variant v (parent = wb_a) for wb_b — wrong parent
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .selected_variants
            .insert(wb_b, v);
        let err = validate_model(&m).unwrap_err().to_string();
        assert!(
            err.contains("parent"),
            "expected parent mismatch message in: {err}"
        );
    }
}

const CREATE_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS worlds (
    id   INTEGER PRIMARY KEY,
    name TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS resource_blocks (
    id            INTEGER PRIMARY KEY,
    world_id      INTEGER NOT NULL REFERENCES worlds(id),
    name          TEXT    NOT NULL,
    resource_type TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS availability_segments (
    id                INTEGER PRIMARY KEY,
    resource_block_id INTEGER NOT NULL REFERENCES resource_blocks(id),
    start_day         REAL    NOT NULL,
    end_day           REAL    NOT NULL,
    factor            REAL    NOT NULL,
    sort_order        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS work_blocks (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT NOT NULL,
    estimate_most_likely REAL NOT NULL,
    estimate_optimistic  REAL NOT NULL,
    estimate_pessimistic REAL NOT NULL,
    estimate_confidence  REAL NOT NULL,
    start_day            REAL NOT NULL DEFAULT 0,
    duration_days        REAL NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS variants (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT    NOT NULL,
    parent_work_block_id INTEGER NOT NULL REFERENCES work_blocks(id),
    sort_order           INTEGER NOT NULL,
    UNIQUE (parent_work_block_id, sort_order)
);

CREATE TABLE IF NOT EXISTS variant_children (
    variant_id           INTEGER NOT NULL REFERENCES variants(id),
    child_work_block_id  INTEGER NOT NULL REFERENCES work_blocks(id),
    sort_order           INTEGER NOT NULL,
    PRIMARY KEY (variant_id, sort_order)
);

CREATE TABLE IF NOT EXISTS dependencies (
    id              INTEGER PRIMARY KEY,
    predecessor_id  INTEGER NOT NULL REFERENCES work_blocks(id),
    successor_id    INTEGER NOT NULL REFERENCES work_blocks(id),
    dependency_type TEXT    NOT NULL,
    lag_days        REAL    NOT NULL DEFAULT 0.0
);

CREATE TABLE IF NOT EXISTS milestones (
    id       INTEGER PRIMARY KEY,
    name     TEXT NOT NULL,
    date_day REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS plans (
    id       INTEGER PRIMARY KEY,
    name     TEXT    NOT NULL,
    world_id INTEGER NOT NULL REFERENCES worlds(id)
);

CREATE TABLE IF NOT EXISTS plan_root_blocks (
    plan_id       INTEGER NOT NULL REFERENCES plans(id),
    work_block_id INTEGER NOT NULL REFERENCES work_blocks(id),
    sort_order    INTEGER NOT NULL,
    PRIMARY KEY (plan_id, sort_order)
);

CREATE TABLE IF NOT EXISTS plan_variant_selections (
    plan_id       INTEGER NOT NULL REFERENCES plans(id),
    work_block_id INTEGER NOT NULL REFERENCES work_blocks(id),
    variant_id    INTEGER NOT NULL REFERENCES variants(id),
    PRIMARY KEY (plan_id, work_block_id)
);

CREATE TABLE IF NOT EXISTS resource_allocations (
    id                INTEGER PRIMARY KEY,
    plan_id           INTEGER NOT NULL REFERENCES plans(id),
    resource_block_id INTEGER NOT NULL REFERENCES resource_blocks(id),
    work_block_id     INTEGER NOT NULL REFERENCES work_blocks(id),
    allocation_factor REAL    NOT NULL
);

CREATE TABLE IF NOT EXISTS plan_milestone_targets (
    plan_id      INTEGER NOT NULL REFERENCES plans(id),
    milestone_id INTEGER NOT NULL REFERENCES milestones(id),
    target_day   REAL    NOT NULL,
    PRIMARY KEY (plan_id, milestone_id)
);
";
