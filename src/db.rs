use std::collections::HashMap;

use chrono::NaiveDate;
use rusqlite::{Connection, Result};

use crate::model::{
    AvailabilitySegment, AvailabilityTimeline, ConfidenceFactors, Dependency, DependencyId,
    DependencyType, Estimate, Milestone, MilestoneId, Model, Plan, PlanId, ResourceAllocation,
    ResourceBlock, ResourceBlockId, ResourceType, TShirtSize, Variant, VariantId, WorkBlock,
    WorkBlockId, World, WorldId,
};

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(CREATE_TABLES_SQL)?;
    // SQLite has no ADD COLUMN IF NOT EXISTS. Run each migration and ignore
    // the "duplicate column name" error that fires when it already exists.
    for sql in [
        "ALTER TABLE work_blocks ADD COLUMN start_day REAL NOT NULL DEFAULT 0",
        "ALTER TABLE work_blocks ADD COLUMN duration_days REAL NOT NULL DEFAULT 0",
        "ALTER TABLE work_blocks ADD COLUMN color_r REAL",
        "ALTER TABLE work_blocks ADD COLUMN color_g REAL",
        "ALTER TABLE work_blocks ADD COLUMN color_b REAL",
        "ALTER TABLE work_blocks ADD COLUMN description TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE work_blocks ADD COLUMN priority INTEGER NOT NULL DEFAULT 1",
        "ALTER TABLE work_blocks ADD COLUMN t_shirt_size TEXT",
    ] {
        match conn.execute_batch(sql) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(e),
        }
    }
    // Seed default t-shirt sizes on first use (table created above).
    let count: i64 =
        conn.query_row("SELECT COUNT(*) FROM t_shirt_sizes", [], |r| r.get(0))?;
    if count == 0 {
        conn.execute_batch(
            "INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('XS',  1.0, 0);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('S',   3.0, 1);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('M',   5.0, 2);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('L',  10.0, 3);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('XL', 20.0, 4);",
        )?;
    }
    Ok(())
}

/// Appends one row to `estimate_snapshots` recording the user's current
/// duration and confidence for a block. Called every time either value
/// changes in the inspector so the history accumulates over time.
pub fn record_estimate_snapshot(
    conn: &Connection,
    work_block_id: u64,
    duration_days: f32,
    confidence: f32,
) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    conn.execute(
        "INSERT INTO estimate_snapshots
             (work_block_id, duration_days, confidence, recorded_at)
         VALUES (?1, ?2, ?3, ?4)",
        (work_block_id as i64, duration_days as f64, confidence as f64, ts),
    )?;
    Ok(())
}

