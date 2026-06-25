# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`brick_road` is a Bevy-based planning-simulation desktop app. The core idea (see `brick_road_prd_v0.3.md`) is that **the schedule is an emergent output, not a maintained artifact**: the user defines work, dependencies, resources, and a calendar, and the app computes/visualizes the timeline. It's a 2D timeline editor built on Bevy 0.18 + bevy_egui 0.39, with all state persisted to a local SQLite file.

## Commands

```bash
cargo dev          # run with bevy/dynamic_linking for faster incremental rebuilds (alias for `cargo run --features dev`)
cargo run          # run a normal (statically linked) build
cargo test         # run all tests
cargo test calendar::   # run tests in one module (e.g. calendar)
cargo build        # compile without running
```

There is no CI. The Rust toolchain is pinned via `rust-toolchain.toml` (1.96.0, with the `rustfmt` and `clippy` components) and `rustfmt.toml` pins `edition`/`style_edition` to 2021, so `cargo fmt` and `cargo clippy` produce identical output on every host (no cross-version formatting churn). Keep everything else at `cargo fmt` / `cargo clippy` defaults.

A repo-tracked **pre-commit hook** (`.githooks/pre-commit`) runs `cargo fmt --check` to keep the tree formatted. It is **not active until you enable it once per checkout/worktree**: `git config core.hooksPath .githooks`. It only checks formatting (fast); run `cargo clippy`/`cargo test` yourself before pushing (they need a full compile and are too slow per-commit). Bypass a single commit with `git commit --no-verify`.

The app opens/creates `brick_road.db` (gitignored SQLite file) in the working directory on launch. Delete it to reset to a freshly seeded demo plan.

## Architecture

This is a **single-binary Bevy app with no custom plugins** — everything is wired directly in `main()` (`src/main.rs`) as a long list of Bevy systems and inserted resources. There is no plugin module; to understand the app, read the `App::new()...run()` chain in `main.rs`, which is the authoritative system schedule and ordering.

### The central data flow

```
brick_road.db ──load_model──▶ Model (Resource, the single source of truth)
                                 │
                    ┌────────────┴─────────────────┐
                    ▼                               ▼
              sprites + gizmos              egui UI (top bar, settings
              (blocks, branch lanes,         flyout, block-inspector flyout)
               grid, markers)                + canvas drag/resize/drill
                    ▲                               │
                    │                               ▼
                    └──────────  Model  ◀── mutated in place
                                   │
                                   ▼
                       db::save_model (atomic, after every edit)
```

- **`Model`** (`model.rs`) is the one source of truth, a Bevy `Resource` holding `HashMap`s of all entities. Blocks are placed by **direct manipulation** (drag/resize/drill on the canvas + the egui block-inspector flyout) — there is no live auto-scheduler maintaining placement.
- **`Schedule`** (`schedule.rs`) is *derived* via `forward_pass` over the dependency graph. It is **not** a live placement engine — it seeds the demo plan and drives the compare-plan ghost overlay; `cascade_dependencies` handles push-on-drag. There is no analysis/critical-path layer (removed).
- **Persistence is auto-save**: there is no Save button. Every mutating UI action calls `db::save_model(&conn, &model)` immediately. When you add a new mutation path, you must add the `save_model` call too.

### Domain model (`model.rs`)

Read `model.rs` first — it defines the whole vocabulary. Key concepts:

