use bevy::{
    core_pipeline::tonemapping::Tonemapping, post_process::bloom::Bloom, prelude::*,
    render::view::Hdr,
};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use chrono::Datelike;

pub mod bands;
pub mod blocks;
pub mod calendar;
pub mod camera;
pub mod constants;
pub mod csv_export;
pub mod datepicker;
pub mod db;
pub mod document;
pub mod graph;
pub mod labels;
pub mod model;
pub mod schedule;
pub mod theme;

use camera::{camera_nav_keys, smooth_camera, update_camera_target, CameraTarget};
use constants::PIXELS_PER_DAY;
use model::Day;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "brick_road".to_string(),
                resolution: (1400u32, 700u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        // BG: rgb(10, 14, 16) linearised for Bevy's sRGB colour space.
        .insert_resource(ClearColor(Color::srgb(0.039, 0.055, 0.063)))
        .insert_resource(CameraTarget::default())
        .insert_resource(blocks::SelectedBlock::default())
        .insert_resource(blocks::SelectedDependency::default())
        .insert_resource(blocks::NameEditState::default())
        .insert_resource(blocks::DragState::default())
        .insert_resource(blocks::ResizeDragState::default())
        .insert_resource(blocks::DepDragState::default())
        .insert_resource(blocks::UndoStack::default())
        .insert_resource(blocks::Clipboard::default())
        .insert_resource(blocks::SelectedBlocks::default())
        .insert_resource(blocks::MarqueeState::default())
        .insert_resource(blocks::CreateModeState::default())
        .insert_resource(blocks::BlockInspectorState::default())
        .insert_resource(schedule::VisibleBlocks::default())
        .insert_resource(schedule::PersonViewCache::default())
        .insert_resource(schedule::DrillScope::default())
        .insert_resource(ViewMode::default())
        .insert_resource(schedule::TodayMarker::default())
        .insert_resource(blocks::BlockSpriteMap::default())
        .insert_resource(blocks::ComparePlanState::default())
        .insert_resource(blocks::CompareBlockSpriteMap::default())
        .insert_resource(blocks::CompareScheduleCache::default())
        .insert_resource(ForkHoverState::default())
        .insert_resource(SelectedPlan::default())
        .insert_resource(RowRename::default())
        .insert_resource(SettingsState::default())
        .insert_resource(HelpState::default())
        .insert_resource(ImportState::default())
        .insert_resource(bands::BandEntities::default())
        .insert_resource(bands::PlanRenameState::default())
        .insert_resource(bands::LaneSelection::default())
        .insert_resource(bands::LaneDrag::default())
        .insert_resource(bands::LaneBlockRename::default())
        .insert_resource(bands::LaneDepDrag::default())
        .insert_resource(document::PendingDocument::default())
        .insert_resource(document::FileMenuState::default())
        .add_systems(Update, apply_document_request)
        .add_systems(Update, sync_window_title)
        .add_systems(Startup, (setup_db, setup_camera))
        .add_systems(Startup, setup_demo_schedule.after(setup_db))
        .add_systems(
            PostStartup,
            schedule::update_visible_blocks.before(blocks::reconcile_block_sprites),
        )
        .add_systems(PostStartup, blocks::reconcile_block_sprites)
        .add_systems(
            PostStartup,
            sync_weekend_bands.after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            PostStartup,
            sync_resource_offday_bands.after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            PostStartup,
            sync_period_bands.after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            Update,
            set_initial_view.after(schedule::update_today_marker),
        )
        .add_systems(
            Update,
            (camera_nav_keys, update_camera_target, smooth_camera).chain(),
        )
        .add_systems(Update, draw_grid)
        .add_systems(Update, frame_on_drill)
        .add_systems(Update, draw_parent_bounds)
        // Plan/branch UI only exists at the plan level — drilling into a block is
        // a focused view of just that block's children (no branches).
        .add_systems(Update, draw_branch_markers.run_if(at_plan_level))
        .add_systems(Update, bands::draw_band_overlays.run_if(at_plan_level))
        .add_systems(Update, bands::sync_band_visuals)
        .add_systems(
            Update,
            // After branch-marker selection so a name click (which disambiguates
            // overlapping same-day forks by height) wins over the nearest marker.
            bands::handle_band_rename_click
                .run_if(at_plan_level)
                .run_if(editing_enabled)
                .after(handle_branch_selection),
        )
        .add_systems(
            Update,
            bands::handle_band_block_create
                .run_if(at_plan_level)
                .run_if(editing_enabled)
                .before(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            bands::handle_lane_dep_drag
                .run_if(at_plan_level)
                .run_if(editing_enabled)
                .before(bands::handle_lane_block_edit),
        )
        .add_systems(
            Update,
            bands::handle_lane_block_edit
                .run_if(at_plan_level)
                .run_if(editing_enabled)
                .before(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            bands::handle_lane_block_delete.run_if(at_plan_level),
        )
        .add_systems(Update, bands::draw_lane_dependencies.run_if(at_plan_level))
        .add_systems(
            Update,
            bands::clear_lane_selection_on_main_select.after(blocks::handle_block_selection),
        )
        .add_systems(Update, handle_fork_hover.run_if(at_plan_level))
        .add_systems(
            Update,
            handle_branch_selection
                .run_if(at_plan_level)
                .before(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            handle_branch_delete.after(blocks::handle_block_drill),
        )
        .add_systems(Update, schedule::update_today_marker)
        .add_systems(Update, sync_total_duration)
        .add_systems(Update, sync_weekend_bands.after(sync_total_duration))
        .add_systems(
            Update,
            sync_resource_offday_bands.after(sync_total_duration),
        )
        .add_systems(Update, sync_period_bands.after(sync_total_duration))
        .add_systems(
            Update,
            schedule::update_person_view
                .before(schedule::update_visible_blocks)
                .after(blocks::handle_block_delete)
                .after(blocks::handle_undo)
                .after(blocks::handle_paste),
        )
        .add_systems(
            Update,
            schedule::update_visible_blocks
                .before(blocks::reconcile_block_sprites)
                .before(blocks::draw_dependency_edges)
                .before(blocks::draw_block_handles)
                .after(schedule::update_person_view)
                .after(blocks::handle_block_delete)
                .after(blocks::handle_undo)
                .after(blocks::handle_paste),
        )
        .add_systems(Update, blocks::handle_block_drill.run_if(editing_enabled))
        .add_systems(Update, blocks::handle_drill_out.run_if(editing_enabled))
        // Runs before the keyboard shortcut handlers so the first typed
        // character opens the rename instead of triggering F/N/Home.
        .add_systems(
            Update,
            blocks::handle_type_to_rename
                .run_if(editing_enabled)
                .before(camera_nav_keys)
                .before(blocks::handle_create_mode_toggle),
        )
        .add_systems(
            Update,
            blocks::handle_canvas_create
                .run_if(editing_enabled)
                .after(blocks::handle_block_drill),
        )
        .add_systems(
            Update,
            blocks::handle_block_delete
                .run_if(editing_enabled)
                .after(blocks::handle_block_drill),
        )
        .add_systems(Update, blocks::handle_undo)
        .add_systems(Update, blocks::handle_copy)
        .add_systems(Update, blocks::handle_paste.after(blocks::handle_copy))
        .add_systems(Update, blocks::handle_open_url)
        .add_systems(
            Update,
            blocks::handle_create_mode_toggle.after(blocks::handle_block_drill),
        )
        .add_systems(Update, blocks::handle_create_mode_click_exit)
        .add_systems(
            Update,
            blocks::handle_marquee_select
                .run_if(editing_enabled)
                .after(blocks::handle_block_drill)
                .before(blocks::handle_block_selection),
        )
        .add_systems(Update, blocks::draw_marquee)
        .add_systems(
            Update,
            blocks::handle_block_selection
                .run_if(editing_enabled)
                .after(blocks::handle_block_drill),
        )
        .add_systems(
            Update,
            blocks::handle_block_resize
                .run_if(editing_enabled)
                .after(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            blocks::handle_block_drag
                .run_if(editing_enabled)
                .after(blocks::handle_block_selection)
                .after(blocks::handle_block_resize),
        )
        .add_systems(
            Update,
            blocks::reconcile_block_sprites.after(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            blocks::sync_block_sprites
                .after(blocks::handle_block_drag)
                .after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            Update,
            blocks::draw_block_borders.after(blocks::sync_block_sprites),
        )
        .add_systems(
            Update,
            blocks::sync_past_overlays
                .after(blocks::reconcile_block_sprites)
                .after(schedule::update_today_marker),
        )
        .add_systems(
            Update,
            blocks::sync_compare_overlays.after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            Update,
            blocks::handle_dep_drag
                .run_if(editing_enabled)
                .before(blocks::handle_block_selection)
                .before(blocks::handle_block_drag)
                .before(blocks::handle_block_resize),
        )
        .add_systems(Update, blocks::draw_block_handles)
        .add_systems(Update, blocks::update_cursor_icon)
        .add_systems(Update, blocks::draw_dependency_edges)
        .add_systems(
            Update,
            blocks::sync_block_labels.after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            Update,
            blocks::sync_block_label_names
                .after(blocks::reconcile_block_sprites)
                .before(blocks::sync_block_labels),
        )
        .add_systems(
            Update,
            blocks::sync_description_dots.after(blocks::reconcile_block_sprites),
        )
        .add_systems(EguiPrimaryContextPass, top_bar_ui)
        .add_systems(EguiPrimaryContextPass, calendar_ruler_ui.after(top_bar_ui))
        .add_systems(EguiPrimaryContextPass, settings_flyout_ui.after(top_bar_ui))
        .add_systems(EguiPrimaryContextPass, help_modal_ui.after(top_bar_ui))
        .add_systems(EguiPrimaryContextPass, import_modal_ui.after(top_bar_ui))
        .add_systems(
            EguiPrimaryContextPass,
            blocks::block_inspector_flyout_ui.after(settings_flyout_ui),
        )
        .add_systems(
            EguiPrimaryContextPass,
            resource_gutter_ui.after(calendar_ruler_ui),
        )
        .add_systems(EguiPrimaryContextPass, blocks::draw_name_edit_overlay)
        .add_systems(EguiPrimaryContextPass, blocks::draw_create_mode_overlay)
        .add_systems(EguiPrimaryContextPass, blocks::draw_block_tooltip)
        .add_systems(EguiPrimaryContextPass, bands::draw_plan_rename_overlay)
        .add_systems(
            EguiPrimaryContextPass,
            bands::draw_lane_block_rename_overlay,
        )
        .run();
}

/// Returns the path to `brick_road.db` in the per-user data directory,
/// creating the directory if needed.
///
/// On macOS: `~/Library/Application Support/brick_road/brick_road.db`
/// On Linux: `~/.local/share/brick_road/brick_road.db`
/// On Windows: `%APPDATA%\katerina7479\brick_road\data\brick_road.db`
///
/// One-time migration: if `./brick_road.db` (legacy cwd path) exists and the
/// new location is empty, the file is moved there so existing data carries over.
/// Falls back to the cwd on the rare chance `ProjectDirs` cannot resolve a home
/// directory (e.g. running as a service with no home).
/// Moves `cwd_db` to `new_db` when only the legacy cwd path exists.
///
/// No-op when `new_db` already exists (never clobbers existing data) or when
/// `cwd_db` is absent.  Tries an atomic `rename` first; on cross-filesystem
/// paths falls back to copy-to-temp then atomic rename so `new_db` is never
/// observed in a partial state.
fn migrate_legacy_db(cwd_db: &std::path::Path, new_db: &std::path::Path) {
    if !cwd_db.exists() || new_db.exists() {
        return;
    }
    if std::fs::rename(cwd_db, new_db).is_ok() {
        return;
    }
    // Cross-filesystem: copy to a temp path in the same directory as new_db,
    // then atomically rename so new_db is never seen partially written.
    let tmp = new_db.with_extension("tmp");
    match std::fs::copy(cwd_db, &tmp) {
        Ok(_) => match std::fs::rename(&tmp, new_db) {
            Ok(_) => {
                let _ = std::fs::remove_file(cwd_db);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                eprintln!("warning: could not migrate {cwd_db:?} → {new_db:?}: {e}");
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            eprintln!("warning: could not copy {cwd_db:?} to temp: {e}");
        }
    }
}

/// Returns the path to `brick_road.db` in the per-user data directory,
/// creating the directory if needed.
///
/// On macOS: `~/Library/Application Support/brick_road/brick_road.db`
/// On Linux: `~/.local/share/brick_road/brick_road.db`
/// On Windows: `%APPDATA%\katerina7479\brick_road\data\brick_road.db`
///
/// Falls back to the cwd when `ProjectDirs` cannot resolve a home directory
/// (e.g. running as a service with no home).  Performs a one-time migration of
/// any legacy `./brick_road.db` before returning the new path.
fn resolve_db_path() -> std::path::PathBuf {
    let data_dir = document::app_data_dir();
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("warning: could not create data dir {data_dir:?}: {e}");
    }

    let new_db = data_dir.join("brick_road.db");
    let cwd_db = std::path::Path::new("brick_road.db");
    migrate_legacy_db(cwd_db, &new_db);
    new_db
}

#[cfg(test)]
mod db_path_tests {
    use super::migrate_legacy_db;
    use std::fs;

    #[test]
    fn migrate_moves_legacy_db_to_new_location() {
        let dir = tempfile::tempdir().unwrap();
        let cwd_db = dir.path().join("cwd.db");
        let new_db = dir.path().join("new.db");
        fs::write(&cwd_db, b"data").unwrap();
        migrate_legacy_db(&cwd_db, &new_db);
        assert!(!cwd_db.exists(), "legacy cwd DB should have been removed");
        assert_eq!(fs::read(&new_db).unwrap(), b"data");
    }

    #[test]
    fn migrate_does_not_clobber_existing_new_db() {
        let dir = tempfile::tempdir().unwrap();
        let cwd_db = dir.path().join("cwd.db");
        let new_db = dir.path().join("new.db");
        fs::write(&cwd_db, b"old").unwrap();
        fs::write(&new_db, b"current").unwrap();
        migrate_legacy_db(&cwd_db, &new_db);
        assert_eq!(
            fs::read(&new_db).unwrap(),
            b"current",
            "new DB must not be clobbered"
        );
        assert!(
            cwd_db.exists(),
            "cwd DB must be untouched when new DB exists"
        );
    }

    #[test]
    fn migrate_no_op_when_cwd_absent() {
        let dir = tempfile::tempdir().unwrap();
        let cwd_db = dir.path().join("cwd.db");
        let new_db = dir.path().join("new.db");
        migrate_legacy_db(&cwd_db, &new_db);
        assert!(
            !new_db.exists(),
            "no new DB should be created when cwd DB is absent"
        );
    }
}

fn setup_db(world: &mut World) {
    // Reopen the most recent document; first run (or all recents deleted)
    // falls back to the legacy default DB, which resolve_db_path migrates.
    let db_path = document::load_recents()
        .into_iter()
        .next()
        .unwrap_or_else(resolve_db_path);
    let conn = rusqlite::Connection::open(&db_path)
        .unwrap_or_else(|e| panic!("failed to open DB at {db_path:?}: {e}"));
    db::create_tables(&conn).expect("failed to create DB tables");
    let mut model = db::load_model(&conn).expect("failed to load model");
    // Events never repeat into plans; sweep ghosts saved before that rule.
    if model.prune_event_ghosts_from_branches() {
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
    document::remember_document(&db_path);
    world.insert_resource(document::CurrentDocument(db_path));
    world.insert_resource(model);
    world.insert_non_send_resource(conn);
}

/// Keeps the OS window title in sync with the open document.
fn sync_window_title(doc: Res<document::CurrentDocument>, mut windows: Query<&mut Window>) {
    if !doc.is_changed() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.title = format!("brick_road — {}", document::doc_display_name(&doc.0));
}

/// Applies a pending FILE-menu request at a safe point in the frame.
///
/// `Open`/`New` swap in the target document wholesale: new DB connection and
/// `Model`, per-document transient state reset (selection, drill, undo,
/// inspector, compare, in-flight gestures), the derived `Schedule` rebuilt,
/// and the camera sent Home on the new calendar. The `Clipboard` deliberately
/// survives — paste re-mints ids, so blocks copy across documents. Sprite maps
/// are left alone: reconciliation diffs them against the new visible set.
///
/// `Duplicate` writes the current model to the new file and re-points the
/// connection at it; all working state carries over (same content, new file).
///
/// Any failure (unwritable path, not a brick_road document) logs and leaves
/// the current document untouched.
fn apply_document_request(world: &mut World) {
    let Some(req) = world.resource_mut::<document::PendingDocument>().0.take() else {
        return;
    };
    let path = match &req {
        document::DocRequest::Open(p)
        | document::DocRequest::New(p)
        | document::DocRequest::Duplicate(p) => p.clone(),
    };

    // New/Duplicate write a fresh file. The native save dialog already
    // confirmed any overwrite, so clear stale content instead of merging
    // tables into whatever the file held before.
    if !matches!(req, document::DocRequest::Open(_)) && path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            error!("could not overwrite {path:?}: {e}");
            return;
        }
    }

    let conn = match rusqlite::Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            error!("could not open {path:?}: {e}");
            return;
        }
    };
    if let Err(e) = db::create_tables(&conn) {
        error!("{path:?} is not a usable brick_road document: {e}");
        return;
    }

    // Duplicate: persist the current model into the copy and keep working.
    if matches!(req, document::DocRequest::Duplicate(_)) {
        {
            let model = world.resource::<model::Model>();
            if let Err(e) = db::save_model(&conn, model) {
                error!("duplicate to {path:?} failed: {e}");
                return;
            }
        }
        world.insert_non_send_resource(conn);
        world.insert_resource(document::CurrentDocument(path.clone()));
        document::remember_document(&path);
        return;
    }

    let mut model = match req {
        document::DocRequest::New(_) => {
            let m = document::blank_document_model(&path);
            if let Err(e) = db::save_model(&conn, &m) {
                error!("could not initialise {path:?}: {e}");
                return;
            }
            m
        }
        _ => match db::load_model(&conn) {
            Ok(m) => m,
            Err(e) => {
                error!("{path:?} is not a brick_road document: {e}");
                return;
            }
        },
    };
    if model.prune_event_ghosts_from_branches() {
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }

    // Derived state computed from the local model before it moves into the
    // world: the Schedule (mirrors setup_demo_schedule's loaded-data path)
    // and the Home camera target on the new calendar.
    let sched = model
        .plans
        .values()
        .min_by_key(|p| (p.branch_start_day.is_some(), p.id.0))
        .cloned()
        .and_then(|plan| {
            let g = graph::build_graph(&model, &plan);
            schedule::forward_pass(&model, &g).ok()
        })
        .unwrap_or_default();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let today_date = calendar::unix_secs_to_date(secs);
    let today_day = calendar::today_marker_day(today_date, &model.calendar);
    let camera_target = {
        let mut q = world.query::<&Window>();
        q.single(world)
            .ok()
            .map(|w| camera::home_target(w, today_day, &model.calendar))
    };

    world.insert_non_send_resource(conn);
    world.insert_resource(document::CurrentDocument(path.clone()));
    document::remember_document(&path);
    world.insert_resource(model);
    world.insert_resource(sched);
    if let Some(target) = camera_target {
        world.insert_resource(target);
    }

    // Per-document transient state: anything holding ids, gestures, or edits
    // from the previous document resets to defaults. Sprite/entity maps stay —
    // the reconcile systems despawn stale entities against the new model.
    world.insert_resource(schedule::DrillScope::default());
    world.insert_resource(blocks::SelectedBlock::default());
    world.insert_resource(blocks::SelectedBlocks::default());
    world.insert_resource(blocks::SelectedDependency::default());
    world.insert_resource(blocks::UndoStack::default());
    world.insert_resource(blocks::NameEditState::default());
    world.insert_resource(blocks::BlockInspectorState::default());
    world.insert_resource(blocks::DragState::default());
    world.insert_resource(blocks::ResizeDragState::default());
    world.insert_resource(blocks::DepDragState::default());
    world.insert_resource(blocks::MarqueeState::default());
    world.insert_resource(blocks::CreateModeState::default());
    world.insert_resource(blocks::ComparePlanState::default());
    world.insert_resource(blocks::CompareScheduleCache::default());
    world.insert_resource(bands::LaneSelection::default());
    world.insert_resource(bands::LaneDepDrag::default());
    world.insert_resource(bands::LaneDrag::default());
    world.insert_resource(bands::LaneBlockRename::default());
    world.insert_resource(bands::PlanRenameState::default());
    world.insert_resource(SelectedPlan::default());
    world.insert_resource(RowRename::default());
    world.insert_resource(SettingsState::default());
    world.insert_resource(ImportState::default());
}

fn setup_camera(mut commands: Commands) {
    commands.spawn((Camera2d, Hdr, Tonemapping::TonyMcMapface, Bloom::default()));
}

/// Run condition: true at the plan's top level (not drilled into a block). The
/// branch/plan UI runs only here; drilling into a block is a focused view of
/// just that block's children.
fn at_plan_level(drill: Res<schedule::DrillScope>) -> bool {
    drill.path.is_empty()
}

fn editing_enabled(view: Res<ViewMode>) -> bool {
    !view.by_person
}

/// On a drill-in/out change, reframe the camera: drilling into a block frames
/// that block's span (with slack to place children beyond it); drilling back to
/// the plan level fits the plan's blocks (or returns to the today/home view).
fn frame_on_drill(
    drill: Res<schedule::DrillScope>,
    model: Res<model::Model>,
    today: Res<schedule::TodayMarker>,
    windows: Query<&Window>,
    mut target: ResMut<CameraTarget>,
    mut cam: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    if !drill.is_changed() {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let new_target = match drill.current().and_then(|id| model.work_blocks.get(&id)) {
        Some(wb) => camera::frame_day_span(
            window,
            wb.start_day,
            wb.start_day + wb.duration_days,
            &model.calendar,
        ),
        None => model
            .main_plan_id()
            .and_then(|p| camera::fit_to_blocks(&model, p, &windows))
            .unwrap_or_else(|| camera::home_target(window, today.day, &model.calendar)),
    };
    let (pos, zoom) = (new_target.pos, new_target.zoom);
    *target = new_target;
    // Snap the camera (don't ease): a programmatic reframe must finish instantly
    // so a double-click to create a block right after maps to the cursor — an
    // in-progress ease would place the block offset from where you clicked.
    if let Ok((mut tf, mut proj)) = cam.single_mut() {
        tf.translation.x = pos.x;
        tf.translation.y = pos.y;
        if let Projection::Orthographic(o) = &mut *proj {
            o.scale = zoom;
        }
    }
}

/// While drilled into a block, draws vertical boundary lines at the parent
/// block's start and end days, so children placed beyond them read as "outside
/// the parent" (where the roll-up toggle decides whether the parent grows).
fn draw_parent_bounds(
    mut gizmos: Gizmos,
    drill: Res<schedule::DrillScope>,
    model: Res<model::Model>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Some(wb) = drill.current().and_then(|id| model.work_blocks.get(&id)) else {
        return;
    };
    let Ok((cam_t, proj)) = cam_q.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let Ok(window) = windows.single() else { return };
    let half_h = (window.height() * 0.5 * ortho.scale).max(800.0);
    let y_top = cam_t.translation.y + half_h;
    let y_bot = cam_t.translation.y - half_h;
    let color = Color::from(LinearRgba::new(2.4, 1.6, 0.3, 0.5)); // amber, bloomed

    let off = model.calendar.global_off_days();
    for day in [wb.start_day, wb.start_day + wb.duration_days] {
        let x = calendar::day_to_x(day, &off, &model.calendar);
        gizmos.line_2d(Vec2::new(x, y_top), Vec2::new(x, y_bot), color);
    }
}

/// On launch, snap the camera to the "Home" view (today at upper-left, main plan
/// at the top) once `today` is known. Runs a single time; afterwards the user
/// drives the camera. Snapping (not easing from the origin) avoids an opening
/// pan across pre-plan emptiness.
fn set_initial_view(
    mut done: Local<bool>,
    mut target: ResMut<CameraTarget>,
    today: Res<schedule::TodayMarker>,
    model: Res<model::Model>,
    windows: Query<&Window>,
    mut cam: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    if *done {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let home = camera::home_target(window, today.day, &model.calendar);
    let (pos, zoom) = (home.pos, home.zoom);
    *target = home;
    if let Ok((mut tf, mut proj)) = cam.single_mut() {
        tf.translation.x = pos.x;
        tf.translation.y = pos.y;
        if let Projection::Orthographic(o) = &mut *proj {
            o.scale = zoom;
        }
    }
    *done = true;
}

fn draw_grid(
    mut gizmos: Gizmos,
    today: Res<schedule::TodayMarker>,
    model: Res<model::Model>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let line_color = Color::srgba(0.42, 0.46, 0.60, 0.13);
    let past_line_color = Color::srgba(0.38, 0.42, 0.55, 0.06);
    let baseline_color = Color::srgba(0.50, 0.55, 0.70, 0.28);
    let today_line_color = Color::from(LinearRgba::new(4.0, 2.0, 0.5, 1.0)); // HDR → Bloom

    let Ok((cam_t, proj)) = cam_q.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let Ok(window) = windows.single() else { return };

    let scale = ortho.scale;
    let cam_x = cam_t.translation.x;
    let cam_y = cam_t.translation.y;

    // Visible world-space extents with a one-day margin to avoid edge pop-in.
    let half_w = (window.width() * 0.5 + PIXELS_PER_DAY) * scale;
    let half_h = (window.height() * 0.5 + 100.0) * scale;

    let x_left = cam_x - half_w;
    let x_right = cam_x + half_w;
    let y_bottom = cam_y - half_h;
    let y_top = cam_y + half_h;

    // Iterate visual columns (which include inserted holiday columns), drawing a
    // boundary line at each. Past/future colouring uses the working day the
    // column maps back to.
    let cal = &model.calendar;
    let off = cal.global_off_days();
    let v_min = (x_left / PIXELS_PER_DAY).floor() as i32;
    let v_max = (x_right / PIXELS_PER_DAY).ceil() as i32;

    for v in v_min..=v_max {
        let x = v as f32 * PIXELS_PER_DAY;
        let day = calendar::x_to_day(x, &off, cal);
        let color = if day < today.day {
            past_line_color
        } else {
            line_color
        };
        gizmos.line_2d(Vec2::new(x, y_bottom), Vec2::new(x, y_top), color);
    }

    // Faint horizontal hints at the row (lane) boundaries, so you can sense where
    // blocks will snap vertically without a heavy grid. Boundaries sit halfway
    // between row centers: y = (k + 0.5) * ROW_HEIGHT.
    let row_hint = Color::srgba(0.45, 0.50, 0.66, 0.05);
    let rh = constants::ROW_HEIGHT;
    let k_min = (y_bottom / rh - 0.5).floor() as i32;
    let k_max = (y_top / rh - 0.5).ceil() as i32;
    for k in k_min..=k_max {
        let y = (k as f32 + 0.5) * rh;
        gizmos.line_2d(Vec2::new(x_left, y), Vec2::new(x_right, y), row_hint);
    }

    gizmos.line_2d(
        Vec2::new(x_left, 0.0),
        Vec2::new(x_right, 0.0),
        baseline_color,
    );

    // Prominent today marker — draw 3 lines 2px apart so it reads as a thick bar at all zooms.
    let x_today = calendar::day_to_x(today.day, &off, cal);
    for dx in [-2.0_f32, 0.0, 2.0] {
        gizmos.line_2d(
            Vec2::new(x_today + dx, y_bottom),
            Vec2::new(x_today + dx, y_top),
            today_line_color,
        );
    }
}

/// Marker for weekend and holiday band sprites behind the timeline grid.
#[derive(Component)]
struct WeekendBand;

/// Marker for per-resource non-working-day column sprites (row-height only).
#[derive(Component)]
struct ResourceOffDayBand;

/// World x of each compressed-weekend seam within the span: a thin marker at
/// every real calendar-week boundary (where consecutive working days fall in
/// different ISO weeks), holiday-shifted. Anchored to actual weeks, not counted
/// every `working_days_per_week` from day 0, so it stays correct whatever
/// weekday the calendar starts on.
fn weekend_band_positions(span_days: i32, model: &model::Model) -> Vec<f32> {
    use chrono::Datelike;
    let cal = &model.calendar;
    let off = cal.global_off_days();
    let mut positions = Vec::new();
    for day in 0..=span_days + 1 {
        let here = calendar::day_to_date(day, cal);
        let next = calendar::day_to_date(day + 1, cal);
        if here.iso_week() != next.iso_week() {
            positions.push(calendar::day_to_x(day + 1, &off, cal));
        }
    }
    positions
}

fn sync_weekend_bands(
    model: Res<model::Model>,
    schedule: Res<schedule::Schedule>,
    mut commands: Commands,
    band_q: Query<Entity, With<WeekendBand>>,
) {
    if !model.is_changed() && !schedule.is_changed() {
        return;
    }
    for e in &band_q {
        commands.entity(e).despawn();
    }

    let span = schedule.total_duration_days.max(CALENDAR_HORIZON_DAYS) + 10;

    // Thin seams where weekends are compressed out.
    let weekend_color = Color::srgba(0.22, 0.26, 0.42, 0.09);
    for x in weekend_band_positions(span, &model) {
        commands.spawn((
            WeekendBand,
            Sprite {
                color: weekend_color,
                custom_size: Some(Vec2::new(8.0, 20_000.0)),
                ..default()
            },
            Transform::from_xyz(x, 0.0, -0.5),
        ));
    }

    // Holidays occupy a full greyed day-wide column that work skips.
    let holiday_color = Color::srgba(0.48, 0.50, 0.56, 0.20);
    for (left_x, _date, _desc) in
        calendar::holiday_columns(&model.calendar.global_off_days(), &model.calendar, span)
    {
        commands.spawn((
            WeekendBand,
            Sprite {
                color: holiday_color,
                custom_size: Some(Vec2::new(PIXELS_PER_DAY, 20_000.0)),
                ..default()
            },
            Transform::from_xyz(left_x + PIXELS_PER_DAY * 0.5, 0.0, -0.5),
        ));
    }
}

/// Spawns row-height grey columns for per-resource non-working dates (PTO,
/// offsite, etc.) in the task view. Each column is PIXELS_PER_DAY wide and
/// ROW_HEIGHT tall, centred on the resource's row y. The column is positioned
/// using the row-augmented off-day set (global ∪ resource off-days) so it
/// aligns with the gap `block_span_x` opens for the same date on the same row.
fn sync_resource_offday_bands(
    model: Res<model::Model>,
    schedule: Res<schedule::Schedule>,
    mut commands: Commands,
    band_q: Query<Entity, With<ResourceOffDayBand>>,
) {
    if !model.is_changed() && !schedule.is_changed() {
        return;
    }
    for e in &band_q {
        commands.entity(e).despawn();
    }

    let cal = &model.calendar;
    // Build the same (global, row_offs) map that sync_block_sprites uses so
    // band x-positions are computed against the identical off-day sets.
    let (global_offs, row_offs_map) = blocks::compute_row_offs(&model);
    let span = schedule.total_duration_days.max(CALENDAR_HORIZON_DAYS) + 10;

    let Some(main_id) = model.main_plan_id() else {
        return;
    };
    let Some(plan) = model.plans.get(&main_id) else {
        return;
    };
    let Some(row_names) = plan.row_names.get(&None) else {
        return;
    };

    let offday_color = Color::srgba(0.48, 0.50, 0.56, 0.28);

    for (row_idx, name) in row_names.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        let Some(rb) = model.resource_by_name(name) else {
            continue;
        };
        if rb.non_working_dates.is_empty() {
            continue;
        }
        let row = row_idx as i32;
        let row_y = -(row as f32) * constants::ROW_HEIGHT;
        // Row-augmented set: same one used by block_span_x for this row.
        let row_offs = row_offs_map.get(&row).unwrap_or(&global_offs);

        for nwd in &rb.non_working_dates {
            let date = nwd.date;
            // Skip compressed weekends and dates already covered by a
            // full-height global holiday column.
            if date.weekday().number_from_monday() > cal.working_days_per_week as u32 {
                continue;
            }
            if global_offs.contains(&date) {
                continue;
            }
            let day = calendar::date_to_day(date, cal);
            if day < 0 || day > span {
                continue;
            }
            // Position the band in the row's augmented layout so it falls
            // in the same gap that block_span_x stretches the block over.
            let left_x = blocks::resource_offday_column_left_x(date, row_offs, cal);
            commands.spawn((
                ResourceOffDayBand,
                Sprite {
                    color: offday_color,
                    custom_size: Some(Vec2::new(PIXELS_PER_DAY, constants::ROW_HEIGHT)),
                    ..default()
                },
                Transform::from_xyz(left_x + PIXELS_PER_DAY * 0.5, row_y, -0.4),
            ));
        }
    }
}

/// Marker for quarter and month period-band sprites rendered behind the timeline.
#[derive(Component)]
struct PeriodBand;

/// Subtle quarter tints for the background period bands — an all-cool twilight
/// palette (blue → indigo → violet) where blue is always the dominant channel,
/// so nothing reads as brown or green. Low alpha so quarters register as gentle
/// tonal shifts over the warm-dark canvas instead of loud color blocks.
const QUARTER_TINTS: [[f32; 3]; 4] = [
    [0.40, 0.50, 0.70], // Q1 — blue
    [0.45, 0.46, 0.72], // Q2 — indigo
    [0.54, 0.46, 0.70], // Q3 — violet
    [0.44, 0.50, 0.70], // Q4 — blue-slate
];
/// Base alpha for the quarter tints (odd months within a quarter use 0.7×).
const QUARTER_TINT_ALPHA: f32 = 0.05;
/// Minimum calendar horizon (working days) the background bands fill, so the
/// quarter tints and week markers keep going for ~3 years even when the plan
/// itself is short. ~260 working days per year.
const CALENDAR_HORIZON_DAYS: i32 = 780;

/// Returns (x_center, width, rgba_color) for each month band in the plan span.
fn period_band_spans(config: &model::CalendarConfig, span_days: i32) -> Vec<(f32, f32, [f32; 4])> {
    let mut result = Vec::new();
    let off = config.global_off_days();
    let span_px = calendar::day_to_x(span_days, &off, config);

    let start_year = config.start_date.year();
    let start_month = config.start_date.month();

    let mut year = start_year;
    let mut month = start_month;

    loop {
        let x_start = match calendar::first_working_day_of_month(year, month, config) {
            Some(d) => calendar::day_to_x(calendar::date_to_day(d, config), &off, config).max(0.0),
            None => {
                let (ny, nm) = next_year_month(year, month);
                year = ny;
                month = nm;
                if x_start_of_month(year, month, config) >= span_px {
                    break;
                }
                continue;
            }
        };

        if x_start >= span_px {
            break;
        }

        let (ny, nm) = next_year_month(year, month);
        let x_end = match calendar::first_working_day_of_month(ny, nm, config) {
            Some(d) => {
                calendar::day_to_x(calendar::date_to_day(d, config), &off, config).min(span_px)
            }
            None => span_px,
        };

        let width = x_end - x_start;
        if width > 0.0 {
            let quarter = ((month - 1) / 3) as usize;
            let month_in_quarter = (month - 1) % 3;
            let tint = QUARTER_TINTS[quarter % 4];
            // Subtle within-quarter texture: dim the odd months slightly.
            let alpha = if month_in_quarter % 2 == 1 {
                QUARTER_TINT_ALPHA * 0.7
            } else {
                QUARTER_TINT_ALPHA
            };
            let color = [tint[0], tint[1], tint[2], alpha];
            result.push((x_start + width * 0.5, width, color));
        }

        year = ny;
        month = nm;
    }

    result
}

fn next_year_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

fn x_start_of_month(year: i32, month: u32, config: &model::CalendarConfig) -> f32 {
    match calendar::first_working_day_of_month(year, month, config) {
        Some(d) => calendar::day_to_x(
            calendar::date_to_day(d, config),
            &config.global_off_days(),
            config,
        )
        .max(0.0),
        None => f32::MAX,
    }
}

fn sync_period_bands(
    model: Res<model::Model>,
    schedule: Res<schedule::Schedule>,
    mut commands: Commands,
    band_q: Query<Entity, With<PeriodBand>>,
) {
    if !model.is_changed() && !schedule.is_changed() {
        return;
    }
    for e in &band_q {
        commands.entity(e).despawn();
    }
    let span = schedule.total_duration_days.max(CALENDAR_HORIZON_DAYS) + 30;
    for (cx, w, color) in period_band_spans(&model.calendar, span) {
        commands.spawn((
            PeriodBand,
            Sprite {
                color: Color::srgba(color[0], color[1], color[2], color[3]),
                custom_size: Some(Vec2::new(w, 20_000.0)),
                ..default()
            },
            Transform::from_xyz(cx, 0.0, -1.0),
        ));
    }
}

/// Keeps `Schedule.total_duration_days` in sync with the actual block extents.
/// `forward_pass` is only run on plan switches / auto-schedule, so manually
/// dragged or resized blocks leave `total_duration_days` stale. This system
/// recomputes it from `model.work_blocks` on every frame the model changes so
/// the background band and label systems always span the full timeline.
fn sync_total_duration(model: Res<model::Model>, mut schedule: ResMut<schedule::Schedule>) {
    if !model.is_changed() {
        return;
    }
    let computed = model
        .work_blocks
        .values()
        .filter(|wb| wb.duration_days > 0)
        .map(|wb| wb.start_day + wb.duration_days)
        .max()
        .unwrap_or(0);
    if schedule.total_duration_days != computed {
        schedule.total_duration_days = computed;
    }
}

/// Tracks which timeline day the user is hovering for a "fork plan here" gesture.
/// Cleared when the pointer leaves the timeline or enters a UI panel.
#[derive(Resource, Default)]
struct ForkHoverState {
    hovered_day: Option<model::Day>,
}

fn setup_demo_schedule(mut model: ResMut<model::Model>, mut commands: Commands) {
    use model::DependencyType;
    // Skip seeding if the DB already has plans — prevents duplicate Demo Plan on every restart.
    // But we still need to build and insert the Schedule resource from the loaded data so that
    // all downstream systems (side_panel_ui, draw_create_mode_overlay, spawn_day_labels, etc.)
    // have a valid Schedule on the very first Update tick.
    if !model.plans.is_empty() {
        // Use the lowest-id root plan (forks sort last via branch_start_day.is_some())
        // for the initial forward pass. Picking values().next() could land on a fork.
        let default_plan = model
            .plans
            .values()
            .min_by_key(|p| (p.branch_start_day.is_some(), p.id.0))
            .cloned();
        if let Some(plan) = default_plan {
            let graph = graph::build_graph(&model, &plan);
            if let Ok(sched) = schedule::forward_pass(&model, &graph) {
                commands.insert_resource(sched);
            } else {
                commands.insert_resource(schedule::Schedule::default());
            }
        }
        return;
    }

    let plan_id = model.create_plan("Demo Plan", None);

    let seed_block = |model: &mut model::Model, name: &str, dur: Day| {
        let id = model.create_work_block(name);
        model.work_blocks.get_mut(&id).unwrap().duration_days = dur;
        id
    };

    let design = seed_block(&mut model, "Design", 5);
    let build = seed_block(&mut model, "Build", 8);
    let test = seed_block(&mut model, "Test", 4);
    let review = seed_block(&mut model, "Review", 2);
    let deploy = seed_block(&mut model, "Deploy", 1);

    model.create_dependency(design, build, DependencyType::FinishToStart);
    model.create_dependency(build, test, DependencyType::FinishToStart);
    model.create_dependency(test, review, DependencyType::FinishToStart);
    model.create_dependency(review, deploy, DependencyType::FinishToStart);

    let plan = {
        let p = model.plans.get_mut(&plan_id).unwrap();
        p.root_blocks = vec![design, build, test, review, deploy];
        p.clone()
    };

    let dep_graph = graph::build_graph(&model, &plan);
    if let Ok(sched) = schedule::forward_pass(&model, &dep_graph) {
        for sb in sched.blocks.values() {
            if let Some(wb) = model.work_blocks.get_mut(&sb.work_block_id) {
                wb.start_day = sb.start_day;
                wb.duration_days = sb.duration_days;
            }
        }
        commands.insert_resource(sched);
    }
}

/// Tracks mouse position over the timeline and updates `ForkHoverState`.
/// On left-click, creates a new plan that branches from the hovered day.
#[allow(clippy::too_many_arguments)]
fn handle_fork_hover(
    mut fork: ResMut<ForkHoverState>,
    mut model: ResMut<model::Model>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut egui_ctx: EguiContexts,
    conn: NonSend<rusqlite::Connection>,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    if ctx.is_pointer_over_area() {
        fork.hovered_day = None;
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_gt)) = camera.single() else {
        return;
    };

    let world_x = window
        .cursor_position()
        .and_then(|cursor| cam.viewport_to_world_2d(cam_gt, cursor).ok())
        .map(|wp| wp.x);

    let off = model.calendar.global_off_days();
    fork.hovered_day = world_x.map(|x| calendar::x_to_day(x, &off, &model.calendar));

    // Ctrl+Left-click: fork main into a new branch at the hovered day.
    let ctrl = keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    if ctrl && mouse.just_pressed(MouseButton::Left) {
        if let Some(fork_day) = fork.hovered_day {
            // Fork main into a new branch at the hovered day (clamped ≥ 0). The
            // branch inherits main's blocks from the fork day forward; see
            // Model::fork_main for the semantics, which is unit-tested.
            if model.fork_main(fork_day.max(0)).is_some() {
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }
        }
    }
}

/// Draws branch-point markers and fork-hover indicators using gizmos.
///
/// For each non-active plan with a `branch_start_day`, draws:
///   - A vertical colored line at that day spanning the viewport
///   - A small fork symbol (two short diagonal lines diverging upward)
///
/// When `ForkHoverState` has a hovered day, draws a ghost vertical line
/// showing where a new plan would branch from.
fn draw_branch_markers(
    mut gizmos: Gizmos,
    model: Res<model::Model>,
    selected_plan: Res<SelectedPlan>,
    fork: Res<ForkHoverState>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Ok((cam_t, proj)) = cam_q.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let Ok(window) = windows.single() else { return };
    let half_h = (window.height() * 0.5 * ortho.scale).max(800.0);

    let main_id = model.main_plan_id();

    // Branch-point markers for forked plans.
    let mut branch_plans: Vec<&model::Plan> = model
        .plans
        .values()
        .filter(|p| Some(p.id) != main_id && p.branch_start_day.is_some())
        .collect();
    branch_plans.sort_by_key(|p| p.id.0);

    let off = model.calendar.global_off_days();
    for (idx, plan) in branch_plans.iter().enumerate() {
        let Some(branch_day) = plan.branch_start_day else {
            continue;
        };
        let x = calendar::day_to_x(branch_day, &off, &model.calendar);
        let lc = blocks::BRANCH_PALETTE[idx % blocks::BRANCH_PALETTE.len()];
        // The selected branch is drawn brighter and fully opaque so it's clear
        // which one the Delete key will remove.
        let selected = selected_plan.0 == Some(plan.id);
        let color = if selected {
            Color::from(LinearRgba::new(
                lc.red * 1.4,
                lc.green * 1.4,
                lc.blue * 1.4,
                1.0,
            ))
        } else {
            Color::from(LinearRgba::new(
                lc.red * 0.7,
                lc.green * 0.7,
                lc.blue * 0.7,
                0.55,
            ))
        };

        // Vertical branch line.
        gizmos.line_2d(
            Vec2::new(x, cam_t.translation.y + half_h),
            Vec2::new(x, cam_t.translation.y - half_h),
            color,
        );

        // Fork symbol: two diagonal lines diverging from the branch point.
        let fork_y = cam_t.translation.y + half_h * 0.30;
        let arm = ortho.scale * 18.0;
        gizmos.line_2d(
            Vec2::new(x, fork_y),
            Vec2::new(x - arm, fork_y + arm),
            color,
        );
        gizmos.line_2d(
            Vec2::new(x, fork_y),
            Vec2::new(x + arm, fork_y + arm),
            color,
        );
    }

    // Fork-hover indicator: ghost line at hovered day.
    if let Some(hovered_day) = fork.hovered_day {
        let x = calendar::day_to_x(hovered_day, &off, &model.calendar);
        let ghost = Color::srgba(0.55, 0.75, 1.0, 0.25);
        gizmos.line_2d(
            Vec2::new(x, cam_t.translation.y + half_h),
            Vec2::new(x, cam_t.translation.y - half_h),
            ghost,
        );
        // Small fork arms on the hover indicator.
        let fork_y = cam_t.translation.y + half_h * 0.30;
        let arm = ortho.scale * 14.0;
        gizmos.line_2d(
            Vec2::new(x, fork_y),
            Vec2::new(x - arm, fork_y + arm),
            ghost,
        );
        gizmos.line_2d(
            Vec2::new(x, fork_y),
            Vec2::new(x + arm, fork_y + arm),
            ghost,
        );
    }
}

/// Fixed calendar ruler docked directly under the top bar. Unlike the old
/// world-space day/period labels (which panned and zoomed with the canvas and
/// "slipped" off the top), this is screen-space: it maps each day to a screen X
/// from the camera (`x` + zoom) and the window width, painting day ticks and
/// quarter labels at a constant Y and constant font size. The timeline body
/// scrolls underneath while the calendar header stays put.
fn calendar_ruler_ui(
    mut contexts: EguiContexts,
    model: Res<model::Model>,
    today: Res<schedule::TodayMarker>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Ok((cam_t, proj)) = cam_q.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let Ok(window) = windows.single() else { return };

    let scale = ortho.scale;
    let cam_x = cam_t.translation.x;
    let win_w = window.width();
    let world_to_screen_x = |wx: f32| win_w * 0.5 + (wx - cam_x) / scale;

    // Visible world-x extents, with a one-day margin so labels don't pop at edges.
    let half_w = win_w * 0.5 * scale + PIXELS_PER_DAY;
    let x_left = cam_x - half_w;
    let x_right = cam_x + half_w;

    let config = &model.calendar;
    let off = config.global_off_days();
    let wdpw = (config.working_days_per_week as i32).max(1);
    // Working-day → date helpers and the on-screen size of one day.
    let day_w = PIXELS_PER_DAY / scale; // screen px per day
    let week_w = wdpw as f32 * day_w;
    let show_days = day_w >= 13.0;
    let show_weeks = week_w >= 44.0;

    let day_min = calendar::x_to_day(x_left, &off, config);
    let day_max = calendar::x_to_day(x_right, &off, config) + 1;

    egui::TopBottomPanel::top("calendar_ruler")
        .exact_height(64.0)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            let painter = ui.painter_at(rect);

            let year_y = rect.top() + 9.0;
            let quarter_y = rect.top() + 24.0;
            let week_y = rect.top() + 39.0;
            let day_y = rect.top() + 54.0;

            let year_color = egui::Color32::from_rgb(214, 178, 120);
            let quarter_color = egui::Color32::from_rgb(196, 162, 110);
            let week_color = egui::Color32::from_rgb(150, 150, 170);
            let day_color = egui::Color32::from_rgb(200, 204, 222);
            let past_color = egui::Color32::from_rgb(110, 110, 130);

            // A centered label over a world-space span, clamped to the visible
            // window so it stays readable as the period scrolls (sticky header).
            let period = |x_start_w: f32, x_end_w: f32, text: &str, y: f32, size: f32, color| {
                let sx = world_to_screen_x(x_start_w).max(rect.left() + 3.0);
                let ex = world_to_screen_x(x_end_w).min(rect.right() - 3.0);
                if ex - sx < size * 1.2 {
                    return;
                }
                painter.text(
                    egui::Pos2::new((sx + ex) * 0.5, y),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(size),
                    color,
                );
            };
            let day_x = |d: i32| calendar::day_to_x(d, &off, config);

            let d_lo = calendar::day_to_date(day_min, config);
            let d_hi = calendar::day_to_date(day_max, config);

            // Tier 1: Year — centered over the year's span.
            for y in d_lo.year()..=d_hi.year() {
                let ys = year_start_x(y, config);
                let ye = year_start_x(y + 1, config);
                period(ys, ye, &format!("{y}"), year_y, 13.0, year_color);
            }

            // Tier 2: Quarter — Q1..Q4 over each quarter's span.
            for y in d_lo.year()..=d_hi.year() {
                for q in 0..4 {
                    let qs = quarter_start_x(y, q, config);
                    let qe = quarter_start_x(y, q + 1, config);
                    period(
                        qs,
                        qe,
                        &format!("Q{} '{:02}", q + 1, y % 100),
                        quarter_y,
                        11.0,
                        quarter_color,
                    );
                }
            }

            // Tier 3: Week — label each working-week with its start date.
            if show_weeks {
                let w_lo = day_min.div_euclid(wdpw);
                let w_hi = day_max.div_euclid(wdpw);
                for wi in w_lo..=w_hi {
                    let ws = wi * wdpw;
                    let date = calendar::day_to_date(ws, config);
                    let label = format!("{} {}", date.format("%b"), date.day());
                    period(
                        day_x(ws),
                        day_x(ws + wdpw),
                        &label,
                        week_y,
                        10.5,
                        week_color,
                    );
                    // Week boundary tick.
                    let sx = world_to_screen_x(day_x(ws));
                    painter.line_segment(
                        [
                            egui::Pos2::new(sx, week_y - 5.0),
                            egui::Pos2::new(sx, rect.bottom()),
                        ],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 74, 92)),
                    );
                }
            }

            // Tier 4: Day numbers (1, 2, 3 …) centered in each day cell.
            if show_days {
                let tick = egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 56, 72));
                // Thin the day *numbers* (not the ticks) so they keep a gap and
                // never collide when columns get narrow.
                let label_stride = labels::day_label_stride(day_w);
                for d in day_min..=day_max {
                    let bx = world_to_screen_x(day_x(d));
                    painter.line_segment(
                        [
                            egui::Pos2::new(bx, day_y - 6.0),
                            egui::Pos2::new(bx, rect.bottom()),
                        ],
                        tick,
                    );
                    // Ticks stay dense; draw a number only every `label_stride`
                    // days. Anchoring on `d` keeps drawn numbers evenly spaced.
                    if d.rem_euclid(label_stride) != 0 {
                        continue;
                    }
                    let cx = world_to_screen_x(day_x(d) + PIXELS_PER_DAY * 0.5);
                    let date = calendar::day_to_date(d, config);
                    let color = if d < today.day { past_color } else { day_color };
                    painter.text(
                        egui::Pos2::new(cx, day_y),
                        egui::Align2::CENTER_CENTER,
                        format!("{}", date.day()),
                        egui::FontId::proportional(10.5),
                        color,
                    );
                }
                // Holiday columns carry their own greyed date number, so the
                // date doesn't disappear from the header where work skips it.
                let holiday_num = egui::Color32::from_rgb(120, 122, 134);
                // Multi-day labeled holidays get one label over the whole run
                // (drawn below); suppress the per-column numbers those cover.
                let label_spans = calendar::holiday_label_spans(&off, config, day_max);
                for (left_x, date, desc) in calendar::holiday_columns(&off, config, day_max) {
                    let sx = world_to_screen_x(left_x);
                    let cx = sx + day_w * 0.5;
                    if cx < rect.left() || cx > rect.right() {
                        continue;
                    }
                    let in_label_span = label_spans
                        .iter()
                        .any(|(l, r, _)| left_x >= *l - 0.5 && left_x < *r - 0.5);
                    // Thin holiday numbers consistently with the regular days,
                    // keyed off the holiday's own day index.
                    if !in_label_span
                        && calendar::date_to_day(date, config).rem_euclid(label_stride) == 0
                    {
                        painter.text(
                            egui::Pos2::new(cx, day_y),
                            egui::Align2::CENTER_CENTER,
                            format!("{}", date.day()),
                            egui::FontId::proportional(10.5),
                            holiday_num,
                        );
                    }
                    if !desc.is_empty() {
                        let col_rect = egui::Rect::from_min_max(
                            egui::Pos2::new(sx, rect.top()),
                            egui::Pos2::new(sx + day_w, rect.bottom()),
                        );
                        ui.allocate_rect(col_rect, egui::Sense::hover())
                            .on_hover_text(&desc);
                    }
                }
                // Draw each multi-day holiday's label once, centered over its run
                // in place of the suppressed per-column numbers.
                let holiday_label_color = egui::Color32::from_rgb(170, 152, 176);
                for (l, r, desc) in &label_spans {
                    period(*l, *r, desc, day_y, 10.0, holiday_label_color);
                }
            }

            // Today marker tick — warm accent, matching the canvas today line.
            let today_x = world_to_screen_x(day_x(today.day));
            if today_x >= rect.left() && today_x <= rect.right() {
                painter.line_segment(
                    [
                        egui::Pos2::new(today_x, rect.top()),
                        egui::Pos2::new(today_x, rect.bottom()),
                    ],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(250, 196, 92)),
                );
            }
        });
}

