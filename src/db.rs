use std::collections::HashMap;

use rusqlite::{Connection, Result};

use crate::model::{
    AvailabilitySegment, AvailabilityTimeline, Dependency, DependencyId, DependencyType,
    Estimate, Milestone, MilestoneId, Model, Plan, PlanId, ResourceAllocation, ResourceBlock,
    ResourceBlockId, ResourceType, Variant, VariantId, WorkBlock, WorkBlockId, World, WorldId,
};

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(CREATE_TABLES_SQL)?;
    Ok(())
}

/// Persists the complete Model to SQLite in a single transaction.
///
/// All existing rows are deleted and reinserted, so the DB reflects
/// the model exactly after this call (including deletions).
pub fn save_model(conn: &Connection, model: &Model) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    // Clear in reverse FK order so no constraint is violated.
    tx.execute_batch("
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
    ")?;

    // worlds
    for world in model.worlds.values() {
        tx.execute(
            "INSERT INTO worlds (id, name) VALUES (?1, ?2)",
            (world.id.0, &world.name),
        )?;
    }

    // Build resource_block_id → world_id from World.resource_ids.
    let mut rb_to_world: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
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
            (rb.id.0, world_id, &rb.name, resource_type_str(rb.resource_type)),
        )?;
        for (order, seg) in rb.availability.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO availability_segments
                     (resource_block_id, start_day, end_day, factor, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (rb.id.0, seg.start as f64, seg.end as f64, seg.factor as f64, order as i64),
            )?;
        }
    }

    // work_blocks
    for wb in model.work_blocks.values() {
        tx.execute(
            "INSERT INTO work_blocks
                 (id, name, estimate_most_likely, estimate_optimistic,
                  estimate_pessimistic, estimate_confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                wb.id.0,
                &wb.name,
                wb.estimate.most_likely as f64,
                wb.estimate.optimistic as f64,
                wb.estimate.pessimistic as f64,
                wb.estimate.confidence as f64,
            ),
        )?;
    }

    // variants + variant_children
    for v in model.variants.values() {
        tx.execute(
            "INSERT INTO variants (id, name, parent_work_block_id) VALUES (?1, ?2, ?3)",
            (v.id.0, &v.name, v.parent.0),
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
                World { id: WorldId(id as u64), name, resource_ids: vec![] },
            );
        }
    }

    // resource_blocks  (also populate world.resource_ids)
    {
        let mut stmt = conn.prepare(
            "SELECT id, world_id, name, resource_type FROM resource_blocks",
        )?;
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
            if let Some(rb) = model.resource_blocks.get_mut(&ResourceBlockId(rb_id as u64)) {
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
                    estimate_pessimistic, estimate_confidence
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
            ))
        })?;
        for row in rows {
            let (id, name, ml, opt, pes, conf) = row?;
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
                },
            );
        }
    }

    // variants  (children populated below)
    {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_work_block_id FROM variants",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })?;
        for row in rows {
            let (id, name, parent_id) = row?;
            bump!(id);
            model.variants.insert(
                VariantId(id as u64),
                Variant {
                    id: VariantId(id as u64),
                    name,
                    parent: WorkBlockId(parent_id as u64),
                    children: vec![],
                },
            );
        }
    }

    // variant_children → populate variant.children (order preserved)
    {
        let mut stmt = conn.prepare(
            "SELECT variant_id, child_work_block_id
             FROM variant_children
             ORDER BY variant_id, sort_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (var_id, child_id) = row?;
            if let Some(v) = model.variants.get_mut(&VariantId(var_id as u64)) {
                v.children.push(WorkBlockId(child_id as u64));
            }
        }
    }

    // Rebuild work_block.variants from each variant's parent field.
    let parent_links: Vec<(WorkBlockId, VariantId)> = model
        .variants
        .values()
        .map(|v| (v.parent, v.id))
        .collect();
    for (wb_id, var_id) in parent_links {
        if let Some(wb) = model.work_blocks.get_mut(&wb_id) {
            wb.variants.push(var_id);
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
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, f64>(2)?))
        })?;
        for row in rows {
            let (id, name, date) = row?;
            bump!(id);
            model.milestones.insert(
                MilestoneId(id as u64),
                Milestone { id: MilestoneId(id as u64), name, date: date as f32 },
            );
        }
    }

    // plans
    {
        let mut stmt = conn.prepare("SELECT id, name, world_id FROM plans")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
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
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (plan_id, wb_id) = row?;
            if let Some(plan) = model.plans.get_mut(&PlanId(plan_id as u64)) {
                plan.root_blocks.push(WorkBlockId(wb_id as u64));
            }
        }
    }

    // plan_variant_selections
    {
        let mut stmt = conn.prepare(
            "SELECT plan_id, work_block_id, variant_id FROM plan_variant_selections",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
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
    Ok(model)
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
    estimate_confidence  REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS variants (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT    NOT NULL,
    parent_work_block_id INTEGER NOT NULL REFERENCES work_blocks(id)
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