- **WorkBlock** — a unit of effort: `start_day` + `duration_days` (integer working days), an optional `parent` (children are blocks whose `parent` points at it — the work-breakdown hierarchy), `t_shirt_size`, `priority`, `description`, optional HDR `color`, and a `rollup` flag (true → the parent's span is computed to span its children).
- **Plan** — a proposed future: `root_blocks` (top-level blocks), an optional `branch_start_day` (`None` = the main/trunk plan; `Some(d)` = a branch forked at working day `d`), and per-plan staffing — `row_names` (named resource lanes, keyed by drill scope) and `block_rows` (which lane each block sits in). Branches share blocks with main **by id** ("ghosts"); a branch may also add its own "owned" blocks. Structure (parent/children) is global; only staffing is per-plan.
- **Dependency** — a branch-local (`plan_id`) edge; `DependencyType` is FinishToStart / StartToStart / FinishToFinish / StartToFinish (no lag).
- **ResourceBlock** — `{ id, name, resource_type }` plus `non_working_dates: Vec<NonWorkingDate>` (that resource's vacation/leave/off-days). Resources are identified by **name** — a row-lane's name maps to a `ResourceBlock` by name.
- **NonWorkingDate** — `{ date, description }`. Global holidays live on `CalendarConfig.non_working_dates`; per-resource off-days live on each `ResourceBlock`. A resource's off-days grey and stretch only that resource's row (see `calendar.rs`).
- **CalendarConfig** — anchors day 0 to a real `start_date`, defines `working_days_per_week`, global `non_working_dates`, and `quarter_colors`. All timeline positions are integer **working days** (`type Day = i32`); convert to pixels with `calendar::day_to_x` (NOT a raw multiply — it inserts greyed holiday columns). Use `calendar.rs` (`date_to_day` / `day_to_date` / `day_to_x` / `x_to_day`) for any date/pixel arithmetic — do not hand-roll working-day math.

All entity IDs are opaque newtypes generated by the `id_newtype!` macro (`WorkBlockId`, `PlanId`, etc.). Never use raw `u64`; `Model::alloc_id` is the only ID source.

### Module map (`src/`)

| Module | Responsibility |
|---|---|
| `main.rs` | App assembly + system schedule; egui (`top_bar_ui`, settings flyout, calendar/grid); fork-on-click + branch markers. |
| `model.rs` | Domain entities + `Model` store + creation/mutation methods. Start here. |
| `db.rs` | SQLite: a single canonical `CREATE_TABLES_SQL` (no migrations) + `load_model` / `save_model`. |
| `schedule.rs` | `forward_pass` (seed/compare), `cascade_dependencies` (push-on-drag); `Schedule`, `DrillScope`, `VisibleBlocks`, `TodayMarker` resources. |
| `graph.rs` | Builds the per-plan dependency DAG and detects cycles. |
| `blocks.rs` | Block sprite reconciliation, drag/resize/select/name-edit/drill, dependency edges, the block-inspector flyout, undo, compare/past overlays. The interaction-heavy module. |
| `bands.rs` | Branch lanes/bands: per-branch rows, ghost-vs-owned block rendering, lane drag/edit, lane dependency drawing. |
| `calendar.rs` | Working-day ↔ pixel/date math (`day_to_x`/`x_to_day`/`date_to_day`/`day_to_date`), holiday columns (over a passed-in off-day set, so callers can union global + a resource's). |
| `labels.rs` | egui/world labels: block names, day-number ruler, quarter/period headers. |
| `camera.rs` | 2D pan/zoom with exponential smoothing (`smooth_camera`), fit-to-view, keyboard nav. |
| `constants.rs` | Layout constants: `PIXELS_PER_DAY`, `ROW_HEIGHT`, `GUTTER_WIDTH`. |

## Conventions & gotchas

- **System ordering is explicit and load-bearing.** `main.rs` uses `.before()`/`.after()`/`.chain()` extensively (e.g. `VisibleBlocks`/`Schedule` updates must run before sprite reconciliation). When adding a system, place it in the existing ordering rather than appending blindly, or you'll get a frame of stale visuals.
- **Change-detection guards.** Most derived systems early-return with `if !model.is_changed() { return; }` (or similar). Mutating a `ResMut<Model>` you don't actually change still trips `is_changed()` — only take `ResMut` when you will mutate.
- **The SQLite `Connection` is a `NonSend` resource** (rusqlite isn't `Send`). Access it with `NonSend<rusqlite::Connection>`; it cannot be used from a parallel system that requires `Send`.
- **The DB schema is a single canonical `CREATE_TABLES_SQL` — there are NO migrations.** The inline `ALTER`/`DROP` migration tail was collapsed away; `create_tables` just runs the one canonical `CREATE`. The local `brick_road.db` is disposable (gitignored), so a schema change is just an edit to `CREATE_TABLES_SQL` — **do not add `ALTER TABLE` migrations**. Pre-change DB files don't auto-upgrade; delete `brick_road.db` to regenerate. (Schema changes need the owner's approval — never add a new table; prefer a column/reference on an existing one.)
- **`save_model` is a full atomic rewrite** (delete join tables → upsert entities → delete stale rows → reinsert joins) in one transaction. Adding a new entity/relation means updating both `save_model` and `load_model`, plus `CREATE_TABLES_SQL`.
- **Colors are HDR linear RGB.** Values > 1.0 are intentional — they drive the bloom post-process (the camera uses `Hdr` + `Bloom` + `TonyMcMapface` tonemapping). The "today" marker and selection highlights rely on this.
- **Tests are inline `#[cfg(test)]` modules** next to the code they cover (model.rs, calendar.rs, schedule.rs, graph.rs, db.rs, blocks.rs, bands.rs, camera.rs, labels.rs); there is no `tests/` directory. Logic that's hard to test inside a Bevy system is extracted into a pure helper and tested directly — follow that pattern.

## Reference docs

- `brick_road_prd_v0.3.md` — product requirements (Worlds vs Plans, uncertainty model, simulation modes). The conceptual source of truth.
- `brick_road_bevy_spike_spec.md` — original tech spike (pinned crate versions, camera-feel/bloom/egui-coexistence validation). `README.md` is empty.