/// Width of the resource-name gutter, in logical pixels.
const GUTTER_WIDTH: f32 = 116.0;

/// Left gutter naming the resource rows of the current view. It carries only a
/// faint background — just enough to be a click target (so clicks don't fall
/// through and create blocks) without reading as a heavy panel. Each row a
/// block sits on gets a label that tracks the row vertically as the camera pans
/// (like the calendar ruler tracks days). Double-click a name to edit it in
/// place; typing an existing row's name merges this row's work onto it.
///
/// Names resolve through the active plan at the current drill scope, with
/// branches inheriting main's names by default (`Model::resolved_row_name`).
#[allow(clippy::too_many_arguments)]
fn resource_gutter_ui(
    mut contexts: EguiContexts,
    mut model: ResMut<model::Model>,
    drill: Res<schedule::DrillScope>,
    visible: Res<schedule::VisibleBlocks>,
    view: Res<ViewMode>,
    person_view: Res<schedule::PersonViewCache>,
    mut rename: ResMut<RowRename>,
    conn: NonSend<rusqlite::Connection>,
    keys: Res<ButtonInput<KeyCode>>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Ok((cam_t, proj)) = cam_q.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let Ok(window) = windows.single() else { return };
    let rh = constants::ROW_HEIGHT;
    let scope = drill.path.last().copied();

    // In By-Person the gutter is read-only: no rename, no resource picker. Clear
    // any pending one (e.g. a rename started in By-Plan, then toggled to
    // By-Person) up front. Both the editable field and the Enter/lost-focus
    // commit key off `rename.editing`, so leaving it set is a read-only escape
    // that could `commit_row_name` on a main lane.
    if view.by_person
        && (rename.editing.is_some() || rename.picker_open.is_some() || rename.drag.is_some())
    {
        rename.editing = None;
        rename.picker_open = None;
        rename.drag = None;
        rename.buf.clear();
    }

    // One labelled row in the gutter. Each plan — main plus every forked band —
    // contributes its own rows at its own world-Y, so the gutter is plan-aware
    // rather than locked to a single "active" plan.
    struct GutterRow {
        plan_id: model::PlanId,
        scope: Option<model::WorkBlockId>,
        row: i32,
        world_y: f32,
        name: Option<String>,
        kind: Option<model::ResourceType>,
    }
    // Resolve a row's display name (with branch→main inheritance) and resource
    // type up front so the egui closure borrows no model state while it may
    // mutate on commit.
    let resolve = |model: &model::Model,
                   plan_id: model::PlanId,
                   scope: Option<model::WorkBlockId>,
                   row: i32,
                   world_y: f32| {
        let name = model.resolved_row_name(plan_id, scope, row);
        let kind = name.and_then(|n| model.resource_kind(n));
        GutterRow {
            plan_id,
            scope,
            row,
            world_y,
            name: name.map(|s| s.to_string()),
            kind,
        }
    };

    // The gutter only labels rows that carry a visible block, so it stays empty
    // until there's real work on a row.
    let mut entries: Vec<GutterRow> = Vec::new();

    if view.by_person {
        // By-Person: one read-only label per individual contributor row.
        let dummy_plan = model.main_plan_id().unwrap_or(model::PlanId(0));
        for (i, (name, kind)) in person_view.0.rows.iter().enumerate() {
            entries.push(GutterRow {
                plan_id: dummy_plan,
                scope: None,
                row: i as i32,
                world_y: -(i as f32 * rh),
                name: Some(name.clone()),
                kind: Some(*kind),
            });
        }
    } else {
        // Main plan occupies world rows 0,1,2… at y = -row * ROW_HEIGHT, respecting
        // the active drill scope.
        if let Some(main_id) = model.main_plan_id() {
            let mut rows: Vec<i32> = visible
                .ids
                .iter()
                .map(|id| model.block_row(main_id, *id))
                .collect();
            rows.sort_unstable();
            rows.dedup();
            for r in rows {
                entries.push(resolve(&model, main_id, scope, r, -(r as f32 * rh)));
            }
        }

        // Each forked-plan band labels its own rows, anchored at that lane's row0_y.
        // Bands are hidden while drilled into a block, so skip them then.
        if drill.path.is_empty() {
            for band in bands::layout_bands(&model) {
                let mut rows: Vec<i32> = schedule::visible_blocks(&model, band.plan_id, None)
                    .iter()
                    .map(|wb| model.block_row(band.plan_id, wb.id))
                    .collect();
                rows.sort_unstable();
                rows.dedup();
                for r in rows {
                    entries.push(resolve(
                        &model,
                        band.plan_id,
                        None,
                        r,
                        band.row0_y - r as f32 * rh,
                    ));
                }
            }
        }

        // A brand-new resource lane is edited before it has a block. Give the edited
        // row an entry (synthesizing its world-Y from the row index in its plan) so
        // the rename field below actually renders on that row.
        if let Some((pid, sc, r)) = rename.editing {
            if !entries
                .iter()
                .any(|e| e.plan_id == pid && e.scope == sc && e.row == r)
            {
                let world_y = if Some(pid) == model.main_plan_id() {
                    -(r as f32 * rh)
                } else {
                    bands::layout_bands(&model)
                        .into_iter()
                        .find(|b| b.plan_id == pid)
                        .map(|b| b.row0_y - r as f32 * rh)
                        .unwrap_or(-(r as f32 * rh))
                };
                entries.push(resolve(&model, pid, sc, r, world_y));
            }
        }
    }

    if entries.is_empty() && rename.editing.is_none() {
        return;
    }

    let scale = ortho.scale;
    let cam_y = cam_t.translation.y;
    let win_h = window.height();
    let world_to_screen_y = |wy: f32| win_h * 0.5 + (cam_y - wy) / scale;
    let editing = rename.editing;
    let picker_open = rename.picker_open;
    let dragging = rename.drag;

    let known_resources = model.named_resources();
    let resource_kinds: Vec<Option<model::ResourceType>> = known_resources
        .iter()
        .map(|n| model.resource_kind(n))
        .collect();

    let Ok(ctx) = contexts.ctx_mut() else { return };

    enum Act {
        OpenPicker(model::PlanId, Option<model::WorkBlockId>, i32),
        ClosePicker,
        SelectResource(model::PlanId, Option<model::WorkBlockId>, i32, String),
        StartNew(model::PlanId, Option<model::WorkBlockId>, i32),
        CommitNew,
        CancelNew,
        StartDrag(model::PlanId, Option<model::WorkBlockId>, i32),
        DropRow(model::PlanId, Option<model::WorkBlockId>, i32, i32),
        CancelDrag,
    }
    let mut act: Option<Act> = if keys.just_pressed(KeyCode::Escape) {
        if dragging.is_some() {
            Some(Act::CancelDrag)
        } else if editing.is_some() {
            Some(Act::CancelNew)
        } else if picker_open.is_some() {
            Some(Act::ClosePicker)
        } else {
            None
        }
    } else if editing.is_some()
        && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
    {
        Some(Act::CommitNew)
    } else {
        None
    };

    egui::SidePanel::left("resource_gutter")
        .exact_width(GUTTER_WIDTH)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(14, 20, 24, 96))
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            let half = (rh / scale * 0.5).clamp(9.0, 22.0);

            for e in &entries {
                let cy = world_to_screen_y(e.world_y);
                if cy < rect.top() - half || cy > rect.bottom() + half {
                    continue;
                }

                let key = (e.plan_id, e.scope, e.row);
                let name = &e.name;
                let kind = &e.kind;
                let editing_this = editing == Some(key);
                let picker_this = picker_open == Some(key);

                if editing_this {
                    let field = egui::Rect::from_min_max(
                        egui::pos2(rect.left() + 6.0, cy - 9.0),
                        egui::pos2(rect.right() - 4.0, cy + 9.0),
                    );
                    let resp = ui
                        .scope(|ui| {
                            theme::style_inputs(ui);
                            ui.put(
                                field,
                                egui::TextEdit::singleline(&mut rename.buf)
                                    .id(egui::Id::new(("gutter_rename", e.plan_id.0, e.row)))
                                    .font(egui::FontId::proportional(13.0))
                                    .text_color(theme::TEXT),
                            )
                        })
                        .inner;
                    if !resp.has_focus() {
                        resp.request_focus();
                    }
                    if resp.lost_focus() && act.is_none() {
                        act = Some(Act::CommitNew);
                    }
                } else if view.by_person {
                    // By-person: read-only label + dot, no click interaction.
                    let (text, color) = match name {
                        Some(n) => (n.clone(), egui::Color32::from_rgb(206, 190, 164)),
                        None => (
                            default_row_label(e.row),
                            egui::Color32::from_rgb(138, 128, 114),
                        ),
                    };
                    let mut text_x = rect.left() + 10.0;
                    if let Some(k) = kind {
                        draw_resource_dot(ui.painter(), egui::pos2(rect.left() + 8.0, cy), *k);
                        text_x = rect.left() + 18.0;
                    }
                    ui.painter().text(
                        egui::pos2(text_x, cy),
                        egui::Align2::LEFT_CENTER,
                        &text,
                        egui::FontId::proportional(13.0),
                        color,
                    );
                } else if e.row < 0 {
                    // The Events row: a fixed label, not a resource — no rename,
                    // no resource picker, no drag-reorder.
                    ui.painter().text(
                        egui::pos2(rect.left() + 10.0, cy),
                        egui::Align2::LEFT_CENTER,
                        default_row_label(e.row),
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_rgb(206, 190, 164),
                    );
                } else {
                    let hot = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), cy - half),
                        egui::pos2(rect.right(), cy + half),
                    );
                    let resp = ui.interact(
                        hot,
                        ui.id().with(("gutter_row", e.plan_id.0, e.row)),
                        egui::Sense::click_and_drag(),
                    );
                    if resp.drag_started() && act.is_none() && dragging.is_none() {
                        act = Some(Act::StartDrag(e.plan_id, e.scope, e.row));
                    }
                    if resp.hovered() && dragging.is_none() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    let (text, color) = match name {
                        Some(n) => (n.clone(), egui::Color32::from_rgb(206, 190, 164)),
                        None => (
                            default_row_label(e.row),
                            egui::Color32::from_rgb(138, 128, 114),
                        ),
                    };
                    let mut text_x = rect.left() + 10.0;
                    if let Some(k) = kind {
                        draw_resource_dot(ui.painter(), egui::pos2(rect.left() + 8.0, cy), *k);
                        text_x = rect.left() + 18.0;
                    }
                    let hovered = resp.hovered();
                    ui.painter().text(
                        egui::pos2(text_x, cy),
                        egui::Align2::LEFT_CENTER,
                        &text,
                        egui::FontId::proportional(13.0),
                        if hovered {
                            egui::Color32::from_rgb(236, 224, 204)
                        } else {
                            color
                        },
                    );
                    if resp.clicked() && act.is_none() {
                        act = Some(Act::OpenPicker(e.plan_id, e.scope, e.row));
                    }

                    if picker_this {
                        let popup_id = ui.id().with(("gutter_picker", e.plan_id.0, e.row));
                        let popup_pos = egui::pos2(rect.right() + 2.0, cy - 4.0);
                        let area_resp = egui::Area::new(popup_id)
                            .fixed_pos(popup_pos)
                            .order(egui::Order::Foreground)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::new()
                                    .fill(theme::PANEL_HI)
                                    .stroke(egui::Stroke::new(1.0, theme::STROKE))
                                    .corner_radius(egui::CornerRadius::same(4))
                                    .inner_margin(egui::Margin::same(6))
                                    .show(ui, |ui| {
                                        ui.set_min_width(130.0);
                                        for (i, res_name) in known_resources.iter().enumerate() {
                                            let is_current = name
                                                .as_ref()
                                                .is_some_and(|n| n.eq_ignore_ascii_case(res_name));
                                            ui.horizontal(|ui| {
                                                if let Some(k) = resource_kinds[i] {
                                                    let (_, dot_rect) =
                                                        ui.allocate_space(egui::vec2(10.0, 16.0));
                                                    draw_resource_dot(
                                                        ui.painter(),
                                                        dot_rect.center(),
                                                        k,
                                                    );
                                                } else {
                                                    ui.allocate_space(egui::vec2(10.0, 16.0));
                                                }
                                                let label_color = if is_current {
                                                    egui::Color32::from_rgb(255, 220, 160)
                                                } else {
                                                    egui::Color32::from_rgb(206, 190, 164)
                                                };
                                                let btn = ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(res_name)
                                                            .color(label_color)
                                                            .size(13.0),
                                                    )
                                                    .selectable(false)
                                                    .sense(egui::Sense::click()),
                                                );
                                                if btn.clicked() {
                                                    act = Some(Act::SelectResource(
                                                        e.plan_id,
                                                        e.scope,
                                                        e.row,
                                                        res_name.clone(),
                                                    ));
                                                }
                                            });
                                        }
                                        ui.add_space(4.0);
                                        ui.separator();
                                        ui.add_space(2.0);
                                        let add_btn = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new("+ Add New")
                                                    .color(egui::Color32::from_rgb(140, 180, 220))
                                                    .size(13.0),
                                            )
                                            .selectable(false)
                                            .sense(egui::Sense::click()),
                                        );
                                        if add_btn.clicked() {
                                            act = Some(Act::StartNew(e.plan_id, e.scope, e.row));
                                        }
                                        if name.is_some() {
                                            ui.add_space(2.0);
                                            let clear_btn = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new("Clear")
                                                        .color(egui::Color32::from_rgb(
                                                            180, 120, 100,
                                                        ))
                                                        .size(12.0),
                                                )
                                                .selectable(false)
                                                .sense(egui::Sense::click()),
                                            );
                                            if clear_btn.clicked() {
                                                act = Some(Act::SelectResource(
                                                    e.plan_id,
                                                    e.scope,
                                                    e.row,
                                                    String::new(),
                                                ));
                                            }
                                        }
                                    });
                            });
                        if ui.ctx().input(|i| i.pointer.any_pressed())
                            && !area_resp.response.rect.contains(
                                ui.ctx()
                                    .input(|i| i.pointer.interact_pos().unwrap_or_default()),
                            )
                            && act.is_none()
                        {
                            act = Some(Act::ClosePicker);
                        }
                    }
                }
            }

            // An active drag-reorder: track the pointer with a ghost of the
            // dragged name, mark the candidate slot, and drop on release. The
            // target comes from the pointer's world-Y so the drop works even
            // when the source row has scrolled out of the culled label range.
            if let Some((pid, sc, from)) = dragging {
                let released = !ui.ctx().input(|i| i.pointer.primary_down());
                let ptr = ui.ctx().input(|i| i.pointer.latest_pos());
                let lane_rows = entries
                    .iter()
                    .filter(|e| e.plan_id == pid && e.scope == sc)
                    .collect::<Vec<_>>();
                let lane = lane_rows.first().map(|e| {
                    let row0_y = e.world_y + e.row as f32 * rh;
                    // Floor at 0: the Events row (−1) is never a reorder target,
                    // and clamp(0, max) needs max ≥ 0 even in an Events-only lane.
                    let max_row = lane_rows.iter().map(|e| e.row).max().unwrap_or(0).max(0);
                    (row0_y, max_row)
                });
                if let (Some((row0_y, max_row)), Some(p)) = (lane, ptr) {
                    let world_y = cam_y - (p.y - win_h * 0.5) * scale;
                    let target = (((row0_y - world_y) / rh).round() as i32).clamp(0, max_row);
                    if released {
                        if act.is_none() {
                            act = Some(Act::DropRow(pid, sc, from, target));
                        }
                    } else {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        let ty = world_to_screen_y(row0_y - target as f32 * rh);
                        ui.painter().line_segment(
                            [
                                egui::pos2(rect.left() + 2.0, ty),
                                egui::pos2(rect.right() - 2.0, ty),
                            ],
                            egui::Stroke::new(1.5, theme::ACCENT),
                        );
                        let name = lane_rows
                            .iter()
                            .find(|e| e.row == from)
                            .and_then(|e| e.name.clone())
                            .unwrap_or_else(|| default_row_label(from));
                        ui.painter().text(
                            egui::pos2(rect.left() + 10.0, p.y),
                            egui::Align2::LEFT_CENTER,
                            name,
                            egui::FontId::proportional(13.0),
                            egui::Color32::from_rgba_unmultiplied(236, 224, 204, 200),
                        );
                    }
                } else if released && act.is_none() {
                    act = Some(Act::CancelDrag);
                }
            }
        });

    match act {
        Some(Act::OpenPicker(pid, sc, r)) => {
            rename.picker_open = Some((pid, sc, r));
            rename.editing = None;
            rename.buf.clear();
        }
        Some(Act::ClosePicker) => {
            rename.picker_open = None;
        }
        Some(Act::SelectResource(pid, sc, r, name)) => {
            commit_row_name(&mut model, &conn, pid, sc, r, &name);
            rename.picker_open = None;
        }
        Some(Act::StartNew(pid, sc, r)) => {
            rename.picker_open = None;
            rename.editing = Some((pid, sc, r));
            rename.buf.clear();
        }
        Some(Act::CommitNew) => {
            if let Some((pid, sc, r)) = rename.editing {
                let raw = rename.buf.trim().to_string();
                commit_row_name(&mut model, &conn, pid, sc, r, &raw);
            }
            rename.editing = None;
            rename.buf.clear();
        }
        Some(Act::CancelNew) => {
            rename.editing = None;
            rename.buf.clear();
        }
        Some(Act::StartDrag(pid, sc, r)) => {
            rename.drag = Some((pid, sc, r));
            rename.picker_open = None;
        }
        Some(Act::DropRow(pid, sc, from, to)) => {
            if from != to {
                apply_row_reorder(&mut model, &conn, pid, sc, from, to);
            }
            rename.drag = None;
        }
        Some(Act::CancelDrag) => {
            rename.drag = None;
        }
        None => {}
    }
}

