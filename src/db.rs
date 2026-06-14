use rusqlite::{Connection, Result};

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(CREATE_TABLES_SQL)?;
    Ok(())
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
