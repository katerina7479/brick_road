use chrono::NaiveDate;
use rusqlite::{Connection, Result};

use crate::model::{
    AvailabilitySegment, AvailabilityTimeline, Dependency, DependencyId, DependencyType, Model,
    Plan, PlanId, ResourceAllocation, ResourceBlock, ResourceBlockId, ResourceType, TShirtSize,
    WorkBlock, WorkBlockId,
};

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    // Rename plan_root_blocks → plan_blocks on pre-existing DBs *before* the
    // fresh CREATE TABLEs run, so the legacy rows carry over. On a fresh DB (no
    // plan_root_blocks) or an already-renamed DB (plan_blocks present) this
    // fails harmlessly and is swallowed below.
    match conn.execute_batch("ALTER TABLE plan_root_blocks RENAME TO plan_blocks") {
        Ok(()) => {}
        Err(e)
            if e.to_string().contains("no such table")
                || e.to_string().contains("already exists") => {}
        Err(e) => return Err(e),
    }
    conn.execute_batch(CREATE_TABLES_SQL)?;
    // SQLite has no ADD COLUMN IF NOT EXISTS. Run each migration and ignore
    // the "duplicate column name" error that fires when it already exists.
    for sql in [
        "ALTER TABLE work_blocks ADD COLUMN start_day INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE work_blocks ADD COLUMN duration_days INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE work_blocks ADD COLUMN color_r REAL",
        "ALTER TABLE work_blocks ADD COLUMN color_g REAL",
        "ALTER TABLE work_blocks ADD COLUMN color_b REAL",
        "ALTER TABLE work_blocks ADD COLUMN description TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE work_blocks ADD COLUMN priority INTEGER NOT NULL DEFAULT 1",
        "ALTER TABLE work_blocks ADD COLUMN t_shirt_size TEXT",
        "ALTER TABLE work_blocks ADD COLUMN block_row INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE work_blocks ADD COLUMN parent_id INTEGER",
        "ALTER TABLE plans ADD COLUMN branch_start_day INTEGER",
    ] {
        match conn.execute_batch(sql) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(e),
        }
    }
    // Drop columns left over from removed features on pre-existing DBs. These
    // were NOT NULL with no default, so the current inserts (which no longer
    // mention them) would fail a NOT NULL constraint on the first save — this is
    // a correctness fix, not just tidiness. Already-gone columns (fresh or
    // already-migrated DB) report "no such column" and are swallowed.
    for sql in [
        "ALTER TABLE plans DROP COLUMN world_id",
        "ALTER TABLE plans DROP COLUMN parent_plan_id",
        "ALTER TABLE plans DROP COLUMN selected_variants",
        "ALTER TABLE resource_blocks DROP COLUMN world_id",
        "ALTER TABLE work_blocks DROP COLUMN estimate_most_likely",
        "ALTER TABLE work_blocks DROP COLUMN estimate_optimistic",
        "ALTER TABLE work_blocks DROP COLUMN estimate_pessimistic",
        "ALTER TABLE work_blocks DROP COLUMN estimate_confidence",
    ] {
        match conn.execute_batch(sql) {
            Ok(()) => {}
            Err(e)
                if e.to_string().contains("no such column")
                    || e.to_string().contains("cannot drop") => {}
            Err(e) => return Err(e),
        }
    }
    // Drop whole tables for removed features so a migrated DB matches a fresh
    // one. IF EXISTS makes these idempotent. The world_id foreign keys that
    // referenced `worlds` were dropped with the columns above.
    conn.execute_batch(
        "DROP TABLE IF EXISTS worlds;
         DROP TABLE IF EXISTS milestones;
         DROP TABLE IF EXISTS plan_milestone_targets;
         DROP TABLE IF EXISTS plan_variant_selections;
         DROP TABLE IF EXISTS plan_removed_inherited;
         DROP TABLE IF EXISTS estimate_snapshots;
         DROP TABLE IF EXISTS confidence_factors;
         DROP TABLE IF EXISTS variants;
         DROP TABLE IF EXISTS variant_children;
         DROP TABLE IF EXISTS variant_block_positions;",
    )?;
    // Seed default t-shirt sizes on first use (table created above).
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM t_shirt_sizes", [], |r| r.get(0))?;
    if count == 0 {
        // Week-based defaults (5 working days = 1 week). Editable in the size
        // settings popup.
        conn.execute_batch(
            "INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('XXS',   2, 0);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('XS',    5, 1);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('S',    10, 2);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('M',    15, 3);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('L',    25, 4);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('XL',   40, 5);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('XXL',  60, 6);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('3XL',  80, 7);
             INSERT INTO t_shirt_sizes (label, days, sort_order) VALUES ('4XL', 100, 8);",
        )?;
    }
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
        "DELETE FROM resource_allocations;
         DELETE FROM plan_blocks;
         DELETE FROM availability_segments;
         DELETE FROM calendar_non_working_dates;
         DELETE FROM t_shirt_sizes;
         DELETE FROM quarter_colors;",
    )?;

    // ── Phase 2: upsert all current entity rows ───────────────────────────────
    // INSERT … ON CONFLICT(id) DO UPDATE SET performs a genuine in-place
    // update on existing rows — no delete+insert — so WAL traffic is
    // proportional to changed rows rather than total row count.

    for rb in model.resource_blocks.values() {
        tx.execute(
            "INSERT INTO resource_blocks (id, name, resource_type)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 resource_type = excluded.resource_type",
            (
                rb.id.0 as i64,
                &rb.name,
                resource_type_str(rb.resource_type),
            ),
        )?;
    }

    for wb in model.work_blocks.values() {
        tx.execute(
            "INSERT INTO work_blocks
                 (id, name,
                  start_day, duration_days, color_r, color_g, color_b, description, priority,
                  t_shirt_size, block_row, parent_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 start_day = excluded.start_day,
                 duration_days = excluded.duration_days,
                 color_r = excluded.color_r,
                 color_g = excluded.color_g,
                 color_b = excluded.color_b,
                 description = excluded.description,
                 priority = excluded.priority,
                 t_shirt_size = excluded.t_shirt_size,
                 block_row = excluded.block_row,
                 parent_id = excluded.parent_id",
            (
                wb.id.0 as i64,
                &wb.name,
                wb.start_day as i64,
                wb.duration_days as i64,
                wb.color.map(|c| c[0] as f64),
                wb.color.map(|c| c[1] as f64),
                wb.color.map(|c| c[2] as f64),
                &wb.description,
                wb.priority as i64,
                &wb.t_shirt_size,
                wb.row as i64,
                wb.parent.map(|p| p.0 as i64),
            ),
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
                dep.lag as i64,
            ),
        )?;
    }

    for plan in model.plans.values() {
        tx.execute(
            "INSERT INTO plans (id, name, branch_start_day)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 branch_start_day = excluded.branch_start_day",
            (plan.id.0 as i64, &plan.name, plan.branch_start_day),
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
            (
                q as i64,
                color[0] as f64,
                color[1] as f64,
                color[2] as f64,
                color[3] as f64,
            ),
        )?;
    }

    // ── Phase 3: delete stale entity rows ─────────────────────────────────────
    // Processed in reverse FK order: child-referencing tables are deleted
    // before the tables they reference, so no FK constraint fires.
    // Join tables are already empty (cleared in phase 1), so entity rows
    // have no remaining FK children at this point.
    delete_stale(
        &tx,
        "plans",
        &model.plans.keys().map(|k| k.0).collect::<Vec<_>>(),
    )?;
    delete_stale(
        &tx,
        "dependencies",
        &model.dependencies.keys().map(|k| k.0).collect::<Vec<_>>(),
    )?;
    delete_stale(
        &tx,
        "resource_blocks",
        &model
            .resource_blocks
            .keys()
            .map(|k| k.0)
            .collect::<Vec<_>>(),
    )?;
    delete_stale(
        &tx,
        "work_blocks",
        &model.work_blocks.keys().map(|k| k.0).collect::<Vec<_>>(),
    )?;

    // ── Phase 4: reinsert join table rows for current entities ────────────────

    for rb in model.resource_blocks.values() {
        for (order, seg) in rb.availability.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO availability_segments
                     (resource_block_id, start_day, end_day, factor, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    rb.id.0 as i64,
                    seg.start as i64,
                    seg.end as i64,
                    seg.factor as f64,
                    order as i64,
                ),
            )?;
        }
    }

    for plan in model.plans.values() {
        for (order, &wb_id) in plan.root_blocks.iter().enumerate() {
            tx.execute(
                "INSERT INTO plan_blocks (plan_id, work_block_id, sort_order)
                 VALUES (?1, ?2, ?3)",
                (plan.id.0 as i64, wb_id.0 as i64, order as i64),
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
            (&size.label, size.days as i64, order as i64),
        )?;
    }

    tx.commit()
}