/// Applies a resource-row rename. Empty clears the name. If `raw` matches
/// another row's name in the same scope (case-insensitively), the two are the
/// same resource: this row's blocks move onto that row and no separate name is
/// kept. Otherwise the name is stored as this plan's override for the row.
fn commit_row_name(
    model: &mut model::Model,
    conn: &rusqlite::Connection,
    plan_id: model::PlanId,
    scope: Option<model::WorkBlockId>,
    row: i32,
    raw: &str,
) {
    let name = raw.trim().to_string();

    // A matching name on another row (resolved through inheritance) means the
    // user is pointing this row at an existing resource.
    let merge_target = if name.is_empty() {
        None
    } else {
        // Bound the search by the actual number of allocated rows so a named
        // resource on a high-numbered row is never silently missed.
        let named_row_count = model
            .plans
            .get(&plan_id)
            .and_then(|p| p.row_names.get(&scope))
            .map(|v| v.len() as i32)
            .unwrap_or(0);
        (0..named_row_count).find(|&other| {
            other != row
                && model
                    .resolved_row_name(plan_id, scope, other)
                    .is_some_and(|n| n.eq_ignore_ascii_case(&name))
        })
    };

    if let Some(target) = merge_target {
        let move_ids: Vec<model::WorkBlockId> = schedule::visible_blocks(model, plan_id, scope)
            .iter()
            .filter(|wb| model.block_row(plan_id, wb.id) == row)
            .map(|wb| wb.id)
            .collect();
        for id in move_ids {
            model.set_block_row(plan_id, id, target);
        }
        if let Some(plan) = model.plans.get_mut(&plan_id) {
            plan.set_row_name(scope, row, String::new());
        }
    } else if let Some(plan) = model.plans.get_mut(&plan_id) {
        plan.set_row_name(scope, row, name);
    }

    if let Err(e) = db::save_model(conn, model) {
        error!("save_model failed: {e}");
    }
}