/// Persists the complete Model to SQLite in a single transaction.
///
/// Uses a three-phase approach to minimise WAL traffic:
///
/// 1. **Clear join tables** (fully owned rows with no FK children — safe to truncate).
/// 2. **Upsert entity rows** via `INSERT … ON CONFLICT(id) DO UPDATE SET`.
///    This is a genuine in-place update, not a delete+insert, so the WAL
///    records only changed columns for existing rows instead of re-writing
///    every row on every save.
/// 3. **Delete stale entity rows** whose IDs are no longer in the model
///    (`DELETE … WHERE id NOT IN (…)`), in reverse FK order.
/// 4. **Reinsert join-table rows** for all current entities.
///
/// Maintains FK correctness throughout: join tables are cleared before entity
/// rows are touched, and stale entities are deleted after their referents are
/// gone. The DB reflects the model exactly when the transaction commits.
pub fn save_model(conn: &Connection, model: &Model) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    // ── Phase 1: clear join tables ────────────────────────────────────────────
    // These tables are fully owned by their parent entity and have no FK
    // children of their own, so truncating them is always safe.
    tx.execute_batch(
        "DELETE FROM plan_milestone_targets;
         DELETE FROM resource_allocations;
         DELETE FROM plan_variant_selections;
         DELETE FROM plan_root_blocks;
         DELETE FROM variant_children;
         DELETE FROM variant_block_positions;
         DELETE FROM availability_segments;
         DELETE FROM calendar_non_working_dates;
         DELETE FROM t_shirt_sizes;
         DELETE FROM quarter_colors;",
    )?;

    // ── Phase 2: upsert all current entity rows ───────────────────────────────
    // INSERT … ON CONFLICT(id) DO UPDATE SET performs a genuine in-place
    // update on existing rows — no delete+insert — so WAL traffic is
    // proportional to changed rows rather than total row count.

    for world in model.worlds.values() {
        tx.execute(
            "INSERT INTO worlds (id, name) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            (world.id.0 as i64, &world.name),
        )?;
    }

    let mut rb_to_world: HashMap<u64, u64> = HashMap::new();
    for world in model.worlds.values() {
        for &rb_id in &world.resource_ids {
            rb_to_world.insert(rb_id.0, world.id.0);
        }
    }
    for rb in model.resource_blocks.values() {
        let world_id = rb_to_world.get(&rb.id.0).copied().ok_or_else(|| {
            rusqlite::Error::InvalidParameterName(format!(
                "ResourceBlock {} is not in any World.resource_ids",
                rb.id.0
            ))
        })?;
        tx.execute(
            "INSERT INTO resource_blocks (id, world_id, name, resource_type)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 world_id = excluded.world_id,
                 name = excluded.name,
                 resource_type = excluded.resource_type",
            (rb.id.0 as i64, world_id as i64, &rb.name, resource_type_str(rb.resource_type)),
        )?;
    }

    for wb in model.work_blocks.values() {
        tx.execute(
            "INSERT INTO work_blocks
                 (id, name, estimate_most_likely, estimate_optimistic,
                  estimate_pessimistic, estimate_confidence,
                  start_day, duration_days, color_r, color_g, color_b, description, priority,
                  t_shirt_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 estimate_most_likely = excluded.estimate_most_likely,
                 estimate_optimistic = excluded.estimate_optimistic,
                 estimate_pessimistic = excluded.estimate_pessimistic,
                 estimate_confidence = excluded.estimate_confidence,
                 start_day = excluded.start_day,
                 duration_days = excluded.duration_days,
                 color_r = excluded.color_r,
                 color_g = excluded.color_g,
                 color_b = excluded.color_b,
                 description = excluded.description,
                 priority = excluded.priority,
                 t_shirt_size = excluded.t_shirt_size",
            (
                wb.id.0 as i64,
                &wb.name,
                wb.estimate.most_likely as f64,
                wb.estimate.optimistic as f64,
                wb.estimate.pessimistic as f64,
                wb.estimate.confidence as f64,
                wb.start_day as f64,
                wb.duration_days as f64,
                wb.color.map(|c| c[0] as f64),
                wb.color.map(|c| c[1] as f64),
                wb.color.map(|c| c[2] as f64),
                &wb.description,
                wb.priority as i64,
                &wb.t_shirt_size,
            ),
        )?;
    }

    // variants: use INSERT OR REPLACE rather than ON CONFLICT DO UPDATE because
    // the UNIQUE(parent_work_block_id, sort_order) constraint would fire if two
    // variants swap positions during an in-place UPDATE. INSERT OR REPLACE
    // deletes the conflicting row first, then inserts, avoiding the collision.
    // variant_children was cleared in phase 1, so any cascade-delete is safe.
    let mut variant_sort_order: HashMap<u64, i64> = HashMap::new();
    for wb in model.work_blocks.values() {
        for (order, &var_id) in wb.variants.iter().enumerate() {
            variant_sort_order.insert(var_id.0, order as i64);
        }
    }
    for v in model.variants.values() {
        let order = variant_sort_order.get(&v.id.0).copied().unwrap_or(0);
        tx.execute(
            "INSERT OR REPLACE INTO variants (id, name, parent_work_block_id, sort_order)
             VALUES (?1, ?2, ?3, ?4)",
            (v.id.0 as i64, &v.name, v.parent.0 as i64, order),
        )?;
    }

    for dep in model.dependencies.values() {
        tx.execute(
            "INSERT INTO dependencies
                 (id, predecessor_id, successor_id, dependency_type, lag_days)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 predecessor_id = excluded.predecessor_id,
                 successor_id = excluded.successor_id,
                 dependency_type = excluded.dependency_type,
                 lag_days = excluded.lag_days",
            (
                dep.id.0 as i64,
                dep.predecessor.0 as i64,
                dep.successor.0 as i64,
                dependency_type_str(dep.dependency_type),
                dep.lag as f64,
            ),
        )?;
    }

    for ms in model.milestones.values() {
        tx.execute(
            "INSERT INTO milestones (id, name, date_day) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, date_day = excluded.date_day",
            (ms.id.0 as i64, &ms.name, ms.date as f64),
        )?;
    }

    for plan in model.plans.values() {
        tx.execute(
            "INSERT INTO plans (id, name, world_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, world_id = excluded.world_id",
            (plan.id.0 as i64, &plan.name, plan.world_id.0 as i64),
        )?;
    }

    tx.execute(
        "INSERT INTO calendar_config (id, start_date, working_days_per_week) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET
             start_date = excluded.start_date,
             working_days_per_week = excluded.working_days_per_week",
        (
            model.calendar.start_date.format("%Y-%m-%d").to_string(),
            model.calendar.working_days_per_week as i64,
        ),
    )?;

    for (q, color) in model.calendar.quarter_colors.iter().enumerate() {
        tx.execute(
            "INSERT OR REPLACE INTO quarter_colors (quarter, color_r, color_g, color_b, color_a)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (q as i64, color[0] as f64, color[1] as f64, color[2] as f64, color[3] as f64),
        )?;
    }

    tx.execute(
        "INSERT INTO confidence_factors (id, opt_50, pes_50, opt_75, pes_75)
             VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             opt_50 = excluded.opt_50,
             pes_50 = excluded.pes_50,
             opt_75 = excluded.opt_75,
             pes_75 = excluded.pes_75",
        (
            model.confidence_factors.opt_50 as f64,
            model.confidence_factors.pes_50 as f64,
            model.confidence_factors.opt_75 as f64,
            model.confidence_factors.pes_75 as f64,
        ),
    )?;

    // ── Phase 3: delete stale entity rows ─────────────────────────────────────
    // Processed in reverse FK order: child-referencing tables are deleted
    // before the tables they reference, so no FK constraint fires.
    // Join tables are already empty (cleared in phase 1), so entity rows
    // have no remaining FK children at this point.
    delete_stale(&tx, "plans",           &model.plans.keys().map(|k| k.0).collect::<Vec<_>>())?;
    delete_stale(&tx, "dependencies",    &model.dependencies.keys().map(|k| k.0).collect::<Vec<_>>())?;
    delete_stale(&tx, "variants",        &model.variants.keys().map(|k| k.0).collect::<Vec<_>>())?;
    delete_stale(&tx, "milestones",      &model.milestones.keys().map(|k| k.0).collect::<Vec<_>>())?;
    delete_stale(&tx, "resource_blocks", &model.resource_blocks.keys().map(|k| k.0).collect::<Vec<_>>())?;
    delete_stale(&tx, "work_blocks",     &model.work_blocks.keys().map(|k| k.0).collect::<Vec<_>>())?;
    delete_stale(&tx, "worlds",          &model.worlds.keys().map(|k| k.0).collect::<Vec<_>>())?;

    // ── Phase 4: reinsert join table rows for current entities ────────────────

    for rb in model.resource_blocks.values() {
        for (order, seg) in rb.availability.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO availability_segments
                     (resource_block_id, start_day, end_day, factor, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (rb.id.0 as i64, seg.start as f64, seg.end as f64, seg.factor as f64, order as i64),
            )?;
        }
    }

    for v in model.variants.values() {
        for (order, &child_id) in v.children.iter().enumerate() {
            tx.execute(
                "INSERT INTO variant_children (variant_id, child_work_block_id, sort_order)
                 VALUES (?1, ?2, ?3)",
                (v.id.0 as i64, child_id.0 as i64, order as i64),
            )?;
        }
        for (&wb_id, &(sd, dd)) in &v.block_positions {
            tx.execute(
                "INSERT INTO variant_block_positions
                     (variant_id, work_block_id, start_day, duration_days)
                 VALUES (?1, ?2, ?3, ?4)",
                (v.id.0 as i64, wb_id.0 as i64, sd as f64, dd as f64),
            )?;
        }
    }

    for plan in model.plans.values() {
        for (order, &wb_id) in plan.root_blocks.iter().enumerate() {
            tx.execute(
                "INSERT INTO plan_root_blocks (plan_id, work_block_id, sort_order)
                 VALUES (?1, ?2, ?3)",
                (plan.id.0 as i64, wb_id.0 as i64, order as i64),
            )?;
        }
        for (&wb_id, &var_id) in &plan.selected_variants {
            tx.execute(
                "INSERT INTO plan_variant_selections (plan_id, work_block_id, variant_id)
                 VALUES (?1, ?2, ?3)",
                (plan.id.0 as i64, wb_id.0 as i64, var_id.0 as i64),
            )?;
        }
        for alloc in &plan.allocations {
            tx.execute(
                "INSERT INTO resource_allocations
                     (plan_id, resource_block_id, work_block_id, allocation_factor)
                 VALUES (?1, ?2, ?3, ?4)",
                (
                    plan.id.0 as i64,
                    alloc.resource_id.0 as i64,
                    alloc.work_block_id.0 as i64,
                    alloc.allocation_factor as f64,
                ),
            )?;
        }
    }

    for date in &model.calendar.non_working_dates {
        tx.execute(
            "INSERT INTO calendar_non_working_dates (date) VALUES (?1)",
            (&date.format("%Y-%m-%d").to_string(),),
        )?;
    }

    // t_shirt_sizes
    for (order, size) in model.t_shirt_sizes.iter().enumerate() {
        tx.execute(
            "INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES (?1, ?2, ?3)",
            (&size.label, size.days as f64, order as i64),
        )?;
    }

    tx.commit()
}