/// Deletes rows from `table` whose `id` column is not in `current_ids`.
/// If `current_ids` is empty the entire table is cleared (every row is stale).
/// Table names come from hardcoded call-sites so there is no injection risk.
fn delete_stale(tx: &rusqlite::Transaction<'_>, table: &str, current_ids: &[u64]) -> Result<()> {
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

    // resource_blocks
    {
        let mut stmt =
            conn.prepare("SELECT id, name, resource_type FROM resource_blocks ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, rt_str) = row?;
            let resource_type = parse_resource_type(&rt_str)?;
            bump!(id);
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
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
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
                    start: start as i32,
                    end: end as i32,
                    factor: factor as f32,
                });
            }
        }
    }

    // work_blocks
    {
        let mut stmt = conn.prepare(
            "SELECT id, name,
                    start_day, duration_days, color_r, color_g, color_b, description, priority,
                    t_shirt_size, block_row, parent_id
             FROM work_blocks",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<i64>>(11)?,
            ))
        })?;
        for row in rows {
            let (
                id,
                name,
                start_day,
                duration_days,
                cr,
                cg,
                cb,
                description,
                priority,
                t_shirt_size,
                block_row,
                parent_id,
            ) = row?;
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
                    parent: parent_id.map(|p| WorkBlockId(p as u64)),
                    start_day: start_day as i32,
                    duration_days: duration_days as i32,
                    row: block_row as i32,
                    color,
                    description,
                    priority: priority.clamp(0, 3) as u8,
                    t_shirt_size,
                },
            );
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
                row.get::<_, i64>(4)?,
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
                    lag: lag as i32,
                },
            );
        }
    }

    // plans
    {
        let mut stmt = conn.prepare("SELECT id, name, branch_start_day FROM plans")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<f64>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, branch_start_day) = row?;
            bump!(id);
            model.plans.insert(
                PlanId(id as u64),
                Plan {
                    id: PlanId(id as u64),
                    name,
                    root_blocks: vec![],
                    allocations: vec![],
                    branch_start_day: branch_start_day.map(|d| d as i32),
                },
            );
        }
    }

    // plan_blocks (order preserved)
    {
        let mut stmt = conn.prepare(
            "SELECT plan_id, work_block_id
             FROM plan_blocks
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
        let mut stmt = conn.prepare("SELECT date FROM calendar_non_working_dates ORDER BY date")?;
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
        let mut stmt = conn.prepare("SELECT label, days FROM t_shirt_sizes ORDER BY sort_order")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (label, days) = row?;
            model.t_shirt_sizes.push(TShirtSize {
                label,
                days: days as i32,
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
        if let Some(parent) = wb.parent {
            if !model.work_blocks.contains_key(&parent) {
                errors.push(format!(
                    "WorkBlock {} has parent WorkBlock {} which does not exist",
                    wb_id.0, parent.0
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

    for (plan_id, plan) in &model.plans {
        for &wb_id in &plan.root_blocks {
            if !model.work_blocks.contains_key(&wb_id) {
                errors.push(format!(
                    "Plan {} root_block WorkBlock {} does not exist",
                    plan_id.0, wb_id.0
                ));
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
        AvailabilitySegment, Day, DependencyType, ResourceAllocation, ResourceType, WorkBlockId,
    };
    use rusqlite::Connection;

    fn open_in_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    /// Creates a work block and sets its duration_days (the scheduler's
    /// placement field). Returns the new id.
    fn wb(m: &mut Model, name: &str, dur: Day) -> WorkBlockId {
        let id = m.create_work_block(name);
        m.work_blocks.get_mut(&id).unwrap().duration_days = dur;
        id
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
        wb(&mut m, "prep", 5);

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        assert_eq!(m.work_blocks, loaded.work_blocks);
        assert!(loaded.plans.is_empty());
        assert!(loaded.dependencies.is_empty());
        assert!(loaded.resource_blocks.is_empty());
    }

    #[test]
    fn full_model_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();

        // Resources created in ascending ID order so ORDER BY id on reload is deterministic.
        let rb1 = m.create_resource_block("Alice", ResourceType::Person);
        m.resource_blocks
            .get_mut(&rb1)
            .unwrap()
            .availability
            .segments
            .push(AvailabilitySegment {
                start: 0,
                end: 100,
                factor: 1.0,
            });
        m.resource_blocks
            .get_mut(&rb1)
            .unwrap()
            .availability
            .segments
            .push(AvailabilitySegment {
                start: 100,
                end: 200,
                factor: 0.5,
            });
        let rb2 = m.create_resource_block("Team Alpha", ResourceType::Team);

        let wb_a = wb(&mut m, "Design", 3);
        let wb_b = wb(&mut m, "Implement", 10);
        let wb_child = wb(&mut m, "Sub-task", 4);
        // wb_child is a child of wb_b — exercises the parent_id round-trip.
        m.work_blocks.get_mut(&wb_child).unwrap().parent = Some(wb_b);

        let dep_id = m.create_dependency(wb_a, wb_b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep_id).unwrap().lag = 1;

        let plan_id = m.create_plan("alpha", None);
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(wb_a);
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(wb_b);
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
        assert_eq!(m.dependencies, loaded.dependencies);
        assert_eq!(m.resource_blocks, loaded.resource_blocks);

        // Allocations have no sort_order column, so compare as sorted sets.
        assert_eq!(m.plans.len(), loaded.plans.len());
        for (pid, orig) in &m.plans {
            let got = loaded.plans.get(pid).expect("plan missing after load");
            assert_eq!(orig.name, got.name);
            assert_eq!(orig.root_blocks, got.root_blocks);
            let mut a = orig.allocations.clone();
            let mut b = got.allocations.clone();
            a.sort_by_key(|x| (x.resource_id.0, x.work_block_id.0));
            b.sort_by_key(|x| (x.resource_id.0, x.work_block_id.0));
            assert_eq!(a, b, "plan allocations mismatch");
        }
    }

    #[test]
    fn parent_id_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();
        let parent = m.create_work_block("parent");
        let child = m.create_work_block("child");
        m.work_blocks.get_mut(&child).unwrap().parent = Some(parent);

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        assert_eq!(loaded.work_blocks.get(&child).unwrap().parent, Some(parent));
        assert_eq!(loaded.work_blocks.get(&parent).unwrap().parent, None);
    }

    #[test]
    fn work_block_placement_fields_round_trip() {
        // Verify that non-default start_day and duration_days survive a
        // save_model → load_model cycle.
        let conn = open_in_memory();
        let mut m = Model::default();
        let id = m.create_work_block("task");
        let block = m.work_blocks.get_mut(&id).unwrap();
        block.start_day = 7;
        block.duration_days = 3;
        block.row = -2; // negative lane (above the baseline) must survive too

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        let loaded_wb = loaded.work_blocks.get(&id).unwrap();
        assert_eq!(loaded_wb.start_day, 7);
        assert_eq!(loaded_wb.duration_days, 3);
        assert_eq!(loaded_wb.row, -2);
        assert_eq!(m.work_blocks, loaded.work_blocks);
    }

    #[test]
    fn nonzero_start_day_and_duration_round_trip() {
        let conn = open_in_memory();
        let mut m = Model::default();
        let id = m.create_work_block("placed task");
        let block = m.work_blocks.get_mut(&id).unwrap();
        block.start_day = 3;
        block.duration_days = 7;

        save_model(&conn, &m).unwrap();
        let loaded = load_model(&conn).unwrap();

        let loaded_wb = loaded.work_blocks.get(&id).unwrap();
        assert_eq!(loaded_wb.start_day, 3);
        assert_eq!(loaded_wb.duration_days, 7);
    }

    #[test]
    fn migration_renames_plan_root_blocks_to_plan_blocks() {
        // Simulate upgrading a DB that still has the legacy plan_root_blocks
        // table with data, and verify create_tables migrates the rows into
        // plan_blocks so they load.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute_batch(
            "CREATE TABLE plans (id INTEGER PRIMARY KEY, name TEXT NOT NULL, world_id INTEGER);
             CREATE TABLE work_blocks (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             CREATE TABLE plan_root_blocks (
                 plan_id INTEGER NOT NULL,
                 work_block_id INTEGER NOT NULL,
                 sort_order INTEGER NOT NULL,
                 PRIMARY KEY (plan_id, sort_order)
             );
             INSERT INTO plans (id, name, world_id) VALUES (1, 'legacy plan', 0);
             INSERT INTO work_blocks (id, name) VALUES (5, 'legacy block');
             INSERT INTO plan_root_blocks (plan_id, work_block_id, sort_order)
                 VALUES (1, 5, 0);",
        )
        .unwrap();

        create_tables(&conn).unwrap();

        let model = load_model(&conn).unwrap();
        let plan = model.plans.get(&PlanId(1)).expect("legacy plan loaded");
        assert_eq!(plan.root_blocks, vec![WorkBlockId(5)]);
    }

    #[test]
    fn validate_accepts_valid_model() {
        let mut m = Model::default();
        let rb = m.create_resource_block("Alice", ResourceType::Person);
        let wb_a = m.create_work_block("a");
        let wb_b = m.create_work_block("b");
        let _dep = m.create_dependency(wb_a, wb_b, DependencyType::FinishToStart);
        let plan_id = m.create_plan("p", None);
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(wb_a);
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
    fn validate_catches_orphan_parent() {
        let mut m = Model::default();
        let child = m.create_work_block("child");
        m.work_blocks.get_mut(&child).unwrap().parent = Some(WorkBlockId(888));
        let err = validate_model(&m).unwrap_err().to_string();
        assert!(
            err.contains("888"),
            "expected missing parent ID 888 in: {err}"
        );
    }

    // ── Incremental / stale-deletion tests ───────────────────────────────────

    #[test]
    fn stale_work_block_deleted_on_second_save() {
        // Exercises the WHERE id NOT IN (…) path in delete_stale for work_blocks.
        let conn = open_in_memory();
        let mut m = Model::default();
        let wb_id = wb(&mut m, "to-be-removed", 3);

        save_model(&conn, &m).unwrap();
        let after_first = load_model(&conn).unwrap();
        assert!(after_first.work_blocks.contains_key(&wb_id));

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
        let conn = open_in_memory();
        let mut m = Model::default();
        let wb_id = wb(&mut m, "original", 5);

        save_model(&conn, &m).unwrap();

        m.work_blocks.get_mut(&wb_id).unwrap().name = "renamed".to_string();
        save_model(&conn, &m).unwrap();

        let loaded = load_model(&conn).unwrap();
        let block = loaded
            .work_blocks
            .get(&wb_id)
            .expect("block must still exist");
        assert_eq!(
            block.name, "renamed",
            "upsert must have written the new name"
        );
    }

    #[test]
    fn stale_deletion_with_empty_current_set() {
        let conn = open_in_memory();
        let mut m = Model::default();
        wb(&mut m, "block-a", 2);
        wb(&mut m, "block-b", 3);

        save_model(&conn, &m).unwrap();
        assert_eq!(load_model(&conn).unwrap().work_blocks.len(), 2);

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
        let conn = open_in_memory();
        let mut m = Model::default();
        let rb_id = m.create_resource_block("Alice", ResourceType::Person);
        let wb_a = wb(&mut m, "A", 2);
        let wb_b = wb(&mut m, "B", 3);
        let dep = m.create_dependency(wb_a, wb_b, DependencyType::FinishToStart);

        save_model(&conn, &m).unwrap();

        // Remove wb_b, the dependency referencing it, and the resource block.
        m.dependencies.remove(&dep);
        m.work_blocks.remove(&wb_b);
        m.resource_blocks.remove(&rb_id);
        save_model(&conn, &m).unwrap();

        let loaded = load_model(&conn).unwrap();
        assert!(
            !loaded.work_blocks.contains_key(&wb_b),
            "wb_b should be deleted"
        );
        assert!(
            !loaded.dependencies.contains_key(&dep),
            "dependency should be deleted"
        );
        assert!(
            !loaded.resource_blocks.contains_key(&rb_id),
            "resource_block should be deleted"
        );
        assert!(loaded.work_blocks.contains_key(&wb_a), "wb_a must survive");
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
                    (loaded.calendar.quarter_colors[q][ch] - m.calendar.quarter_colors[q][ch])
                        .abs()
                        < 1e-5,
                    "Q{q} channel {ch} mismatch"
                );
            }
        }
    }
}

const CREATE_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS resource_blocks (
    id            INTEGER PRIMARY KEY,
    name          TEXT    NOT NULL,
    resource_type TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS availability_segments (
    id                INTEGER PRIMARY KEY,
    resource_block_id INTEGER NOT NULL REFERENCES resource_blocks(id),
    start_day         INTEGER NOT NULL,
    end_day           INTEGER NOT NULL,
    factor            REAL    NOT NULL,
    sort_order        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS work_blocks (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT    NOT NULL,
    start_day            INTEGER NOT NULL DEFAULT 0,
    duration_days        INTEGER NOT NULL DEFAULT 0,
    color_r              REAL,
    color_g              REAL,
    color_b              REAL,
    parent_id            INTEGER
);

CREATE TABLE IF NOT EXISTS dependencies (
    id              INTEGER PRIMARY KEY,
    predecessor_id  INTEGER NOT NULL REFERENCES work_blocks(id),
    successor_id    INTEGER NOT NULL REFERENCES work_blocks(id),
    dependency_type TEXT    NOT NULL,
    lag_days        INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS plans (
    id                INTEGER PRIMARY KEY,
    name              TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS plan_blocks (
    plan_id       INTEGER NOT NULL REFERENCES plans(id),
    work_block_id INTEGER NOT NULL REFERENCES work_blocks(id),
    sort_order    INTEGER NOT NULL,
    PRIMARY KEY (plan_id, sort_order)
);

CREATE TABLE IF NOT EXISTS resource_allocations (
    id                INTEGER PRIMARY KEY,
    plan_id           INTEGER NOT NULL REFERENCES plans(id),
    resource_block_id INTEGER NOT NULL REFERENCES resource_blocks(id),
    work_block_id     INTEGER NOT NULL REFERENCES work_blocks(id),
    allocation_factor REAL    NOT NULL
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
    days       INTEGER NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS quarter_colors (
    quarter INTEGER PRIMARY KEY,
    color_r REAL    NOT NULL,
    color_g REAL    NOT NULL,
    color_b REAL    NOT NULL,
    color_a REAL    NOT NULL
);
";