/// New index for `row` after a drag-reorder that moves row `from` to `to`
/// (list-insert semantics: the rows between them shift one step toward
/// `from`; rows outside the span are untouched).
fn reordered_row(row: i32, from: i32, to: i32) -> i32 {
    if row == from {
        to
    } else if from < to && row > from && row <= to {
        row - 1
    } else if to < from && row >= to && row < from {
        row + 1
    } else {
        row
    }
}

/// Permutes a scope's row-name list for a `from`→`to` drag, growing it with
/// empty placeholders so both indices are addressable and trimming trailing
/// empties afterwards (mirroring `reordered_row`'s mapping).
fn reorder_row_names(names: &mut Vec<String>, from: i32, to: i32) {
    let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
        return;
    };
    if from == to {
        return;
    }
    let need = from.max(to) + 1;
    if names.len() < need {
        names.resize(need, String::new());
    }
    let moved = names.remove(from);
    names.insert(to, moved);
    while names.last().is_some_and(|s| s.is_empty()) {
        names.pop();
    }
}

/// Applies a gutter drag-reorder within `(plan, scope)`: row `from` moves to
/// `to` and the rows between shift one step. The blocks visible at that scope
/// follow their rows, the row-name list is permuted to match, and the result
/// autosaves. Per-plan by construction — a branch reorder never touches main.
fn apply_row_reorder(
    model: &mut model::Model,
    conn: &rusqlite::Connection,
    plan_id: model::PlanId,
    scope: Option<model::WorkBlockId>,
    from: i32,
    to: i32,
) {
    if from == to {
        return;
    }
    let moves: Vec<(model::WorkBlockId, i32)> = schedule::visible_blocks(model, plan_id, scope)
        .iter()
        .map(|wb| (wb.id, model.block_row(plan_id, wb.id)))
        .collect();
    for (id, row) in moves {
        let new_row = reordered_row(row, from, to);
        if new_row != row {
            model.set_block_row(plan_id, id, new_row);
        }
    }
    if let Some(names) = model
        .plans
        .get_mut(&plan_id)
        .and_then(|p| p.row_names.get_mut(&scope))
    {
        reorder_row_names(names, from, to);
    }
    if let Err(e) = db::save_model(conn, model) {
        error!("save_model failed: {e}");
    }
}