/// Deletes rows from `table` whose `id` column is not in `current_ids`.
/// If `current_ids` is empty the entire table is cleared (every row is stale).
/// Table names come from hardcoded call-sites so there is no injection risk.
fn delete_stale(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    current_ids: &[u64],
) -> Result<()> {
    if current_ids.is_empty() {
        tx.execute_batch(&format!("DELETE FROM {table}"))?;
        return Ok(());
    }
    let placeholders = std::iter::repeat("?")
        .take(current_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("DELETE FROM {table} WHERE id NOT IN ({placeholders})");
    tx.execute(
        &sql,
        rusqlite::params_from_iter(current_ids.iter().map(|&id| id as i64)),
    )?;
    Ok(())
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
                    start_day, duration_days, color_r, color_g, color_b, description, priority,
                    t_shirt_size
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
                row.get::<_, Option<f64>>(8)?,
                row.get::<_, Option<f64>>(9)?,
                row.get::<_, Option<f64>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, Option<String>>(13)?,
            ))
        })?;
        for row in rows {
            let (id, name, ml, opt, pes, conf, start_day, duration_days, cr, cg, cb, description, priority, t_shirt_size) = row?;
            let color = match (cr, cg, cb) {
                (Some(r), Some(g), Some(b)) => Some([r as f32, g as f32, b as f32]),
                _ => None,
            };
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
                    color,
                    description,
                    priority: priority.clamp(0, 3) as u8,
                    t_shirt_size,
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
                    block_positions: std::collections::HashMap::new(),
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

    // variant_block_positions → restore snapshots into variant.block_positions
    {
        let mut stmt = conn.prepare(
            "SELECT variant_id, work_block_id, start_day, duration_days
             FROM variant_block_positions",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        for row in rows {
            let (var_id, wb_id, sd, dd) = row?;
            if let Some(v) = model.variants.get_mut(&VariantId(var_id as u64)) {
                v.block_positions
                    .insert(WorkBlockId(wb_id as u64), (sd as f32, dd as f32));
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

    // calendar_config
    {
        let mut stmt = conn.prepare(
            "SELECT start_date, working_days_per_week FROM calendar_config WHERE id = 1",
        )?;
        match stmt.query_row([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) {
            Ok((date_str, wdpw)) => {
                if let Ok(date) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
                    model.calendar.start_date = date;
                }
                model.calendar.working_days_per_week = wdpw.clamp(1, 7) as u8;
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Err(e),
        }
    }

    // confidence_factors
    {
        let mut stmt = conn.prepare(
            "SELECT opt_50, pes_50, opt_75, pes_75 FROM confidence_factors WHERE id = 1",
        )?;
        match stmt.query_row([], |row| {
            Ok((
                row.get::<_, f64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        }) {
            Ok((opt_50, pes_50, opt_75, pes_75)) => {
                model.confidence_factors = ConfidenceFactors {
                    opt_50: opt_50 as f32,
                    pes_50: pes_50 as f32,
                    opt_75: opt_75 as f32,
                    pes_75: pes_75 as f32,
                };
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Err(e),
        }
    }

    // quarter_colors
    {
        let mut stmt = conn.prepare(
            "SELECT quarter, color_r, color_g, color_b, color_a FROM quarter_colors ORDER BY quarter",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?;
        for row in rows {
            let (q, r, g, b, a) = row?;
            if q >= 0 && q < 4 {
                model.calendar.quarter_colors[q as usize] =
                    [r as f32, g as f32, b as f32, a as f32];
            }
        }
    }

    // calendar_non_working_dates
    {
        let mut stmt =
            conn.prepare("SELECT date FROM calendar_non_working_dates ORDER BY date")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let date_str = row?;
            if let Ok(date) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
                model.calendar.non_working_dates.push(date);
            }
        }
    }

    // t_shirt_sizes
    {
        let mut stmt =
            conn.prepare("SELECT label, days FROM t_shirt_sizes ORDER BY sort_order")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        for row in rows {
            let (label, days) = row?;
            model.t_shirt_sizes.push(TShirtSize {
                label,
                days: days as f32,
            });
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
    fn work_block_placement_fields_round_trip() {
        // Verify that non-default start_day and duration_days survive a
        // save_model → load_model cycle.  The general round-trip tests use
        // create_work_block (which zeros both), so this explicitly covers the
        // persistence path for user-defined placement values.
        let conn = open_in_memory();
        let mut m = Model::default();
        let id = m.create_work_block("task", est(5.0, 3.0, 8.0, 0.9));
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = 7.5;
        wb.duration_days = 3.0;

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        let loaded_wb = loaded.work_blocks.get(&id).unwrap();
        assert_eq!(loaded_wb.start_day, 7.5);
        assert_eq!(loaded_wb.duration_days, 3.0);
        assert_eq!(m.work_blocks, loaded.work_blocks);
    }

    #[test]
    fn nonzero_start_day_and_duration_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();
        let id = m.create_work_block("placed task", est(4.0, 2.0, 8.0, 0.8));
        let wb = m.work_blocks.get_mut(&id).unwrap();
        wb.start_day = 3.5;
        wb.duration_days = 7.0;

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        let loaded_wb = loaded.work_blocks.get(&id).unwrap();
        assert_eq!(loaded_wb.start_day, 3.5);
        assert_eq!(loaded_wb.duration_days, 7.0);
    }

    #[test]
    fn migration_from_pre_br60_schema_adds_placement_columns() {
        // Simulate upgrading a DB created before br-60 added start_day /
        // duration_days.  We create the old work_blocks table (no placement
        // columns), insert a row, then call create_tables to run the ALTER
        // TABLE migrations, and verify load_model succeeds with defaults.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        // Old schema: work_blocks without placement columns.
        conn.execute_batch(
            "CREATE TABLE work_blocks (
                id                   INTEGER PRIMARY KEY,
                name                 TEXT NOT NULL,
                estimate_most_likely REAL NOT NULL,
                estimate_optimistic  REAL NOT NULL,
                estimate_pessimistic REAL NOT NULL,
                estimate_confidence  REAL NOT NULL
            );",
        )
        .unwrap();

        // Insert a pre-migration work block.
        conn.execute(
            "INSERT INTO work_blocks
                 (id, name, estimate_most_likely, estimate_optimistic,
                  estimate_pessimistic, estimate_confidence)
             VALUES (1, 'legacy task', 5.0, 3.5, 8.0, 0.8)",
            [],
        )
        .unwrap();

        // Run create_tables: all other tables are created fresh, and the two
        // ALTER TABLE statements add start_day / duration_days to work_blocks.
        create_tables(&conn).unwrap();

        let model = load_model(&conn).unwrap();
        let wb_id = WorkBlockId(1);
        let wb = model.work_blocks.get(&wb_id).expect("legacy block loaded");
        assert_eq!(wb.name, "legacy task");
        assert_eq!(wb.start_day, 0.0, "start_day should default to 0.0");
        assert_eq!(wb.duration_days, 0.0, "duration_days should default to 0.0");
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
                block_positions: std::collections::HashMap::new(),
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

    // ── Incremental / stale-deletion tests ───────────────────────────────────

    #[test]
    fn stale_work_block_deleted_on_second_save() {
        // Exercises the WHERE id NOT IN (…) path in delete_stale for work_blocks.
        // Save with block A, then remove it, save again, and verify it is gone.
        let conn = open_in_memory();
        let mut m = Model::default();
        let wb_id = m.create_work_block("to-be-removed", est(3.0, 2.0, 5.0, 0.9));

        save_model(&conn, &m).unwrap();
        // Confirm it is present after the first save.
        let after_first = load_model(&conn).unwrap();
        assert!(after_first.work_blocks.contains_key(&wb_id));

        // Remove the block and save again.
        m.work_blocks.remove(&wb_id);
        save_model(&conn, &m).unwrap();

        let after_second = load_model(&conn).unwrap();
        assert!(
            !after_second.work_blocks.contains_key(&wb_id),
            "stale work_block should have been deleted by Phase 3"
        );
    }

    #[test]
    fn incremental_save_updates_existing_entity() {
        // Verifies that an in-place upsert (ON CONFLICT DO UPDATE) actually
        // reflects the new field values on the second load.
        let conn = open_in_memory();
        let mut m = Model::default();
        let wb_id = m.create_work_block("original", est(5.0, 3.0, 8.0, 0.8));

        save_model(&conn, &m).unwrap();

        // Mutate the block in-place.
        m.work_blocks.get_mut(&wb_id).unwrap().name = "renamed".to_string();
        save_model(&conn, &m).unwrap();

        let loaded = load_model(&conn).unwrap();
        let wb = loaded.work_blocks.get(&wb_id).expect("block must still exist");
        assert_eq!(wb.name, "renamed", "upsert must have written the new name");
    }

    #[test]
    fn stale_deletion_with_empty_current_set() {
        // Exercises the `current_ids.is_empty()` branch of delete_stale, which
        // clears the table entirely with `DELETE FROM <table>` rather than
        // `DELETE FROM <table> WHERE id NOT IN (…)`.
        let conn = open_in_memory();
        let mut m = Model::default();
        m.create_work_block("block-a", est(2.0, 1.0, 4.0, 1.0));
        m.create_work_block("block-b", est(3.0, 2.0, 5.0, 0.9));

        save_model(&conn, &m).unwrap();
        assert_eq!(load_model(&conn).unwrap().work_blocks.len(), 2);

        // Remove all blocks — save_model will call delete_stale with empty ids.
        m.work_blocks.clear();
        save_model(&conn, &m).unwrap();

        let loaded = load_model(&conn).unwrap();
        assert!(
            loaded.work_blocks.is_empty(),
            "delete_stale empty-set path must clear the table"
        );
    }

    #[test]
    fn stale_entity_deletion_across_multiple_types() {
        // One test exercising stale deletion for several entity types in one cycle.
        let conn = open_in_memory();
        let mut m = Model::default();
        let world_id = m.create_world("w");
        let rb_id = m.create_resource_block("Alice", ResourceType::Person);
        m.worlds.get_mut(&world_id).unwrap().resource_ids.push(rb_id);
        let wb_a = m.create_work_block("A", est(2.0, 1.0, 3.0, 1.0));
        let wb_b = m.create_work_block("B", est(3.0, 2.0, 5.0, 0.9));
        let dep = m.create_dependency(wb_a, wb_b, DependencyType::FinishToStart);
        let ms_id = m.create_milestone("launch", 10.0);

        save_model(&conn, &m).unwrap();

        // Remove wb_b, the dependency referencing it, the resource block, and the milestone.
        m.dependencies.remove(&dep);
        m.work_blocks.remove(&wb_b);
        // Remove rb from world.resource_ids before removing from resource_blocks,
        // since save_model validates ResourceBlocks are in a World.resource_ids list.
        m.worlds.get_mut(&world_id).unwrap().resource_ids.retain(|&id| id != rb_id);
        m.resource_blocks.remove(&rb_id);
        m.milestones.remove(&ms_id);
        save_model(&conn, &m).unwrap();

        let loaded = load_model(&conn).unwrap();
        assert!(!loaded.work_blocks.contains_key(&wb_b), "wb_b should be deleted");
        assert!(!loaded.dependencies.contains_key(&dep), "dependency should be deleted");
        assert!(!loaded.resource_blocks.contains_key(&rb_id), "resource_block should be deleted");
        assert!(!loaded.milestones.contains_key(&ms_id), "milestone should be deleted");
        // Surviving entity stays.
        assert!(loaded.work_blocks.contains_key(&wb_a), "wb_a must survive");
    }

    #[test]
    fn variant_block_positions_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();
        let wb_parent = m.create_work_block("parent", est(3.0, 2.0, 5.0, 0.8));
        let wb_child = m.create_work_block("child", est(2.0, 1.0, 4.0, 0.8));
        let vid = m.create_variant("fast", wb_parent);
        m.work_blocks.get_mut(&wb_parent).unwrap().variants.push(vid);
        m.variants.get_mut(&vid).unwrap().children.push(wb_child);
        // Store a position snapshot on the variant.
        m.variants.get_mut(&vid).unwrap().block_positions.insert(wb_child, (5.0, 3.0));

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        let loaded_v = loaded.variants.get(&vid).unwrap();
        assert_eq!(loaded_v.block_positions.get(&wb_child), Some(&(5.0, 3.0)));
    }

    #[test]
    fn confidence_factors_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();
        m.confidence_factors = ConfidenceFactors { opt_50: 0.4, pes_50: 3.0, opt_75: 0.6, pes_75: 1.8 };
        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();
        assert_eq!(loaded.confidence_factors, m.confidence_factors);
    }

    #[test]
    fn confidence_factors_default_when_row_absent() {
        let conn = open_in_memory();
        // load_model on a fresh DB with no saved confidence_factors row returns defaults.
        let m = load_model(&conn).unwrap();
        assert_eq!(m.confidence_factors, ConfidenceFactors::default());
    }

    #[test]
    fn quarter_colors_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();
        m.calendar.quarter_colors = [
            [0.10, 0.20, 0.30, 0.05],
            [0.40, 0.50, 0.60, 0.08],
            [0.70, 0.80, 0.90, 0.06],
            [0.11, 0.22, 0.33, 0.04],
        ];
        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();
        for q in 0..4 {
            for ch in 0..4 {
                assert!(
                    (loaded.calendar.quarter_colors[q][ch] - m.calendar.quarter_colors[q][ch]).abs() < 1e-5,
                    "Q{q} channel {ch} mismatch"
                );
            }
        }
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
    duration_days        REAL NOT NULL DEFAULT 0,
    color_r              REAL,
    color_g              REAL,
    color_b              REAL
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

CREATE TABLE IF NOT EXISTS variant_block_positions (
    variant_id    INTEGER NOT NULL REFERENCES variants(id),
    work_block_id INTEGER NOT NULL REFERENCES work_blocks(id),
    start_day     REAL    NOT NULL,
    duration_days REAL    NOT NULL,
    PRIMARY KEY (variant_id, work_block_id)
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

CREATE TABLE IF NOT EXISTS estimate_snapshots (
    id             INTEGER PRIMARY KEY,
    work_block_id  INTEGER NOT NULL REFERENCES work_blocks(id),
    duration_days  REAL    NOT NULL,
    confidence     REAL    NOT NULL,
    recorded_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS calendar_config (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    start_date            TEXT    NOT NULL DEFAULT '2025-01-01',
    working_days_per_week INTEGER NOT NULL DEFAULT 5
);

CREATE TABLE IF NOT EXISTS calendar_non_working_dates (
    date TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS t_shirt_sizes (
    label      TEXT    PRIMARY KEY,
    days       REAL    NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS confidence_factors (
    id     INTEGER PRIMARY KEY CHECK (id = 1),
    opt_50 REAL    NOT NULL DEFAULT 0.5,
    pes_50 REAL    NOT NULL DEFAULT 2.0,
    opt_75 REAL    NOT NULL DEFAULT 0.7,
    pes_75 REAL    NOT NULL DEFAULT 1.4
);

CREATE TABLE IF NOT EXISTS quarter_colors (
    quarter INTEGER PRIMARY KEY,
    color_r REAL    NOT NULL,
    color_g REAL    NOT NULL,
    color_b REAL    NOT NULL,
    color_a REAL    NOT NULL
);
";
