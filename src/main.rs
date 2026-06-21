use bevy::{
    core_pipeline::tonemapping::Tonemapping, post_process::bloom::Bloom, prelude::*,
    render::view::Hdr,
};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use chrono::Datelike;

pub mod analysis;
pub mod bands;
pub mod blocks;
pub mod calendar;
pub mod camera;
pub mod constants;
pub mod db;
pub mod graph;
pub mod labels;
pub mod model;
pub mod schedule;

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
        .insert_resource(ClearColor(Color::srgb(0.08, 0.07, 0.065)))
        .insert_resource(CameraTarget::default())
        .insert_resource(blocks::SelectedBlock::default())
        .insert_resource(blocks::SelectedDependency::default())
        .insert_resource(blocks::NameEditState::default())
        .insert_resource(blocks::DragState::default())
        .insert_resource(blocks::ResizeDragState::default())
        .insert_resource(blocks::DepDragState::default())
        .insert_resource(blocks::UndoStack::default())
        .insert_resource(blocks::CreateModeState::default())
        .insert_resource(blocks::SizePickerState::default())
        .insert_resource(schedule::VisibleBlocks::default())
        .insert_resource(schedule::DrillScope::default())
        .insert_resource(analysis::ScheduleAnalysis::default())
        .insert_resource(schedule::TodayMarker::default())
        .insert_resource(blocks::BlockSpriteMap::default())
        .insert_resource(blocks::ComparePlanState::default())
        .insert_resource(blocks::CompareBlockSpriteMap::default())
        .insert_resource(ForkHoverState::default())
        .insert_resource(SelectedPlan::default())
        .insert_resource(bands::BandEntities::default())
        .insert_resource(bands::PlanRenameState::default())
        .insert_resource(bands::LaneSelection::default())
        .insert_resource(bands::LaneDrag::default())
        .insert_resource(bands::LaneBlockRename::default())
        .insert_resource(bands::LaneDepDrag::default())
        .add_systems(Startup, (setup_db, setup_camera))
        .add_systems(Startup, setup_demo_schedule.after(setup_db))
        .add_systems(
            PostStartup,
            update_analysis.before(blocks::reconcile_block_sprites),
        )
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
            sync_period_bands.after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            PostStartup,
            labels::spawn_labels.after(blocks::reconcile_block_sprites),
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
            bands::handle_band_rename_click.run_if(at_plan_level),
        )
        .add_systems(
            Update,
            bands::handle_band_block_create
                .run_if(at_plan_level)
                .before(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            bands::handle_lane_dep_drag
                .run_if(at_plan_level)
                .before(bands::handle_lane_block_edit),
        )
        .add_systems(
            Update,
            bands::handle_lane_block_edit
                .run_if(at_plan_level)
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
        .add_systems(Update, handle_branch_delete.after(blocks::handle_block_drill))
        .add_systems(Update, schedule::update_today_marker)
        .add_systems(Update, sync_total_duration)
        .add_systems(Update, sync_weekend_bands.after(sync_total_duration))
        .add_systems(Update, sync_period_bands.after(sync_total_duration))
        .add_systems(Update, update_analysis)
        .add_systems(
            Update,
            schedule::update_visible_blocks
                .before(blocks::reconcile_block_sprites)
                .before(blocks::draw_dependency_edges)
                .before(blocks::draw_block_handles)
                .after(blocks::handle_block_delete)
                .after(blocks::handle_undo),
        )
        .add_systems(Update, blocks::handle_block_drill)
        .add_systems(Update, blocks::handle_drill_out)
        // Runs before the keyboard shortcut handlers so the first typed
        // character opens the rename instead of triggering F/N/S/Home.
        .add_systems(
            Update,
            blocks::handle_type_to_rename
                .before(camera_nav_keys)
                .before(blocks::handle_create_mode_toggle)
                .before(blocks::handle_size_picker_hotkey),
        )
        .add_systems(
            Update,
            blocks::handle_canvas_create.after(blocks::handle_block_drill),
        )
        .add_systems(
            Update,
            blocks::handle_block_delete.after(blocks::handle_block_drill),
        )
        .add_systems(Update, blocks::handle_undo)
        .add_systems(
            Update,
            blocks::handle_create_mode_toggle.after(blocks::handle_block_drill),
        )
        .add_systems(Update, blocks::handle_create_mode_click_exit)
        .add_systems(Update, blocks::handle_size_picker_hotkey)
        .add_systems(
            Update,
            blocks::handle_block_selection.after(blocks::handle_block_drill),
        )
        .add_systems(
            Update,
            blocks::handle_block_resize.after(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            blocks::handle_block_drag
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
                .before(blocks::handle_block_selection)
                .before(blocks::handle_block_drag)
                .before(blocks::handle_block_resize),
        )
        .add_systems(Update, blocks::draw_block_handles)
        .add_systems(Update, blocks::update_cursor_icon)
        .add_systems(Update, blocks::draw_dependency_edges.after(update_analysis))
        .add_systems(
            Update,
            labels::spawn_labels
                .after(blocks::handle_block_selection)
                .after(blocks::reconcile_block_sprites),
        )
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
        .add_systems(EguiPrimaryContextPass, blocks::draw_name_edit_overlay)
        .add_systems(EguiPrimaryContextPass, blocks::draw_create_mode_overlay)
        .add_systems(EguiPrimaryContextPass, blocks::draw_block_tooltip)
        .add_systems(EguiPrimaryContextPass, blocks::draw_size_picker_popup)
        .add_systems(EguiPrimaryContextPass, blocks::draw_size_settings_popup)
        .add_systems(EguiPrimaryContextPass, bands::draw_plan_rename_overlay)
        .add_systems(EguiPrimaryContextPass, bands::draw_lane_block_rename_overlay)
        .run();
}

fn setup_db(world: &mut World) {
    let conn = rusqlite::Connection::open("brick_road.db").expect("failed to open brick_road.db");
    db::create_tables(&conn).expect("failed to create DB tables");
    let model = db::load_model(&conn).expect("failed to load model");
    world.insert_resource(model);
    world.insert_non_send_resource(conn);
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

/// On a drill-in/out change, reframe the camera: drilling into a block frames
/// that block's span (with slack to place children beyond it); drilling back to
/// the plan level fits the plan's blocks (or returns to the today/home view).
fn frame_on_drill(
    drill: Res<schedule::DrillScope>,
    model: Res<model::Model>,
    schedule: Res<schedule::Schedule>,
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
        Some(wb) => camera::frame_day_span(window, wb.start_day, wb.start_day + wb.duration_days),
        None => camera::fit_to_blocks(&model, schedule.plan_id, &windows)
            .unwrap_or_else(|| camera::home_target(window, today.day)),
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
    let Ok((cam_t, proj)) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let Ok(window) = windows.single() else { return };
    let half_h = (window.height() * 0.5 * ortho.scale).max(800.0);
    let y_top = cam_t.translation.y + half_h;
    let y_bot = cam_t.translation.y - half_h;
    let color = Color::from(LinearRgba::new(2.4, 1.6, 0.3, 0.5)); // amber, bloomed

    for day in [wb.start_day, wb.start_day + wb.duration_days] {
        let x = day as f32 * PIXELS_PER_DAY;
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
    windows: Query<&Window>,
    mut cam: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    if *done {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let home = camera::home_target(window, today.day);
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

    let day_min = (x_left / PIXELS_PER_DAY).floor() as i32;
    let day_max = (x_right / PIXELS_PER_DAY).ceil() as i32;

    for day in day_min..=day_max {
        let x = day as f32 * PIXELS_PER_DAY;
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
    let x_today = today.day as f32 * PIXELS_PER_DAY;
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

/// Returns `(x_world_position, is_holiday)` for each non-working day band within
/// the given span.
///
/// Week-boundary bands appear every `working_days_per_week` days (`is_holiday = false`).
/// Calendar holiday bands are placed at the next working-day boundary after each
/// date in `non_working_dates` (`is_holiday = true`).
fn weekend_band_positions(span_days: i32, model: &model::Model) -> Vec<(f32, bool)> {
    let mut positions = Vec::new();
    let wdpw = model.calendar.working_days_per_week as i32;

    let mut day = wdpw;
    while day <= span_days + wdpw {
        positions.push((day as f32 * PIXELS_PER_DAY, false));
        day += wdpw;
    }

    for &holiday in &model.calendar.non_working_dates {
        // date_to_day for a non-working day returns the last working-day count
        // before it; +1 gives the next working day's index, which is the correct
        // band position (immediately after the holiday gap).
        let boundary = calendar::date_to_day(holiday, &model.calendar) + 1;
        if boundary >= 0 && boundary <= span_days + 10 {
            positions.push((boundary as f32 * PIXELS_PER_DAY, true));
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
    let weekend_color = Color::srgba(0.22, 0.26, 0.42, 0.09);
    let holiday_color = Color::srgba(0.72, 0.28, 0.28, 0.11);

    for (x, is_holiday) in weekend_band_positions(span, &model) {
        let color = if is_holiday {
            holiday_color
        } else {
            weekend_color
        };
        commands.spawn((
            WeekendBand,
            Sprite {
                color,
                custom_size: Some(Vec2::new(8.0, 20_000.0)),
                ..default()
            },
            Transform::from_xyz(x, 0.0, -0.5),
        ));
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
    let span_px = span_days as f32 * PIXELS_PER_DAY;

    let start_year = config.start_date.year();
    let start_month = config.start_date.month();

    let mut year = start_year;
    let mut month = start_month;

    loop {
        let x_start = match calendar::first_working_day_of_month(year, month, config) {
            Some(d) => (calendar::date_to_day(d, config) as f32 * PIXELS_PER_DAY).max(0.0),
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
            Some(d) => (calendar::date_to_day(d, config) as f32 * PIXELS_PER_DAY).min(span_px),
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
        Some(d) => (calendar::date_to_day(d, config) as f32 * PIXELS_PER_DAY).max(0.0),
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
        // Default the active plan to the lowest-id root plan (forks sort last
        // via `branch_start_day.is_some()`). Picking an arbitrary
        // `values().next()` could make a fork active — and the active plan can't
        // be deleted, so a randomly-active branch would be impossible to remove.
        let default_plan = model
            .plans
            .values()
            .min_by_key(|p| (p.branch_start_day.is_some(), p.id.0))
            .cloned();
        if let Some(plan) = default_plan {
            let graph = graph::build_graph(&model, &plan);
            if let Ok(sched) = schedule::forward_pass(&model, &plan, &graph) {
                commands.insert_resource(sched);
            } else {
                commands.insert_resource(schedule::Schedule::new(plan.id));
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
    if let Ok(sched) = schedule::forward_pass(&model, &plan, &dep_graph) {
        for sb in sched.blocks.values() {
            if let Some(wb) = model.work_blocks.get_mut(&sb.work_block_id) {
                wb.start_day = sb.start_day;
                wb.duration_days = sb.duration_days;
            }
        }
        commands.insert_resource(sched);
    }
}

fn update_analysis(model: Res<model::Model>, mut sa: ResMut<analysis::ScheduleAnalysis>) {
    if !model.is_changed() {
        return;
    }
    let dep = analysis::analyze_dependencies(&model);
    let (critical_path, float) = model
        .plans
        .values()
        .next()
        .and_then(|plan| {
            let graph = graph::build_graph(&model, plan);
            schedule::analyze_user_placement(&model, &graph).ok()
        })
        .map(|cpa| (cpa.critical_path, cpa.float))
        .unwrap_or_default();

    let resource_conflicts = model
        .plans
        .values()
        .next()
        .map(|plan| analysis::analyze_resources(&model, plan))
        .unwrap_or_default();

    *sa = analysis::ScheduleAnalysis {
        violations: dep.violations,
        resource_conflicts,
        critical_path,
        float,
    };
}

/// Tracks mouse position over the timeline and updates `ForkHoverState`.
/// On left-click, creates a new plan that branches from the hovered day.
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

    fork.hovered_day = world_x.map(|x| (x / PIXELS_PER_DAY).floor() as model::Day);

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
    schedule: Res<schedule::Schedule>,
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

    let active_id = schedule.plan_id;

    // Branch-point markers for non-active plans.
    let mut branch_plans: Vec<&model::Plan> = model
        .plans
        .values()
        .filter(|p| p.id != active_id && p.branch_start_day.is_some())
        .collect();
    branch_plans.sort_by_key(|p| p.id.0);

    for (idx, plan) in branch_plans.iter().enumerate() {
        let Some(branch_day) = plan.branch_start_day else {
            continue;
        };
        let x = branch_day as f32 * PIXELS_PER_DAY;
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
        let x = hovered_day as f32 * PIXELS_PER_DAY;
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
    let wdpw = (config.working_days_per_week as i32).max(1);
    // Working-day → date helpers and the on-screen size of one day.
    let day_w = PIXELS_PER_DAY / scale; // screen px per day
    let week_w = wdpw as f32 * day_w;
    let show_days = day_w >= 13.0;
    let show_weeks = week_w >= 44.0;

    let day_min = (x_left / PIXELS_PER_DAY).floor() as i32;
    let day_max = (x_right / PIXELS_PER_DAY).ceil() as i32;

    egui::TopBottomPanel::top("calendar_ruler")
        .exact_height(64.0)
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(22, 17, 12))
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
            let day_x = |d: i32| date_to_day_x(d);

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
                    period(qs, qe, &format!("Q{} '{:02}", q + 1, y % 100), quarter_y, 11.0, quarter_color);
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
                    period(day_x(ws), day_x(ws + wdpw), &label, week_y, 10.5, week_color);
                    // Week boundary tick.
                    let sx = world_to_screen_x(day_x(ws));
                    painter.line_segment(
                        [egui::Pos2::new(sx, week_y - 5.0), egui::Pos2::new(sx, rect.bottom())],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 74, 92)),
                    );
                }
            }

            // Tier 4: Day numbers (1, 2, 3 …) centered in each day cell.
            if show_days {
                let tick = egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 56, 72));
                for d in day_min..=day_max {
                    let bx = world_to_screen_x(day_x(d));
                    painter.line_segment(
                        [egui::Pos2::new(bx, day_y - 6.0), egui::Pos2::new(bx, rect.bottom())],
                        tick,
                    );
                    let cx = world_to_screen_x((d as f32 + 0.5) * PIXELS_PER_DAY);
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
            }

            // Today marker tick — warm accent, matching the canvas today line.
            let today_x = world_to_screen_x(today.day as f32 * PIXELS_PER_DAY);
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

/// World-space x of the left edge of working day `d`.
fn date_to_day_x(d: i32) -> f32 {
    d as f32 * PIXELS_PER_DAY
}

/// World-space x of the start of calendar year `y` (its Jan 1, mapped to a
/// working day). Used for the year tier of the calendar ruler.
fn year_start_x(y: i32, config: &model::CalendarConfig) -> f32 {
    let date = chrono::NaiveDate::from_ymd_opt(y, 1, 1)
        .unwrap_or_else(|| config.start_date);
    calendar::date_to_day(date, config) as f32 * PIXELS_PER_DAY
}

/// World-space x of the start of quarter `q` (0..=4, where 4 = next year's Q1)
/// in calendar year `y`.
fn quarter_start_x(y: i32, q: i32, config: &model::CalendarConfig) -> f32 {
    let (yy, month) = if q >= 4 { (y + 1, 1) } else { (y, (q * 3 + 1) as u32) };
    let date = chrono::NaiveDate::from_ymd_opt(yy, month, 1)
        .unwrap_or_else(|| config.start_date);
    calendar::date_to_day(date, config) as f32 * PIXELS_PER_DAY
}

/// Renders Re-center and Fit-to-view buttons in a small floating area
/// anchored to the top-right of the window. Keyboard shortcuts (Home / F)
/// are handled by `camera_nav_keys` in `camera.rs`.
/// Renders a fixed top bar containing the brand logo and camera/view navigation
/// buttons. Using TopBottomPanel reserves space so block labels and side panel
/// content cannot render behind the controls.
#[allow(clippy::too_many_arguments)]
fn top_bar_ui(
    mut contexts: EguiContexts,
    mut target: ResMut<CameraTarget>,
    mut model: ResMut<model::Model>,
    mut schedule: ResMut<schedule::Schedule>,
    mut drill: ResMut<schedule::DrillScope>,
    windows: Query<&Window>,
    today: Res<schedule::TodayMarker>,
    conn: NonSend<rusqlite::Connection>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut clear_all = false;
    // Breadcrumb path (block ids) to optionally truncate to, and a rollup toggle.
    let mut jump_to: Option<usize> = None; // new path length
    let mut toggle_rollup: Option<model::WorkBlockId> = None;
    egui::TopBottomPanel::top("top_bar")
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(18, 12, 4, 230))
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let text = egui::RichText::new("brick_road")
                    .size(18.0)
                    .color(egui::Color32::from_rgb(250, 165, 40));
                let btn = egui::Button::new(text)
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE);
                if ui.add(btn).on_hover_text("Fit to view [F]").clicked() {
                    if let Some(new_target) = camera::fit_to_blocks(&model, schedule.plan_id, &windows) {
                        *target = new_target;
                    }
                }

                // Drill-in breadcrumb: Plan / Block / Block… Click a crumb to
                // jump out to that level. A roll-up toggle for the current block.
                if !drill.path.is_empty() {
                    ui.separator();
                    if ui
                        .selectable_label(false, egui::RichText::new("Plan").color(
                            egui::Color32::from_rgb(196, 162, 110),
                        ))
                        .clicked()
                    {
                        jump_to = Some(0);
                    }
                    for (i, id) in drill.path.iter().enumerate() {
                        ui.label(egui::RichText::new("/").color(egui::Color32::from_gray(110)));
                        let name = model
                            .work_blocks
                            .get(id)
                            .map(|wb| wb.name.clone())
                            .unwrap_or_else(|| "?".to_string());
                        let is_last = i + 1 == drill.path.len();
                        let text = egui::RichText::new(name).color(if is_last {
                            egui::Color32::from_rgb(232, 234, 244)
                        } else {
                            egui::Color32::from_rgb(196, 162, 110)
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

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("→ Today").clicked() {
                        let x = today.day as f32 * PIXELS_PER_DAY;
                        target.pos.x = x;
                    }
                    if ui.small_button("Fit to view [F]").clicked() {
                        if let Some(new_target) = camera::fit_to_blocks(&model, schedule.plan_id, &windows) {
                            *target = new_target;
                        }
                    }
                    if ui.small_button("Re-center [Home]").clicked() {
                        if let Ok(window) = windows.single() {
                            *target = camera::home_target(window, today.day);
                        }
                    }
                    // Dev: wipe all blocks, branches, and links; keep one empty
                    // main plan to start fresh from.
                    if ui
                        .small_button(
                            egui::RichText::new("⌫ Clear all")
                                .color(egui::Color32::from_rgb(230, 120, 120)),
                        )
                        .on_hover_text("Dev: delete all blocks, branches, and links")
                        .clicked()
                    {
                        clear_all = true;
                    }
                });
            });
        });

    if clear_all {
        let main_id = model.clear_all_work();
        *schedule = schedule::Schedule::new(main_id);
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

/// The branch (forked plan) whose marker is currently selected, if any.
/// Selecting a branch by clicking its marker arms the Delete key to remove it.
#[derive(Resource, Default)]
pub struct SelectedPlan(pub Option<model::PlanId>);

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
    for plan in model.plans.values() {
        if plan.id == active_id {
            continue;
        }
        let Some(day) = plan.branch_start_day else {
            continue;
        };
        let dist = (world_x - day as f32 * PIXELS_PER_DAY).abs();
        if dist <= hit_world && best.is_none_or(|(bd, _)| dist < bd) {
            best = Some((dist, plan.id));
        }
    }
    best.map(|(_, id)| id)
}

/// Left-click on (or very near) a branch marker selects that branch; the Delete
/// key then removes it. Selecting a branch clears any block/dependency
/// selection so Delete is unambiguous.
fn handle_branch_selection(
    mut egui_ctx: EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    cam_proj: Query<&Projection, With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    model: Res<model::Model>,
    schedule: Res<schedule::Schedule>,
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
        .and_then(|p| match p {
            Projection::Orthographic(o) => Some(o.scale),
            _ => None,
        })
        .unwrap_or(1.0);

    // ~6 screen pixels of grab tolerance on either side of the marker line.
    if let Some(id) = branch_plan_at_x(&model, schedule.plan_id, world.x, 6.0 * scale) {
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
    fn week_bands_at_every_wdpw_boundary() {
        let model = model::Model::default();
        let positions = weekend_band_positions(10, &model);
        let xs: Vec<f32> = positions
            .iter()
            .filter(|(_, h)| !h)
            .map(|(x, _)| *x)
            .collect();
        // Default wdpw=5; bands at day 5, 10, 15 (span + wdpw).
        assert!(xs.contains(&(5.0 * PIXELS_PER_DAY)));
        assert!(xs.contains(&(10.0 * PIXELS_PER_DAY)));
        assert!(xs.contains(&(15.0 * PIXELS_PER_DAY)));
    }

    #[test]
    fn no_holiday_bands_without_non_working_dates() {
        let model = model::Model::default();
        let positions = weekend_band_positions(10, &model);
        assert_eq!(positions.iter().filter(|(_, h)| *h).count(), 0);
    }

    #[test]
    fn holiday_band_placed_at_next_working_day_boundary() {
        let mut model = model::Model::default();
        model.calendar.start_date = NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(); // Monday
        model.calendar.non_working_dates = vec![NaiveDate::from_ymd_opt(2025, 1, 7).unwrap()]; // Tuesday
        let positions = weekend_band_positions(20, &model);
        let holiday_xs: Vec<f32> = positions
            .iter()
            .filter(|(_, h)| *h)
            .map(|(x, _)| *x)
            .collect();
        // date_to_day(Tue Jan 7 holiday, Mon Jan 6 start) = 0 → boundary = 1 → x = 100.0
        assert!(holiday_xs.contains(&(1.0 * PIXELS_PER_DAY)));
    }

    #[test]
    fn holiday_out_of_span_excluded() {
        let mut model = model::Model::default();
        model.calendar.start_date = NaiveDate::from_ymd_opt(2025, 1, 6).unwrap();
        // Holiday 200 working days out — far beyond span=5.
        model.calendar.non_working_dates = vec![calendar::day_to_date(200, &model.calendar)];
        let positions = weekend_band_positions(5, &model);
        assert_eq!(positions.iter().filter(|(_, h)| *h).count(), 0);
    }

    #[test]
    fn period_bands_start_jan_produces_bands() {
        let mut cfg = model::CalendarConfig::default();
        cfg.start_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
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
        let mut cfg = model::CalendarConfig::default();
        cfg.start_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let bands = period_band_spans(&cfg, 25);
        // Jan is Q1, first month → Q1 tint at full QUARTER_TINT_ALPHA.
        let (_, _, color) = bands[0];
        assert_eq!([color[0], color[1], color[2]], QUARTER_TINTS[0]);
        assert!((color[3] - QUARTER_TINT_ALPHA).abs() < 1e-5);
    }

    #[test]
    fn period_bands_alternating_alpha() {
        let mut cfg = model::CalendarConfig::default();
        cfg.start_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
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
}