/// The default label for resource row `row` (0-based) when the user hasn't
/// named it. The fixed Events row above the resources has its own name.
fn default_row_label(row: i32) -> String {
    if row == constants::EVENTS_ROW {
        "Events".to_string()
    } else {
        format!("Resource {}", row + 1)
    }
}

/// The accent colour marking a resource's type in the gutter and settings.
fn resource_type_rgb(kind: model::ResourceType) -> (u8, u8, u8) {
    match kind {
        model::ResourceType::Engineer => (98, 154, 224), // blue
        model::ResourceType::NewHire => (140, 200, 230), // light cyan
        model::ResourceType::Team => (120, 196, 140),    // green
        model::ResourceType::Equipment => (224, 176, 92), // amber
        model::ResourceType::Budget => (180, 150, 222),  // violet
    }
}

/// Paint the small resource-type indicator dot at `pos`.
fn draw_resource_dot(painter: &egui::Painter, pos: egui::Pos2, kind: model::ResourceType) {
    let (r, g, b) = resource_type_rgb(kind);
    painter.circle_filled(pos, 3.5, egui::Color32::from_rgb(r, g, b));
}

/// World-space x of the start of calendar year `y` (its Jan 1, mapped to a
/// working day). Used for the year tier of the calendar ruler.
fn year_start_x(y: i32, config: &model::CalendarConfig) -> f32 {
    let date = chrono::NaiveDate::from_ymd_opt(y, 1, 1).unwrap_or(config.start_date);
    calendar::day_to_x(
        calendar::date_to_day(date, config),
        &config.global_off_days(),
        config,
    )
}

/// World-space x of the start of quarter `q` (0..=4, where 4 = next year's Q1)
/// in calendar year `y`.
fn quarter_start_x(y: i32, q: i32, config: &model::CalendarConfig) -> f32 {
    let (yy, month) = if q >= 4 {
        (y + 1, 1)
    } else {
        (y, (q * 3 + 1) as u32)
    };
    let date = chrono::NaiveDate::from_ymd_opt(yy, month, 1).unwrap_or(config.start_date);
    calendar::day_to_x(
        calendar::date_to_day(date, config),
        &config.global_off_days(),
        config,
    )
}

// ── Settings fly-out layout constants ────────────────────────────────────────
const SETTINGS_SECTION_GAP: f32 = 16.0; // between sections
const SETTINGS_ROW_GAP: f32 = 6.0; // between rows within a section
const SETTINGS_LABEL_COL: f32 = 116.0; // fixed left label column width

/// One config row: fixed-width left label + right-aligned control.
fn settings_row(ui: &mut egui::Ui, label: &str, add_control: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [SETTINGS_LABEL_COL, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(label).color(theme::TEXT_MUTED)),
        );
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            add_control,
        );
    });
}

/// A run of consecutive calendar days that share a label — one logical holiday.
struct HolidayGroup {
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
    description: String,
    dates: Vec<chrono::NaiveDate>,
}

/// Groups non-working dates into runs of consecutive calendar days that share a
/// description, so a multi-day holiday stored as N daily rows shows and removes
/// as one entry. Result is ordered by start date.
fn group_holidays(dates: &[model::NonWorkingDate]) -> Vec<HolidayGroup> {
    let mut sorted = dates.to_vec();
    sorted.sort_by_key(|nwd| nwd.date);
    let mut groups: Vec<HolidayGroup> = Vec::new();
    for nwd in sorted {
        if let Some(g) = groups.last_mut() {
            if g.description == nwd.description && g.end.succ_opt() == Some(nwd.date) {
                g.end = nwd.date;
                g.dates.push(nwd.date);
                continue;
            }
        }
        groups.push(HolidayGroup {
            start: nwd.date,
            end: nwd.date,
            description: nwd.description.clone(),
            dates: vec![nwd.date],
        });
    }
    groups
}

/// All calendar days in `[start, end]` inclusive, with `end` clamped to `>= start`
/// and to at most a year past `start` (so a mis-picked span can't insert thousands
/// of daily rows). This is the expansion behind a multi-day holiday / time-off span.
fn expand_date_range(start: chrono::NaiveDate, end: chrono::NaiveDate) -> Vec<chrono::NaiveDate> {
    let end = end.max(start).min(start + chrono::Duration::days(366));
    let mut out = Vec::new();
    let mut d = start;
    loop {
        out.push(d);
        if d >= end {
            break;
        }
        match d.succ_opt() {
            Some(n) => d = n,
            None => break,
        }
    }
    out
}

/// Re-anchor a date group: drop its current `old_dates` from `dates` and insert
/// the expanded `[start, end]` span, all carrying `desc`, de-duplicated against
/// the survivors. Shared by the holiday and per-resource time-off editors.
fn set_date_range(
    dates: &mut Vec<model::NonWorkingDate>,
    old_dates: &[chrono::NaiveDate],
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
    desc: &str,
) {
    dates.retain(|x| !old_dates.contains(&x.date));
    for d in expand_date_range(start, end) {
        if !dates.iter().any(|x| x.date == d) {
            dates.push(model::NonWorkingDate {
                date: d,
                description: desc.to_string(),
            });
        }
    }
}

/// Apply a committed date-picker result to `dates`, re-anchoring the group that
/// currently starts at `old_start`. A `Single` collapses to a one-day span; a
/// `Range` expands to the inclusive span. Returns whether anything was applied.
fn apply_date_pick(
    dates: &mut Vec<model::NonWorkingDate>,
    old_start: chrono::NaiveDate,
    result: &datepicker::DatePickerResult,
) -> bool {
    let (old_dates, desc) = {
        let groups = group_holidays(dates);
        match groups.iter().find(|g| g.start == old_start) {
            Some(g) => (g.dates.clone(), g.description.clone()),
            None => return false,
        }
    };
    let (start, end) = match *result {
        datepicker::DatePickerResult::Single(d) => (d, d),
        datepicker::DatePickerResult::Range(s, e) => (s, e),
    };
    set_date_range(dates, &old_dates, start, end, &desc);
    true
}

/// Display order for the settings SIZES list: indices into `sizes` sorted by
/// `days` ascending (stable, so ties keep insertion order). While `frozen` and
/// the cached order still covers the current list, the cache is returned
/// unchanged so rows don't re-sort (and steal focus) mid-edit; any add or
/// remove invalidates the cache and falls back to a fresh sort.
fn sizes_display_order(sizes: &[model::TShirtSize], cached: &[usize], frozen: bool) -> Vec<usize> {
    if frozen && cached.len() == sizes.len() {
        return cached.to_vec();
    }
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| sizes[i].days);
    order
}

/// Which date group an open Aurora picker is editing — a calendar holiday or a
/// specific resource's time-off, keyed by the group's (current) start date.
#[derive(Clone, PartialEq)]
enum DateTarget {
    Holiday(chrono::NaiveDate),
    Resource(String, chrono::NaiveDate),
}

/// An open settings date-range picker popup: what it edits, its calendar state,
/// and the screen anchor to draw it at (kept fresh from the row each frame).
struct OpenDatePicker {
    target: DateTarget,
    state: datepicker::DatePickerState,
    anchor: egui::Pos2,
    /// `false` on the frame it opened (so the chip-click that spawned it isn't
    /// counted as a click-outside dismiss); becomes `true` after one frame.
    armed: bool,
}

/// Right-side settings fly-out. Toggled by the top-bar gear. Holds general
/// settings; the first section is the calendar (working days per week, the
/// holiday list, and the start date). Edits write straight to `model.calendar`
/// and autosave.
fn settings_flyout_ui(
    mut contexts: EguiContexts,
    mut settings: ResMut<SettingsState>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
) {
    if !settings.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut changed = false;
    let mut close = false;

    egui::SidePanel::right("settings_flyout")
        .resizable(false)
        .exact_width(272.0)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::STROKE))
                .inner_margin(egui::Margin::same(14)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Settings")
                        .size(16.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(egui::RichText::new("✕").size(14.0)).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(10.0);

            // auto_shrink([false,false]) pins body to the full panel width so
            // desired_width fields and RTL buttons stay on-screen.
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Constrain content to leave room for the scrollbar gutter and
                    // right breathing room so the bar never overlays controls.
                    let gutter =
                        ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
                    ui.set_max_width(ui.available_width() - gutter - 6.0);

                    // ── CALENDAR ─────────────────────────────────────────────
                    theme::section_header(ui, "CALENDAR", None);

                    settings_row(ui, "Days / week", |ui| {
                        let mut wdpw = model.calendar.working_days_per_week as i32;
                        if ui
                            .add(egui::DragValue::new(&mut wdpw).range(1..=7).speed(0.05))
                            .changed()
                        {
                            model.calendar.working_days_per_week = wdpw.clamp(1, 7) as u8;
                            changed = true;
                        }
                        theme::chip(ui, &format!("{} / 7", model.calendar.working_days_per_week));
                    });

                    ui.add_space(SETTINGS_ROW_GAP);
                    // Start date: click the chip to edit in place; no separate TextEdit + SET.
                    settings_row(ui, "Start date", |ui| {
                        if matches!(&settings.editing, Some(SettingsEdit::StartDate)) {
                            let r = ui
                                .scope(|ui| {
                                    theme::style_inputs(ui);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut settings.edit_buf)
                                            .hint_text("YYYY-MM-DD")
                                            .desired_width(100.0),
                                    )
                                })
                                .inner;
                            r.request_focus();
                            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if esc {
                                settings.editing = None;
                            } else if r.lost_focus()
                                || ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                if let Ok(d) = chrono::NaiveDate::parse_from_str(
                                    settings.edit_buf.trim(),
                                    "%Y-%m-%d",
                                ) {
                                    model.calendar.start_date = d;
                                    changed = true;
                                }
                                settings.editing = None;
                            }
                        } else {
                            let resp = theme::date_chip(ui, model.calendar.start_date);
                            if resp.clicked() {
                                settings.editing = Some(SettingsEdit::StartDate);
                                settings.edit_buf =
                                    model.calendar.start_date.format("%Y-%m-%d").to_string();
                            }
                        }
                    });

                    // ── HOLIDAYS ──────────────────────────────────────────────
                    // Header inlined to accommodate the + add button on the right.
                    ui.add_space(SETTINGS_SECTION_GAP);
                    let holiday_groups = group_holidays(&model.calendar.non_working_dates);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("HOLIDAYS")
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::add_button(ui).on_hover_text("Add holiday").clicked() {
                                // Find a date not already in the list as a unique key.
                                let mut new_date = model.calendar.start_date;
                                for _ in 0..1000 {
                                    if !model
                                        .calendar
                                        .non_working_dates
                                        .iter()
                                        .any(|x| x.date == new_date)
                                    {
                                        break;
                                    }
                                    new_date = new_date.succ_opt().unwrap_or(new_date);
                                }
                                model
                                    .calendar
                                    .non_working_dates
                                    .push(model::NonWorkingDate {
                                        date: new_date,
                                        description: String::new(),
                                    });
                                changed = true;
                                settings.editing = Some(SettingsEdit::HolidayLabel(new_date));
                                settings.edit_buf = String::new();
                            }
                            if !holiday_groups.is_empty() {
                                ui.label(
                                    egui::RichText::new(holiday_groups.len().to_string())
                                        .size(10.0)
                                        .color(theme::ACCENT),
                                );
                            }
                        });
                    });
                    // Section rule (same as theme::section_header draws).
                    {
                        let avail = ui.available_width();
                        if avail > 0.0 {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(avail, 1.0), egui::Sense::hover());
                            ui.painter().hline(
                                rect.x_range(),
                                rect.center().y,
                                egui::Stroke::new(1.0, theme::STROKE),
                            );
                        }
                    }
                    ui.add_space(4.0);

                    if holiday_groups.is_empty() {
                        ui.label(
                            egui::RichText::new("None set")
                                .italics()
                                .color(theme::TEXT_MUTED),
                        );
                    }

                    let mut remove_hol: Option<Vec<chrono::NaiveDate>> = None;
                    for g in &holiday_groups {
                        let ed_label = matches!(&settings.editing,
                            Some(SettingsEdit::HolidayLabel(d)) if *d == g.start);

                        let mut row_rm_rect = egui::Rect::NOTHING;
                        let mut row_rm_clicked = false;
                        let mut row_rm_hovered = false;

                        let row = theme::list_row(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Date span: a clickable chip for the start (plus
                                // an end chip for a multi-day span). Clicking either
                                // opens the Aurora calendar range picker popup.
                                let cr = theme::date_chip(ui, g.start);
                                let anchor = cr.rect.left_bottom();
                                let mut open_picker = cr.clicked();
                                if g.start != g.end {
                                    ui.label(egui::RichText::new("–").color(theme::TEXT_MUTED));
                                    open_picker |= theme::date_chip(ui, g.end).clicked();
                                }
                                if open_picker {
                                    settings.editing = None;
                                    settings.picker = Some(OpenDatePicker {
                                        target: DateTarget::Holiday(g.start),
                                        state: datepicker::DatePickerState::range(g.start, None),
                                        anchor,
                                        armed: false,
                                    });
                                }
                                // Keep an already-open picker pinned under this row.
                                if let Some(p) = &mut settings.picker {
                                    if p.target == DateTarget::Holiday(g.start) {
                                        p.anchor = anchor;
                                    }
                                }

                                ui.add_space(4.0);

                                // Label: clickable text or in-place TextEdit.
                                // Reserve remove-slot width so the label doesn't overlap it.
                                let remove_w = 20.0;
                                let label_avail =
                                    (ui.available_width() - remove_w - ui.spacing().item_spacing.x)
                                        .max(20.0);
                                if ed_label {
                                    let r = ui
                                        .scope(|ui| {
                                            theme::style_inputs(ui);
                                            ui.add(
                                                egui::TextEdit::singleline(&mut settings.edit_buf)
                                                    .desired_width(label_avail)
                                                    .hint_text("Label"),
                                            )
                                        })
                                        .inner;
                                    r.request_focus();
                                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    if esc {
                                        settings.editing = None;
                                    } else if r.lost_focus()
                                        || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    {
                                        let new_desc = settings.edit_buf.trim().to_string();
                                        let grp_dates = g.dates.clone();
                                        for nwd in &mut model.calendar.non_working_dates {
                                            if grp_dates.contains(&nwd.date) {
                                                nwd.description = new_desc.clone();
                                                changed = true;
                                            }
                                        }
                                        settings.editing = None;
                                    }
                                } else {
                                    let desc = if g.description.is_empty() {
                                        "—"
                                    } else {
                                        g.description.as_str()
                                    };
                                    let lr = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(desc).color(theme::TEXT_MUTED),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    if lr.clicked() {
                                        settings.editing =
                                            Some(SettingsEdit::HolidayLabel(g.start));
                                        settings.edit_buf = g.description.clone();
                                    }
                                }

                                // Always allocate the × slot to prevent layout shift on hover.
                                let (rr, rresp) = ui
                                    .with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.allocate_exact_size(
                                                egui::vec2(remove_w, 14.0),
                                                egui::Sense::click(),
                                            )
                                        },
                                    )
                                    .inner;
                                row_rm_rect = rr;
                                row_rm_clicked = rresp.clicked();
                                row_rm_hovered = rresp.hovered();
                            });
                        });

                        // Paint × only when row hovered; click detection is always registered.
                        let ptr = ui.ctx().input(|i| i.pointer.hover_pos());
                        if ptr.is_some_and(|p| row.response.rect.contains(p)) {
                            ui.painter().text(
                                row_rm_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "×",
                                egui::FontId::proportional(14.0),
                                if row_rm_hovered {
                                    theme::DANGER
                                } else {
                                    theme::TEXT_MUTED
                                },
                            );
                            if row_rm_clicked {
                                remove_hol = Some(g.dates.clone());
                            }
                        }
                        ui.add_space(2.0);
                    }
                    if let Some(dts) = remove_hol {
                        model
                            .calendar
                            .non_working_dates
                            .retain(|x| !dts.contains(&x.date));
                        changed = true;
                        let stale = matches!(&settings.editing,
                            Some(SettingsEdit::HolidayLabel(d)) if dts.contains(d));
                        if stale {
                            settings.editing = None;
                        }
                        // Drop an open picker that targeted a removed group.
                        if matches!(&settings.picker,
                            Some(p) if matches!(&p.target, DateTarget::Holiday(d) if dts.contains(d)))
                        {
                            settings.picker = None;
                        }
                    }

                    // ── RESOURCES ─────────────────────────────────────────────
                    ui.add_space(SETTINGS_SECTION_GAP);
                    let resource_count = model.named_resources().len();
                    theme::section_header(ui, "RESOURCES", Some(resource_count));

                    let names = model.named_resources();
                    if names.is_empty() {
                        ui.label(
                            egui::RichText::new("Name rows in the gutter to add resources")
                                .italics()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                    for name in &names {
                        ui.horizontal(|ui| {
                            let kind = model.resource_kind(name);
                            if let Some(k) = kind {
                                let dot = ui.allocate_space(egui::vec2(9.0, 9.0)).1;
                                draw_resource_dot(ui.painter(), dot.center(), k);
                            }
                            ui.label(egui::RichText::new(name).color(theme::TEXT));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    egui::ComboBox::from_id_salt(format!("restype:{name}"))
                                        .selected_text(kind.map(|k| k.label()).unwrap_or("—"))
                                        .width(96.0)
                                        .show_ui(ui, |ui| {
                                            for k in model::ResourceType::ALL {
                                                if ui
                                                    .selectable_label(kind == Some(k), k.label())
                                                    .clicked()
                                                {
                                                    model.set_resource_kind(name, k);
                                                    changed = true;
                                                }
                                            }
                                        });
                                },
                            );
                        });

                        // Per-resource time-off — in-place edit, hover-only × remove.
                        let rb_id = model
                            .resource_blocks
                            .values()
                            .find(|r| r.name.eq_ignore_ascii_case(name))
                            .map(|r| r.id);
                        if let Some(rb_id) = rb_id {
                            let sorted = model.resource_blocks[&rb_id].non_working_dates.to_vec();
                            let to_groups = group_holidays(&sorted);

                            let mut remove_to: Option<Vec<chrono::NaiveDate>> = None;
                            for g in &to_groups {
                                let ed_lbl = matches!(&settings.editing,
                                    Some(SettingsEdit::ResourceLabel(rn, d))
                                    if rn.as_str() == name.as_str() && *d == g.start);

                                let mut row_rm_rect = egui::Rect::NOTHING;
                                let mut row_rm_clicked = false;
                                let mut row_rm_hovered = false;

                                ui.add_space(2.0);
                                let row = theme::list_row(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add_space(4.0);

                                        // Date span: clickable chip(s) opening the
                                        // Aurora calendar range picker popup.
                                        let cr = theme::date_chip(ui, g.start);
                                        let anchor = cr.rect.left_bottom();
                                        let mut open_picker = cr.clicked();
                                        if g.start != g.end {
                                            ui.label(
                                                egui::RichText::new("–").color(theme::TEXT_MUTED),
                                            );
                                            open_picker |= theme::date_chip(ui, g.end).clicked();
                                        }
                                        if open_picker {
                                            settings.editing = None;
                                            settings.picker = Some(OpenDatePicker {
                                                target: DateTarget::Resource(
                                                    name.clone(),
                                                    g.start,
                                                ),
                                                state: datepicker::DatePickerState::range(
                                                    g.start, None,
                                                ),
                                                anchor,
                                                armed: false,
                                            });
                                        }
                                        if let Some(p) = &mut settings.picker {
                                            if p.target
                                                == DateTarget::Resource(name.clone(), g.start)
                                            {
                                                p.anchor = anchor;
                                            }
                                        }

                                        ui.add_space(4.0);

                                        let remove_w = 20.0;
                                        let label_avail = (ui.available_width()
                                            - remove_w
                                            - ui.spacing().item_spacing.x)
                                            .max(20.0);

                                        // Label or in-place label editor.
                                        if ed_lbl {
                                            let r = ui
                                                .scope(|ui| {
                                                    theme::style_inputs(ui);
                                                    ui.add(
                                                        egui::TextEdit::singleline(
                                                            &mut settings.edit_buf,
                                                        )
                                                        .desired_width(label_avail)
                                                        .hint_text("Reason"),
                                                    )
                                                })
                                                .inner;
                                            r.request_focus();
                                            let esc =
                                                ui.input(|i| i.key_pressed(egui::Key::Escape));
                                            if esc {
                                                settings.editing = None;
                                            } else if r.lost_focus()
                                                || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                            {
                                                let new_desc = settings.edit_buf.trim().to_string();
                                                let grp_dates = g.dates.clone();
                                                if let Some(rb) =
                                                    model.resource_blocks.get_mut(&rb_id)
                                                {
                                                    for nwd in &mut rb.non_working_dates {
                                                        if grp_dates.contains(&nwd.date) {
                                                            nwd.description = new_desc.clone();
                                                            changed = true;
                                                        }
                                                    }
                                                }
                                                settings.editing = None;
                                            }
                                        } else {
                                            let desc = if g.description.is_empty() {
                                                "—"
                                            } else {
                                                g.description.as_str()
                                            };
                                            let lr = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(desc)
                                                        .size(11.0)
                                                        .color(theme::TEXT_MUTED),
                                                )
                                                .sense(egui::Sense::click()),
                                            );
                                            if lr.clicked() {
                                                settings.editing =
                                                    Some(SettingsEdit::ResourceLabel(
                                                        name.clone(),
                                                        g.start,
                                                    ));
                                                settings.edit_buf = g.description.clone();
                                            }
                                        }

                                        // Always allocate the × slot.
                                        let (rr, rresp) = ui
                                            .with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.allocate_exact_size(
                                                        egui::vec2(remove_w, 14.0),
                                                        egui::Sense::click(),
                                                    )
                                                },
                                            )
                                            .inner;
                                        row_rm_rect = rr;
                                        row_rm_clicked = rresp.clicked();
                                        row_rm_hovered = rresp.hovered();
                                    });
                                });

                                let ptr = ui.ctx().input(|i| i.pointer.hover_pos());
                                if ptr.is_some_and(|p| row.response.rect.contains(p)) {
                                    ui.painter().text(
                                        row_rm_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "×",
                                        egui::FontId::proportional(14.0),
                                        if row_rm_hovered {
                                            theme::DANGER
                                        } else {
                                            theme::TEXT_MUTED
                                        },
                                    );
                                    if row_rm_clicked {
                                        remove_to = Some(g.dates.clone());
                                    }
                                }
                            }
                            if let Some(dts) = remove_to {
                                if let Some(rb) = model.resource_blocks.get_mut(&rb_id) {
                                    rb.non_working_dates.retain(|x| !dts.contains(&x.date));
                                    changed = true;
                                }
                                let stale = matches!(&settings.editing,
                                    Some(SettingsEdit::ResourceLabel(rn, d))
                                    if rn.as_str() == name.as_str() && dts.contains(d));
                                if stale {
                                    settings.editing = None;
                                }
                                if matches!(&settings.picker, Some(p) if matches!(&p.target,
                                    DateTarget::Resource(rn, d)
                                    if rn.as_str() == name.as_str() && dts.contains(d)))
                                {
                                    settings.picker = None;
                                }
                            }

                            // + Time off: append a blank entry and enter label-edit mode.
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                if ui
                                    .small_button(
                                        egui::RichText::new("+ Time off")
                                            .size(11.0)
                                            .color(theme::ACCENT),
                                    )
                                    .clicked()
                                {
                                    let existing: Vec<_> = model
                                        .resource_blocks
                                        .get(&rb_id)
                                        .map(|rb| {
                                            rb.non_working_dates.iter().map(|x| x.date).collect()
                                        })
                                        .unwrap_or_default();
                                    let mut new_date = model.calendar.start_date;
                                    for _ in 0..1000 {
                                        if !existing.contains(&new_date) {
                                            break;
                                        }
                                        new_date = new_date.succ_opt().unwrap_or(new_date);
                                    }
                                    if let Some(rb) = model.resource_blocks.get_mut(&rb_id) {
                                        rb.non_working_dates.push(model::NonWorkingDate {
                                            date: new_date,
                                            description: String::new(),
                                        });
                                        changed = true;
                                    }
                                    settings.editing =
                                        Some(SettingsEdit::ResourceLabel(name.clone(), new_date));
                                    settings.edit_buf = String::new();
                                }
                            });
                        }
                        ui.add_space(4.0);
                    }

                    // ── SIZES ─────────────────────────────────────────────────
                    ui.add_space(SETTINGS_SECTION_GAP);
                    theme::section_header(ui, "SIZES", Some(model.t_shirt_sizes.len()));

                    // Re-sorting is frozen while a row is being edited so rows
                    // don't jump (and drop focus) as the days value changes.
                    let order = sizes_display_order(
                        &model.t_shirt_sizes,
                        &settings.sizes_order,
                        settings.sizes_editing,
                    );

                    let mut sizes_editing_now = false;
                    let mut remove_size: Option<usize> = None;
                    for &idx in &order {
                        let mut trash_rect = egui::Rect::NOTHING;
                        let mut trash_clicked = false;
                        let mut trash_hovered = false;

                        let row = ui.horizontal(|ui| {
                            let trash_w = 22.0;
                            let right_reserve = 58.0 + trash_w + ui.spacing().item_spacing.x * 2.0;
                            let label_w = (ui.available_width() - right_reserve).max(40.0);
                            let label_resp = ui.add(
                                egui::TextEdit::singleline(&mut model.t_shirt_sizes[idx].label)
                                    .desired_width(label_w),
                            );
                            if label_resp.changed() {
                                changed = true;
                            }
                            if settings.sizes_focus == Some(idx) {
                                label_resp.request_focus();
                                settings.sizes_focus = None;
                                sizes_editing_now = true;
                            }
                            let days_resp = ui.add(
                                egui::DragValue::new(&mut model.t_shirt_sizes[idx].days)
                                    .range(1..=400)
                                    .suffix(" d"),
                            );
                            if days_resp.changed() {
                                changed = true;
                            }
                            sizes_editing_now |= label_resp.has_focus()
                                || days_resp.has_focus()
                                || days_resp.dragged();
                            // Always allocate the 🗑 slot; only paint and act on hover.
                            let (tr, tresp) = ui.allocate_exact_size(
                                egui::vec2(trash_w, 16.0),
                                egui::Sense::click(),
                            );
                            trash_rect = tr;
                            trash_clicked = tresp.clicked();
                            trash_hovered = tresp.hovered();
                        });

                        let ptr = ui.ctx().input(|i| i.pointer.hover_pos());
                        if ptr.is_some_and(|p| row.response.rect.contains(p)) {
                            ui.painter().text(
                                trash_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "🗑",
                                egui::FontId::proportional(14.0),
                                if trash_hovered {
                                    theme::DANGER
                                } else {
                                    theme::TEXT_MUTED
                                },
                            );
                            if trash_clicked {
                                remove_size = Some(idx);
                            }
                        }
                        ui.add_space(2.0);
                    }
                    settings.sizes_order = order;
                    settings.sizes_editing = sizes_editing_now;
                    if let Some(i) = remove_size {
                        if i < model.t_shirt_sizes.len() {
                            model.t_shirt_sizes.remove(i);
                            changed = true;
                        }
                    }
                    ui.add_space(SETTINGS_ROW_GAP);
                    ui.horizontal(|ui| {
                        if theme::add_button(ui).clicked() {
                            model.t_shirt_sizes.push(model::TShirtSize {
                                label: "New".to_string(),
                                days: 5,
                            });
                            settings.sizes_focus = Some(model.t_shirt_sizes.len() - 1);
                            changed = true;
                        }
                        ui.label(
                            egui::RichText::new("Add size")
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                        );
                    });
                }); // ScrollArea::vertical
        });

    // ── Date-range picker popup ───────────────────────────────────────────────
    // Drawn over the panel as a foreground Area anchored under the clicked row.
    // Committing a date re-anchors that group's span; Esc or a click outside the
    // popup dismisses it without change.
    if let Some(mut open) = settings.picker.take() {
        let area = egui::Area::new(egui::Id::new("settings_date_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(open.anchor)
            .show(ctx, |ui| {
                datepicker::aurora_date_picker(ui, &mut open.state)
            });
        let committed = area.inner;
        let area_rect = area.response.rect;
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let click_outside = open.armed
            && ctx.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !area_rect.contains(p))
            });
        if let Some(result) = committed {
            let applied = match &open.target {
                DateTarget::Holiday(start) => {
                    apply_date_pick(&mut model.calendar.non_working_dates, *start, &result)
                }
                DateTarget::Resource(rn, start) => {
                    let rb_id = model
                        .resource_blocks
                        .values()
                        .find(|r| r.name.eq_ignore_ascii_case(rn))
                        .map(|r| r.id);
                    match rb_id.and_then(|id| model.resource_blocks.get_mut(&id)) {
                        Some(rb) => apply_date_pick(&mut rb.non_working_dates, *start, &result),
                        None => false,
                    }
                }
            };
            changed |= applied;
            // Committed → leave `settings.picker` cleared (closed).
        } else if esc || click_outside {
            // Dismissed → leave cleared.
        } else {
            open.armed = true;
            settings.picker = Some(open); // still open next frame
        }
    }

    if close {
        settings.open = false;
    }
    if changed {
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn top_bar_ui(
    mut contexts: EguiContexts,
    mut target: ResMut<CameraTarget>,
    mut model: ResMut<model::Model>,
    mut schedule: ResMut<schedule::Schedule>,
    mut drill: ResMut<schedule::DrillScope>,
    mut settings: ResMut<SettingsState>,
    mut help: ResMut<HelpState>,
    mut import_state: ResMut<ImportState>,
    mut selected_plan: ResMut<SelectedPlan>,
    mut view: ResMut<ViewMode>,
    mut pending_doc: ResMut<document::PendingDocument>,
    mut file_menu: ResMut<document::FileMenuState>,
    current_doc: Res<document::CurrentDocument>,
    windows: Query<&Window>,
    today: Res<schedule::TodayMarker>,
    conn: NonSend<rusqlite::Connection>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut clear_all = false;
    let mut export_csv = false;
    let mut import_csv = false;
    // The branch (if any) the user asked to promote to main this frame.
    let mut accept_branch: Option<model::PlanId> = None;
    // Breadcrumb path (block ids) to optionally truncate to, and a rollup toggle.
    let mut jump_to: Option<usize> = None; // new path length
    let mut toggle_rollup: Option<model::WorkBlockId> = None;
    egui::TopBottomPanel::top("top_bar")
        .frame(
            egui::Frame::new()
                // Opaque — a translucent fill let the timeline show through the
                // empty area to the right of the breadcrumb.
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let text = egui::RichText::new("brick_road")
                    .size(18.0)
                    .color(theme::ACCENT);
                let btn = egui::Button::new(text)
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE);
                if ui.add(btn).on_hover_text("Fit to view [F]").clicked() {
                    if let Some(new_target) = model
                        .main_plan_id()
                        .and_then(|p| camera::fit_to_blocks(&model, p, &windows))
                    {
                        *target = new_target;
                    }
                }

                // Current document name + FILE menu (New / Open / Duplicate /
                // recent documents). Selections go through PendingDocument and
                // are applied by `apply_document_request`.
                let doc_name = document::doc_display_name(&current_doc.0);
                let file_btn = theme::pill_button(ui, &format!("▤ {doc_name}"), file_menu.open)
                    .on_hover_text(current_doc.0.to_string_lossy().to_string());
                if file_btn.clicked() {
                    file_menu.open = !file_menu.open;
                    file_menu.armed = false;
                }
                if file_menu.open {
                    let menu_item = |ui: &mut egui::Ui, label: &str| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(label).size(13.0).color(theme::TEXT),
                            )
                            .selectable(false)
                            .sense(egui::Sense::click()),
                        )
                    };
                    let area = egui::Area::new(egui::Id::new("file_menu"))
                        .fixed_pos(egui::pos2(
                            file_btn.rect.left(),
                            file_btn.rect.bottom() + 4.0,
                        ))
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::new()
                                .fill(theme::PANEL_HI)
                                .stroke(egui::Stroke::new(1.0, theme::STROKE))
                                .corner_radius(egui::CornerRadius::same(6))
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_min_width(200.0);
                                    if menu_item(ui, "＋ New…").clicked() {
                                        if let Some(p) = rfd::FileDialog::new()
                                            .set_file_name("untitled.brickroad")
                                            .add_filter("Brick Road", &[document::DOC_EXTENSION])
                                            .save_file()
                                        {
                                            pending_doc.0 = Some(document::DocRequest::New(
                                                document::with_doc_extension(p),
                                            ));
                                        }
                                        file_menu.open = false;
                                    }
                                    ui.add_space(2.0);
                                    if menu_item(ui, "▸ Open…").clicked() {
                                        if let Some(p) = rfd::FileDialog::new()
                                            .add_filter(
                                                "Brick Road",
                                                &[document::DOC_EXTENSION, "db"],
                                            )
                                            .pick_file()
                                        {
                                            pending_doc.0 = Some(document::DocRequest::Open(p));
                                        }
                                        file_menu.open = false;
                                    }
                                    ui.add_space(2.0);
                                    if menu_item(ui, "⧉ Duplicate…").clicked() {
                                        if let Some(p) = rfd::FileDialog::new()
                                            .set_file_name(format!("{doc_name} copy.brickroad"))
                                            .add_filter("Brick Road", &[document::DOC_EXTENSION])
                                            .save_file()
                                        {
                                            pending_doc.0 = Some(document::DocRequest::Duplicate(
                                                document::with_doc_extension(p),
                                            ));
                                        }
                                        file_menu.open = false;
                                    }
                                    let recents: Vec<std::path::PathBuf> = document::load_recents()
                                        .into_iter()
                                        .filter(|p| *p != current_doc.0)
                                        .collect();
                                    if !recents.is_empty() {
                                        ui.add_space(4.0);
                                        ui.separator();
                                        ui.add_space(2.0);
                                        ui.label(
                                            egui::RichText::new("RECENT")
                                                .size(10.5)
                                                .color(theme::TEXT_MUTED),
                                        );
                                        for p in recents {
                                            let resp =
                                                menu_item(ui, &document::doc_display_name(&p))
                                                    .on_hover_text(p.to_string_lossy().to_string());
                                            if resp.clicked() {
                                                pending_doc.0 = Some(document::DocRequest::Open(p));
                                                file_menu.open = false;
                                            }
                                        }
                                    }
                                });
                        });
                    // Esc or a click outside the popup dismisses it; `armed` is
                    // false on the opening frame so the opening click doesn't.
                    let esc = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape));
                    let clicked_out = file_menu.armed
                        && ui.ctx().input(|i| {
                            i.pointer.any_click()
                                && i.pointer.interact_pos().is_some_and(|p| {
                                    !area.response.rect.contains(p) && !file_btn.rect.contains(p)
                                })
                        });
                    if esc || clicked_out {
                        file_menu.open = false;
                    }
                    file_menu.armed = true;
                }

                // Drill-in breadcrumb: Plan / Block / Block… Click a crumb to
                // jump out to that level. A roll-up toggle for the current block.
                if !drill.path.is_empty() {
                    ui.separator();
                    if ui
                        .selectable_label(
                            false,
                            egui::RichText::new("Plan").color(theme::TEXT_MUTED),
                        )
                        .clicked()
                    {
                        jump_to = Some(0);
                    }
                    for (i, id) in drill.path.iter().enumerate() {
                        ui.label(egui::RichText::new("/").color(theme::TEXT_MUTED));
                        let name = model
                            .work_blocks
                            .get(id)
                            .map(|wb| wb.name.clone())
                            .unwrap_or_else(|| "?".to_string());
                        let is_last = i + 1 == drill.path.len();
                        let text = egui::RichText::new(name).color(if is_last {
                            theme::TEXT
                        } else {
                            theme::TEXT_MUTED
                        });
                        if ui.selectable_label(is_last, text).clicked() {
                            jump_to = Some(i + 1);
                        }
                    }
                    if let Some(&current) = drill.path.last() {
                        let mut rolled = model
                            .work_blocks
                            .get(&current)
                            .map(|wb| wb.rollup)
                            .unwrap_or(false);
                        if ui
                            .checkbox(&mut rolled, "Roll up")
                            .on_hover_text("Size this block from its children")
                            .changed()
                        {
                            toggle_rollup = Some(current);
                        }
                    }
                }

                // When a branch is selected, offer to promote it to main.
                // No confirm, no undo — DELIBERATE product decision (br-236/237,
                // both closed wontfix). Destructive actions are immediate;
                // recovery is manual redo, "like a game, just do it again."
                // Do NOT add a confirmation dialog or undo machinery here.
                if let Some(bid) = selected_plan.0 {
                    if plan_is_acceptable(&model, bid) {
                        ui.separator();
                        let name = model.plans[&bid].name.clone();
                        if theme::pill_button(ui, "▲ ACCEPT AS MAIN", true)
                            .on_hover_text(format!(
                                "Rewrite main to adopt \"{name}\" and delete this branch"
                            ))
                            .clicked()
                        {
                            accept_branch = Some(bid);
                        }
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Far right: the settings gear (lit while the panel is open).
                    if theme::pill_button(ui, "⚙", settings.open)
                        .on_hover_text("Settings")
                        .clicked()
                    {
                        settings.open = !settings.open;
                    }
                    // Just left of the gear: keyboard-shortcuts help.
                    if theme::pill_button(ui, "?", help.open)
                        .on_hover_text("Keyboard shortcuts")
                        .clicked()
                    {
                        help.open = !help.open;
                    }
                    ui.add_space(8.0);
                    if theme::pill_button(ui, "⬇ EXPORT", false)
                        .on_hover_text("Export plan blocks to CSV")
                        .clicked()
                    {
                        export_csv = true;
                    }
                    if theme::pill_button(ui, "⬆ IMPORT", false)
                        .on_hover_text("Import blocks from CSV")
                        .clicked()
                    {
                        import_csv = true;
                    }
                    ui.add_space(8.0);
                    // TODAY is the lit/primary control in the Aurora reference.
                    if theme::pill_button(ui, "→ TODAY", true).clicked() {
                        target.pos.x = calendar::day_to_x(
                            today.day,
                            &model.calendar.global_off_days(),
                            &model.calendar,
                        );
                    }
                    if theme::pill_button(ui, "⤢ FIT", false)
                        .on_hover_text("Fit to view [F]")
                        .clicked()
                    {
                        if let Some(new_target) = model
                            .main_plan_id()
                            .and_then(|p| camera::fit_to_blocks(&model, p, &windows))
                        {
                            *target = new_target;
                        }
                    }
                    if theme::pill_button(ui, "⌂ HOME", false)
                        .on_hover_text("Re-center [Home]")
                        .clicked()
                    {
                        if let Ok(window) = windows.single() {
                            *target = camera::home_target(window, today.day, &model.calendar);
                        }
                    }
                    ui.add_space(8.0);
                    // By Plan / By Person view toggle.
                    if theme::pill_button(ui, "BY PLAN", !view.by_person).clicked() {
                        view.by_person = false;
                    }
                    if theme::pill_button(ui, "BY PERSON", view.by_person).clicked() {
                        view.by_person = true;
                        if view.plan.is_none() {
                            view.plan = model.main_plan_id();
                        }
                    }
                    if view.by_person {
                        ui.add_space(4.0);
                        let current = view.plan.or_else(|| model.main_plan_id());
                        let current_name = current
                            .and_then(|id| model.plans.get(&id))
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| "(none)".to_string());
                        let mut sorted: Vec<model::PlanId> = model.plans.keys().copied().collect();
                        sorted.sort_by_key(|&id| {
                            model.plans[&id].branch_start_day.map_or(i32::MIN, |d| d)
                        });
                        egui::ComboBox::from_id_salt("view_plan_select")
                            .selected_text(&current_name)
                            .show_ui(ui, |ui| {
                                for id in &sorted {
                                    let name = model.plans[id].name.clone();
                                    if ui.selectable_label(current == Some(*id), &name).clicked() {
                                        view.plan = Some(*id);
                                    }
                                }
                            });
                    }
                    ui.add_space(8.0);
                    // Dev: wipe all blocks, branches, and links; keep one empty
                    // main plan to start fresh from. Coral-red DANGER pill.
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("⌫ CLEAR")
                                    .size(12.5)
                                    .color(theme::DANGER),
                            )
                            .fill(egui::Color32::TRANSPARENT)
                            .stroke(egui::Stroke::new(1.0, theme::DANGER))
                            .corner_radius(10.0),
                        )
                        .on_hover_text("Dev: delete all blocks, branches, and links")
                        .clicked()
                    {
                        clear_all = true;
                    }
                });
            });
        });

    if import_csv && import_state.pending.is_none() {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    import_state.pending = Some(content);
                    import_state.errors.clear();
                    // Default replace plan = selected branch or main.
                    import_state.replace_plan_id = selected_plan.0.or_else(|| model.main_plan_id());
                }
                Err(e) => error!("CSV read failed: {e}"),
            }
        }
    }
    if export_csv {
        let plan_id = selected_plan.0.or_else(|| model.main_plan_id());
        if let Some(pid) = plan_id {
            if let Some(plan) = model.plans.get(&pid) {
                let csv = csv_export::plan_to_csv(plan, &model);
                let file_name = format!("{}.csv", plan.name.replace('/', "_"));
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(&file_name)
                    .add_filter("CSV", &["csv"])
                    .save_file()
                {
                    if let Err(e) = std::fs::write(&path, csv) {
                        error!("CSV export failed: {e}");
                    }
                }
            }
        }
    }
    if clear_all {
        model.clear_all_work();
        *schedule = schedule::Schedule::default();
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
    if let Some(bid) = accept_branch {
        // Promote the branch to main, persist, and drop the now-gone selection.
        // The mutated model drives the canvas refresh via change-detection.
        // One-click rewrite of main + sibling branches, no confirm/undo — by
        // design (br-236/237 wontfix). Recovery is manual redo, not a guardrail.
        model.accept_plan_as_main(bid);
        selected_plan.0 = None;
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
    if let Some(len) = jump_to {
        drill.path.truncate(len);
    }
    if let Some(id) = toggle_rollup {
        if let Some(wb) = model.work_blocks.get_mut(&id) {
            wb.rollup = !wb.rollup;
        }
        model.recompute_rollup(id);
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
}

/// Whether `plan_id` can be promoted to main: it must be an existing *branch*
/// (a forked plan, i.e. one with a `branch_start_day`). The baseline/main plan
/// has no fork day and can never accept itself; a missing id is not acceptable.
fn plan_is_acceptable(model: &model::Model, plan_id: model::PlanId) -> bool {
    model
        .plans
        .get(&plan_id)
        .is_some_and(|p| p.branch_start_day.is_some())
}

/// The branch (forked plan) whose marker is currently selected, if any.
/// Selecting a branch by clicking its marker arms the Delete key to remove it.
#[derive(Resource, Default)]
pub struct SelectedPlan(pub Option<model::PlanId>);

/// View mode: By Plan (default) vs By Person. In By-Person mode all editing and
/// drilling is disabled (run-condition `editing_enabled`) and branch swimlanes are
/// hidden.
#[derive(Resource, Default)]
pub struct ViewMode {
    pub by_person: bool,
    pub plan: Option<model::PlanId>,
}

/// Resource-gutter state: which row has an open picker popup and, when editing,
/// the text buffer for the row name.
#[derive(Resource, Default)]
pub struct RowRename {
    pub editing: Option<(model::PlanId, Option<model::WorkBlockId>, i32)>,
    pub buf: String,
    pub picker_open: Option<(model::PlanId, Option<model::WorkBlockId>, i32)>,
    /// An in-progress gutter drag-reorder: (plan, scope, source row).
    pub drag: Option<(model::PlanId, Option<model::WorkBlockId>, i32)>,
}

/// Which settings field is currently being edited in-place (one at a time).
#[derive(Clone, PartialEq)]
enum SettingsEdit {
    StartDate,
    /// Holiday group's label — keyed by the group's start date (unique).
    HolidayLabel(chrono::NaiveDate),
    /// Per-resource time-off label — (resource name, group start date).
    ResourceLabel(String, chrono::NaiveDate),
}

/// Whether the keyboard-shortcuts help modal is open. Mirrors `SettingsState.open`.
#[derive(Resource, Default)]
pub struct HelpState {
    pub open: bool,
}

/// The keyboard/mouse reference shown by `help_modal_ui` — one source of truth,
/// grouped `(section, &[(key, description)])`. Keep in sync with real bindings.
const HELP_KEYMAP: &[(&str, &[(&str, &str)])] = &[
    (
        "NAVIGATION",
        &[
            ("Home", "Recenter / home view"),
            ("F", "Fit all blocks in view"),
            ("Esc", "Drill out one level"),
            ("2-finger / middle / right drag", "Pan the canvas"),
            ("Pinch · Ctrl/Cmd + scroll", "Zoom"),
        ],
    ),
    (
        "BLOCKS",
        &[
            ("Double-click canvas", "Create a block"),
            ("Double-click block", "Drill into it"),
            ("N", "Toggle create mode"),
            ("Drag block", "Move it (the whole selection if several)"),
            ("Drag gutter name", "Reorder resource rows"),
            ("Type a letter", "Rename the selected block"),
            ("Enter · Esc", "Commit · cancel an edit"),
        ],
    ),
    (
        "SELECTION",
        &[
            ("Left-drag canvas", "Marquee select"),
            ("Shift/Ctrl-click", "Add / remove from selection"),
            ("Delete · Backspace", "Delete the selection"),
        ],
    ),
    (
        "EDIT",
        &[
            ("Ctrl/Cmd + C", "Copy selection"),
            ("Ctrl/Cmd + V", "Paste at cursor"),
            ("Ctrl/Cmd + Z", "Undo"),
            ("Ctrl/Cmd + O", "Open the selected block's URL"),
        ],
    ),
];

/// Centered, Aurora-styled modal listing the keyboard & mouse shortcuts. Toggled
/// by the top-bar `?`; dismissed via the ✕, Esc, or a click on the dim backdrop.
fn help_modal_ui(mut contexts: EguiContexts, mut help: ResMut<HelpState>) {
    if !help.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut close = false;
    let resp = egui::Modal::new(egui::Id::new("help_modal"))
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::STROKE))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(18)),
        )
        .show(ctx, |ui| {
            ui.set_max_width(390.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Keyboard & Mouse")
                        .size(17.0)
                        .strong()
                        .color(theme::ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui::RichText::new("✕").size(14.0).color(theme::TEXT_MUTED))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
            ui.add_space(10.0);
            for &(section, rows) in HELP_KEYMAP {
                theme::section_header(ui, section, None);
                egui::Grid::new(("help_grid", section))
                    .num_columns(2)
                    .spacing([14.0, 5.0])
                    .show(ui, |ui| {
                        for &(key, desc) in rows {
                            theme::chip(ui, key);
                            ui.label(egui::RichText::new(desc).color(theme::TEXT));
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
            }
        });
    if close || resp.should_close() {
        help.open = false;
    }
}

/// State for the right-side settings fly-out: which field (if any) is being
/// edited in-place, plus the shared edit buffer.
#[derive(Resource, Default)]
pub struct SettingsState {
    pub open: bool,
    /// The field currently being edited in-place (one at a time).
    editing: Option<SettingsEdit>,
    /// Shared text buffer for whichever field is in `editing`.
    edit_buf: String,
    /// Open Aurora date-range picker popup, if any (one at a time).
    picker: Option<OpenDatePicker>,
    /// Cached SIZES display order (indices into `model.t_shirt_sizes`), kept
    /// frozen while a size row is being edited (see `sizes_display_order`).
    sizes_order: Vec<usize>,
    /// True while a SIZES row widget had focus last frame — freezes re-sorting.
    sizes_editing: bool,
    /// Model index of a just-added size whose label should grab focus.
    sizes_focus: Option<usize>,
}

/// Transient state for the CSV import modal.
#[derive(Resource, Default)]
struct ImportState {
    /// CSV text content loaded from a file, awaiting mode selection.
    pending: Option<String>,
    /// True = create a new plan; false = replace an existing plan.
    is_new: bool,
    /// Name buffer for the "new plan" branch.
    new_plan_name: String,
    /// Which existing plan to replace (defaults to selected branch / main).
    replace_plan_id: Option<model::PlanId>,
    /// Validation or IO errors to surface in the modal.
    errors: Vec<String>,
}

/// Shows the CSV import modal when a file has been chosen but the user has not
/// yet confirmed the import mode.  Runs every frame while `ImportState.pending`
/// is `Some`.
fn import_modal_ui(
    mut contexts: EguiContexts,
    mut import_state: ResMut<ImportState>,
    mut model: ResMut<model::Model>,
    mut schedule: ResMut<schedule::Schedule>,
    conn: NonSend<rusqlite::Connection>,
) {
    if import_state.pending.is_none() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut do_import = false;
    let mut do_cancel = false;

    egui::Window::new("Import CSV")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.radio_value(&mut import_state.is_new, true, "Create new plan");
            if import_state.is_new {
                ui.horizontal(|ui| {
                    ui.label("Plan name:");
                    ui.text_edit_singleline(&mut import_state.new_plan_name);
                });
            }

            ui.radio_value(&mut import_state.is_new, false, "Replace existing plan");
            if !import_state.is_new {
                // Dropdown of all plans by name.
                let plans: Vec<(model::PlanId, String)> = model
                    .plans
                    .values()
                    .map(|p| (p.id, p.name.clone()))
                    .collect();
                let selected_name = import_state
                    .replace_plan_id
                    .and_then(|id| model.plans.get(&id))
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("import_plan_combo")
                    .selected_text(&selected_name)
                    .show_ui(ui, |ui| {
                        for (pid, name) in &plans {
                            ui.selectable_value(
                                &mut import_state.replace_plan_id,
                                Some(*pid),
                                name,
                            );
                        }
                    });
                ui.colored_label(
                    egui::Color32::from_rgb(228, 132, 122),
                    "⚠ This will permanently replace all blocks in the selected plan.",
                );
            }

            if !import_state.errors.is_empty() {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(228, 132, 122), "Import errors:");
                for e in &import_state.errors {
                    ui.label(e);
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Import").clicked() {
                    do_import = true;
                }
                if ui.button("Cancel").clicked() {
                    do_cancel = true;
                }
            });
        });

    if do_cancel {
        *import_state = ImportState::default();
        return;
    }

    if do_import {
        let csv = import_state.pending.clone().unwrap_or_default();
        let rows = match csv_export::parse_csv(&csv) {
            Ok(rows) => rows,
            Err(errors) => {
                import_state.errors = errors;
                return;
            }
        };

        if import_state.is_new {
            let name = import_state.new_plan_name.trim().to_string();
            let name = if name.is_empty() {
                "Imported".to_string()
            } else {
                name
            };
            let plan_id = model.create_plan(&name, None);
            let errors = csv_export::populate_plan(&rows, plan_id, &mut model);
            if !errors.is_empty() {
                import_state.errors = errors;
                // Roll back: drop the newly created plan.
                model.delete_plan(plan_id);
                return;
            }
        } else {
            let Some(plan_id) = import_state.replace_plan_id else {
                import_state.errors = vec!["No plan selected to replace.".to_string()];
                return;
            };
            csv_export::clear_plan_blocks(&mut model, plan_id);
            let errors = csv_export::populate_plan(&rows, plan_id, &mut model);
            if !errors.is_empty() {
                import_state.errors = errors;
                return;
            }
        }

        *schedule = schedule::Schedule::default();
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model after import failed: {e}");
        }
        *import_state = ImportState::default();
    }
}

/// Returns the non-active forked plan whose branch marker is within `hit_world`
/// units of `world_x`, nearest first. Used both to select a branch on click and
/// to keep block-creation clicks from landing on a marker.
pub fn branch_plan_at_x(
    model: &model::Model,
    active_id: model::PlanId,
    world_x: f32,
    hit_world: f32,
) -> Option<model::PlanId> {
    let mut best: Option<(f32, model::PlanId)> = None;
    let off = model.calendar.global_off_days();
    for plan in model.plans.values() {
        if plan.id == active_id {
            continue;
        }
        let Some(day) = plan.branch_start_day else {
            continue;
        };
        let dist = (world_x - calendar::day_to_x(day, &off, &model.calendar)).abs();
        if dist <= hit_world && best.is_none_or(|(bd, _)| dist < bd) {
            best = Some((dist, plan.id));
        }
    }
    best.map(|(_, id)| id)
}

/// Left-click on (or very near) a branch marker selects that branch; the Delete
/// key then removes it. Selecting a branch clears any block/dependency
/// selection so Delete is unambiguous.
#[allow(clippy::too_many_arguments)]
fn handle_branch_selection(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    cam_proj: Query<&Projection, With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    model: Res<model::Model>,
    mut selected_plan: ResMut<SelectedPlan>,
    mut selected_block: ResMut<blocks::SelectedBlock>,
    mut selected_dep: ResMut<blocks::SelectedDependency>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    // Ctrl+click is the fork gesture, not selection.
    if keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight) {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }
    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_gt)) = camera.single() else {
        return;
    };
    let Some(world) = window
        .cursor_position()
        .and_then(|c| cam.viewport_to_world_2d(cam_gt, c).ok())
    else {
        return;
    };
    let scale = cam_proj
        .single()
        .ok()
        .and_then(crate::blocks::ortho_scale)
        .unwrap_or(1.0);

    // ~6 screen pixels of grab tolerance on either side of the marker line.
    // Prefer the lane the click is in (disambiguates same-day forks by height);
    // fall back to the nearest marker by x for clicks on the line above the
    // lanes (in the main timeline area).
    let hit = 6.0 * scale;
    let plan = bands::plan_marker_in_lane_at(&model, world, hit).or_else(|| {
        model
            .main_plan_id()
            .and_then(|p| branch_plan_at_x(&model, p, world.x, hit))
    });
    if let Some(id) = plan {
        selected_plan.0 = Some(id);
        selected_block.0 = None;
        selected_dep.0 = None;
    }
}

/// Deletes the selected branch on Delete/Backspace. Block deletion lives in
/// `blocks::handle_block_delete`; the two never collide because selecting a
/// branch clears the block selection and vice versa.
fn handle_branch_delete(
    mut egui_ctx: EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    name_edit: Res<blocks::NameEditState>,
    mut selected_plan: ResMut<SelectedPlan>,
    mut model: ResMut<model::Model>,
    conn: NonSend<rusqlite::Connection>,
) {
    if name_edit.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            return;
        }
    }
    if !(keyboard.just_pressed(KeyCode::Delete) || keyboard.just_pressed(KeyCode::Backspace)) {
        return;
    }
    if let Some(id) = selected_plan.0.take() {
        model.delete_plan(id);
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn group_holidays_merges_consecutive_same_label() {
        use chrono::NaiveDate;
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        let nwd = |date, desc: &str| model::NonWorkingDate {
            date,
            description: desc.to_string(),
        };
        let dates = vec![
            nwd(d(12, 26), "Christmas"),
            nwd(d(12, 24), "Christmas"),
            nwd(d(12, 25), "Christmas"),
            nwd(d(7, 4), "July 4"),
            nwd(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), "Christmas"),
        ];
        let groups = group_holidays(&dates);
        assert_eq!(
            groups.len(),
            3,
            "xmas run + july4 + far new-year = 3 groups"
        );
        let xmas = groups
            .iter()
            .find(|g| g.start == d(12, 24))
            .expect("christmas group");
        assert_eq!(xmas.end, d(12, 26));
        assert_eq!(xmas.dates.len(), 3);
        assert_eq!(xmas.description, "Christmas");
    }

    #[test]
    fn expand_date_range_inclusive_and_clamped() {
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        // Inclusive multi-day span.
        assert_eq!(
            expand_date_range(d(7, 1), d(7, 3)),
            vec![d(7, 1), d(7, 2), d(7, 3)]
        );
        // end == start is a single day.
        assert_eq!(expand_date_range(d(7, 1), d(7, 1)), vec![d(7, 1)]);
        // end < start collapses to a single day at start.
        assert_eq!(expand_date_range(d(7, 5), d(7, 1)), vec![d(7, 5)]);
        // A wildly large span is clamped to start + 366 days inclusive.
        let huge = expand_date_range(d(1, 1), NaiveDate::from_ymd_opt(2030, 1, 1).unwrap());
        assert_eq!(huge.len(), 367, "clamped to start + 366 days inclusive");
    }

    #[test]
    fn set_date_range_replaces_group_and_dedups() {
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        let nwd = |date, desc: &str| model::NonWorkingDate {
            date,
            description: desc.to_string(),
        };
        // A 2-day "Trip" group plus an unrelated holiday.
        let mut dates = vec![
            nwd(d(7, 1), "Trip"),
            nwd(d(7, 2), "Trip"),
            nwd(d(1, 1), "NY"),
        ];
        // Re-anchor the Trip group to a 3-day span starting one day later.
        set_date_range(&mut dates, &[d(7, 1), d(7, 2)], d(7, 2), d(7, 4), "Trip");
        let mut trip: Vec<_> = dates
            .iter()
            .filter(|x| x.description == "Trip")
            .map(|x| x.date)
            .collect();
        trip.sort();
        assert_eq!(trip, vec![d(7, 2), d(7, 3), d(7, 4)]);
        // Unrelated holiday is untouched; no duplicate dates.
        assert!(dates
            .iter()
            .any(|x| x.date == d(1, 1) && x.description == "NY"));
        assert_eq!(dates.len(), 4);
    }

    #[test]
    fn apply_date_pick_range_reanchors_group() {
        use crate::datepicker::DatePickerResult;
        let d = |m, dd| NaiveDate::from_ymd_opt(2025, m, dd).unwrap();
        let nwd = |date, desc: &str| model::NonWorkingDate {
            date,
            description: desc.to_string(),
        };
        let mut dates = vec![nwd(d(7, 1), "PTO")];
        let applied = apply_date_pick(
            &mut dates,
            d(7, 1),
            &DatePickerResult::Range(d(7, 1), d(7, 3)),
        );
        assert!(applied);
        let mut got: Vec<_> = dates.iter().map(|x| x.date).collect();
        got.sort();
        assert_eq!(got, vec![d(7, 1), d(7, 2), d(7, 3)]);
        assert!(dates.iter().all(|x| x.description == "PTO"));
        // A start that matches no group applies nothing.
        assert!(!apply_date_pick(
            &mut dates,
            d(12, 25),
            &DatePickerResult::Single(d(12, 25))
        ));
    }

    #[test]
    fn sizes_display_order_sorts_and_freezes() {
        let s = |label: &str, days| model::TShirtSize {
            label: label.to_string(),
            days,
        };
        let sizes = vec![s("M", 15), s("XS", 5), s("L", 25)];
        // Unfrozen: sorted by days ascending.
        assert_eq!(sizes_display_order(&sizes, &[], false), vec![1, 0, 2]);
        // Frozen with a still-valid cache: cache wins even though it no longer
        // matches the sorted order (a mid-edit days change must not re-sort).
        assert_eq!(sizes_display_order(&sizes, &[0, 1, 2], true), vec![0, 1, 2]);
        // Frozen but the cache is stale (a size was added/removed): re-sort.
        assert_eq!(sizes_display_order(&sizes, &[0, 1], true), vec![1, 0, 2]);
        // Ties keep insertion order (stable sort).
        let tied = vec![s("A", 5), s("B", 5)];
        assert_eq!(sizes_display_order(&tied, &[], false), vec![0, 1]);
    }

    #[test]
    fn help_keymap_is_well_formed() {
        assert!(!HELP_KEYMAP.is_empty(), "the help modal needs sections");
        for (section, rows) in HELP_KEYMAP {
            assert!(!section.is_empty(), "section name must not be empty");
            assert!(!rows.is_empty(), "section {section} has no rows");
            for (key, desc) in *rows {
                assert!(!key.is_empty(), "empty key in {section}");
                assert!(!desc.is_empty(), "empty description in {section}");
            }
        }
    }

    #[test]
    fn only_branches_are_acceptable_as_main() {
        let mut m = model::Model::default();
        let main = m.create_plan("main", None);
        let branch = m.fork_main(0).unwrap();
        assert!(
            plan_is_acceptable(&m, branch),
            "a forked branch can be accepted"
        );
        assert!(
            !plan_is_acceptable(&m, main),
            "the baseline/main plan cannot accept itself"
        );
        assert!(
            !plan_is_acceptable(&m, model::PlanId(99_999)),
            "a missing plan id is not acceptable"
        );
    }

    #[test]
    fn week_bands_at_calendar_week_boundaries() {
        let mut model = model::Model::default();
        model.calendar.start_date = NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(); // Monday
        let xs = weekend_band_positions(12, &model);
        // Monday start: seams land after each Friday — days 5, 10, 15.
        assert!(xs.contains(&(5.0 * PIXELS_PER_DAY)));
        assert!(xs.contains(&(10.0 * PIXELS_PER_DAY)));
    }

    #[test]
    fn week_bands_anchor_to_weeks_not_start_day() {
        // Starting mid-week, the first seam falls after that week's Friday — not
        // a naive five working days from day 0.
        let mut model = model::Model::default();
        model.calendar.start_date = NaiveDate::from_ymd_opt(2025, 1, 8).unwrap(); // Wednesday
        let xs = weekend_band_positions(12, &model);
        // Wed(0) Thu(1) Fri(2) → weekend → seam at day 3, not day 5.
        assert!(xs.contains(&(3.0 * PIXELS_PER_DAY)));
        assert!(!xs.contains(&(5.0 * PIXELS_PER_DAY)));
    }

    #[test]
    fn period_bands_start_jan_produces_bands() {
        let cfg = model::CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            ..model::CalendarConfig::default()
        };
        // 130 working days covers Jan–Jun 2025.
        let bands = period_band_spans(&cfg, 130);
        assert!(!bands.is_empty(), "should produce at least one month band");
        // All widths positive.
        for (_, w, _) in &bands {
            assert!(*w > 0.0, "band width should be positive");
        }
    }

    #[test]
    fn period_bands_use_builtin_quarter_tints() {
        let cfg = model::CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            ..model::CalendarConfig::default()
        };
        let bands = period_band_spans(&cfg, 25);
        // Jan is Q1, first month → Q1 tint at full QUARTER_TINT_ALPHA.
        let (_, _, color) = bands[0];
        assert_eq!([color[0], color[1], color[2]], QUARTER_TINTS[0]);
        assert!((color[3] - QUARTER_TINT_ALPHA).abs() < 1e-5);
    }

    #[test]
    fn period_bands_alternating_alpha() {
        let cfg = model::CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            ..model::CalendarConfig::default()
        };
        // Span enough to cover Jan and Feb (both Q1: month_in_quarter 0 and 1).
        let bands = period_band_spans(&cfg, 45);
        // Jan is month_in_quarter=0 (even): full alpha.
        // Feb is month_in_quarter=1 (odd): 0.7× alpha.
        let base_alpha = QUARTER_TINT_ALPHA;
        let jan = &bands[0];
        let feb = &bands[1];
        assert!(
            (jan.2[3] - base_alpha).abs() < 1e-5,
            "Jan should have full alpha"
        );
        assert!(
            (feb.2[3] - base_alpha * 0.7).abs() < 1e-5,
            "Feb should have 0.7× alpha"
        );
    }

    #[test]
    fn commit_row_name_merges_blocks_onto_existing_row() {
        // Merge semantics: naming row A the same as row B moves A's placed blocks to B.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();

        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        let scope: Option<model::WorkBlockId> = None;

        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(scope, 3, "Backend".to_string());
        let block = m.create_work_block("Task");
        // duration_days must be > 0 so visible_blocks includes this block.
        m.work_blocks.get_mut(&block).unwrap().duration_days = 5;
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(block);
        m.set_block_row(plan_id, block, 7);
        db::save_model(&conn, &m).unwrap();

        commit_row_name(&mut m, &conn, plan_id, scope, 7, "Backend");

        assert_eq!(
            m.block_row(plan_id, block),
            3,
            "block should move to the target row"
        );
        assert_eq!(
            m.plans[&plan_id].row_name(scope, 7),
            None,
            "source row name cleared"
        );
    }

    #[test]
    fn commit_row_name_merges_onto_row_beyond_old_64_cap() {
        // Regression: the old `0..64` scan missed named rows ≥ 64, silently
        // creating a duplicate instead of merging. Verified by the fix that
        // derives the bound from the actual `row_names` Vec length.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();

        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        let scope: Option<model::WorkBlockId> = None;

        // Row 100 — beyond the old 0..64 cap — is already named "Infra".
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(scope, 100, "Infra".to_string());
        let block = m.create_work_block("Deploy");
        // duration_days must be > 0 so visible_blocks includes this block.
        m.work_blocks.get_mut(&block).unwrap().duration_days = 3;
        m.plans.get_mut(&plan_id).unwrap().root_blocks.push(block);
        m.set_block_row(plan_id, block, 1);
        db::save_model(&conn, &m).unwrap();

        // Naming row 1 "Infra" must find the target at row 100.
        commit_row_name(&mut m, &conn, plan_id, scope, 1, "Infra");

        assert_eq!(
            m.block_row(plan_id, block),
            100,
            "block must merge onto the high-numbered row, not duplicate"
        );
        assert_eq!(
            m.plans[&plan_id].row_name(scope, 1),
            None,
            "source row name cleared"
        );
    }

    #[test]
    fn reordered_row_insert_semantics() {
        // Moving row 0 down to 2: rows 1..=2 shift up one; outside untouched.
        assert_eq!(reordered_row(0, 0, 2), 2);
        assert_eq!(reordered_row(1, 0, 2), 0);
        assert_eq!(reordered_row(2, 0, 2), 1);
        assert_eq!(reordered_row(3, 0, 2), 3);
        // Moving row 3 up to 1: rows 1..=2 shift down one.
        assert_eq!(reordered_row(3, 3, 1), 1);
        assert_eq!(reordered_row(1, 3, 1), 2);
        assert_eq!(reordered_row(2, 3, 1), 3);
        assert_eq!(reordered_row(0, 3, 1), 0);
        assert_eq!(reordered_row(4, 3, 1), 4);
        // A no-move drag maps every row to itself.
        assert_eq!(reordered_row(5, 2, 2), 5);
        assert_eq!(reordered_row(2, 2, 2), 2);
    }

    #[test]
    fn reorder_row_names_moves_grows_and_trims() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Down-move permutes like reordered_row.
        let mut v = names(&["A", "B", "C"]);
        reorder_row_names(&mut v, 0, 2);
        assert_eq!(v, names(&["B", "C", "A"]));
        // Up-move.
        let mut v = names(&["A", "B", "C"]);
        reorder_row_names(&mut v, 2, 0);
        assert_eq!(v, names(&["C", "A", "B"]));
        // A short list grows so the drag is addressable, then trailing
        // empties are trimmed: only "A" moving to index 2 survives.
        let mut v = names(&["A"]);
        reorder_row_names(&mut v, 0, 2);
        assert_eq!(v, names(&["", "", "A"]));
        // Moving an unnamed high row up shifts names down and trims the tail.
        let mut v = names(&["A", "B"]);
        reorder_row_names(&mut v, 3, 0);
        assert_eq!(v, names(&["", "A", "B"]));
    }

    #[test]
    fn apply_row_reorder_moves_blocks_and_names_together() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::create_tables(&conn).unwrap();

        let mut m = model::Model::default();
        let plan_id = m.create_plan("main", None);
        let scope: Option<model::WorkBlockId> = None;

        let block_on_row = |m: &mut model::Model, name: &str, row: i32| {
            let id = m.create_work_block(name);
            m.work_blocks.get_mut(&id).unwrap().duration_days = 5;
            m.plans.get_mut(&plan_id).unwrap().root_blocks.push(id);
            m.set_block_row(plan_id, id, row);
            id
        };
        let a = block_on_row(&mut m, "A", 0);
        let b = block_on_row(&mut m, "B", 1);
        let c = block_on_row(&mut m, "C", 2);
        for (row, name) in [(0, "Ann"), (1, "Bob"), (2, "Cat")] {
            m.plans
                .get_mut(&plan_id)
                .unwrap()
                .set_row_name(scope, row, name.to_string());
        }
        // A child block (visible only when drilled into A) sits on row 1 of
        // its own scope and must not move with a top-level reorder.
        let child = m.create_work_block("A.1");
        m.work_blocks.get_mut(&child).unwrap().duration_days = 2;
        m.work_blocks.get_mut(&child).unwrap().parent = Some(a);
        m.set_block_row(plan_id, child, 1);
        db::save_model(&conn, &m).unwrap();

        // Drag Ann's row (0) below Cat's (2).
        apply_row_reorder(&mut m, &conn, plan_id, scope, 0, 2);

        assert_eq!(m.block_row(plan_id, a), 2, "dragged row's block lands at 2");
        assert_eq!(m.block_row(plan_id, b), 0, "rows between shift up");
        assert_eq!(m.block_row(plan_id, c), 1, "rows between shift up");
        assert_eq!(
            m.block_row(plan_id, child),
            1,
            "drilled-scope block is not visible at top level and stays put"
        );
        assert_eq!(m.plans[&plan_id].row_name(scope, 0), Some("Bob"));
        assert_eq!(m.plans[&plan_id].row_name(scope, 1), Some("Cat"));
        assert_eq!(m.plans[&plan_id].row_name(scope, 2), Some("Ann"));
    }

    #[test]
    fn editing_enabled_reflects_by_person() {
        let by_plan = ViewMode {
            by_person: false,
            plan: None,
        };
        assert!(!by_plan.by_person, "By-Plan: by_person is false");
        let by_person = ViewMode {
            by_person: true,
            plan: None,
        };
        assert!(by_person.by_person, "By-Person: by_person is true");
    }
}
