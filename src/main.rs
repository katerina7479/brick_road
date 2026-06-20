use bevy::{
    core_pipeline::tonemapping::Tonemapping, post_process::bloom::Bloom, prelude::*,
    render::view::Hdr,
};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use chrono::Datelike;

pub mod analysis;
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
        .insert_resource(analysis::ScheduleAnalysis::default())
        .insert_resource(schedule::TodayMarker::default())
        .insert_resource(blocks::BlockSpriteMap::default())
        .insert_resource(blocks::ComparePlanState::default())
        .insert_resource(blocks::CompareBlockSpriteMap::default())
        .insert_resource(ForkHoverState::default())
        .insert_resource(SelectedPlan::default())
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
            (camera_nav_keys, update_camera_target, smooth_camera).chain(),
        )
        .add_systems(Update, draw_grid)
        .add_systems(Update, draw_branch_markers)
        .add_systems(Update, handle_fork_hover)
        .add_systems(
            Update,
            handle_branch_selection.before(blocks::handle_block_selection),
        )
        .add_systems(Update, handle_branch_delete.after(blocks::handle_name_edit))
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
        .add_systems(Update, blocks::handle_name_edit)
        .add_systems(
            Update,
            blocks::handle_block_delete.after(blocks::handle_name_edit),
        )
        .add_systems(Update, blocks::handle_undo)
        .add_systems(
            Update,
            blocks::handle_create_mode_toggle.after(blocks::handle_name_edit),
        )
        .add_systems(Update, blocks::handle_create_mode_click_exit)
        .add_systems(Update, blocks::handle_size_picker_hotkey)
        .add_systems(
            Update,
            blocks::handle_block_selection.after(blocks::handle_name_edit),
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
    mut schedule: ResMut<schedule::Schedule>,
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

    // Ctrl+Left-click: fork the active plan from the hovered day.
    let ctrl = keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    if ctrl && mouse.just_pressed(MouseButton::Left) {
        if let Some(fork_day) = fork.hovered_day {
            let active_id = schedule.plan_id;
            if let Some(active_plan) = model.plans.get(&active_id).cloned() {
                let n = model.plans.len() + 1;
                let new_id = model.create_plan(format!("Plan {n}"), Some(fork_day.max(0)));
                // Copy root blocks from the active plan.
                if let Some(new_plan) = model.plans.get_mut(&new_id) {
                    new_plan.root_blocks = active_plan.root_blocks.clone();
                }
                *schedule = schedule::Schedule::new(active_id);
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
    schedule: Res<schedule::Schedule>,
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

    egui::TopBottomPanel::top("calendar_ruler")
        .exact_height(38.0)
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(22, 17, 12))
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            let painter = ui.painter_at(rect);

            let quarter_y = rect.top() + 9.0;
            let tick_top = rect.top() + 19.0;
            let tick_bottom = rect.bottom() - 2.0;
            let day_label_y = rect.top() + 28.0;

            // Quarter labels — clamped to the visible portion of their span so the
            // label stays readable while the quarter scrolls (sticky-header feel).
            for span in labels::quarter_label_spans(&schedule, &model) {
                let vis_start = span.world_x_start.max(x_left);
                let vis_end = span.world_x_end.min(x_right);
                if vis_end <= vis_start {
                    continue;
                }
                let cx = world_to_screen_x((vis_start + vis_end) * 0.5);
                painter.text(
                    egui::Pos2::new(cx, quarter_y),
                    egui::Align2::CENTER_CENTER,
                    span.label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_rgb(196, 162, 110),
                );
            }

            // Day ticks + date labels.
            let tick_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 74, 92));
            for tick in labels::day_tick_labels(scale, x_left, x_right, &model, today.day) {
                let sx = world_to_screen_x(tick.world_x);
                painter.line_segment(
                    [
                        egui::Pos2::new(sx, tick_top),
                        egui::Pos2::new(sx, tick_bottom),
                    ],
                    tick_stroke,
                );
                let color = if tick.is_past {
                    egui::Color32::from_rgb(120, 120, 140)
                } else {
                    egui::Color32::from_rgb(212, 214, 228)
                };
                painter.text(
                    egui::Pos2::new(sx, day_label_y),
                    egui::Align2::CENTER_CENTER,
                    tick.label,
                    egui::FontId::proportional(12.0),
                    color,
                );
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

/// Renders Re-center and Fit-to-view buttons in a small floating area
/// anchored to the top-right of the window. Keyboard shortcuts (Home / F)
/// are handled by `camera_nav_keys` in `camera.rs`.
/// Renders a fixed top bar containing the brand logo and camera/view navigation
/// buttons. Using TopBottomPanel reserves space so block labels and side panel
/// content cannot render behind the controls.
fn top_bar_ui(
    mut contexts: EguiContexts,
    mut target: ResMut<CameraTarget>,
    model: Res<model::Model>,
    windows: Query<&Window>,
    today: Res<schedule::TodayMarker>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
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
                    if let Some(new_target) = camera::fit_to_blocks(&model, &windows) {
                        *target = new_target;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("→ Today").clicked() {
                        let x = today.day as f32 * PIXELS_PER_DAY;
                        target.pos.x = x;
                    }
                    if ui.small_button("Fit to view [F]").clicked() {
                        if let Some(new_target) = camera::fit_to_blocks(&model, &windows) {
                            *target = new_target;
                        }
                    }
                    if ui.small_button("Re-center [Home]").clicked() {
                        if let Ok(window) = windows.single() {
                            *target = camera::home_target(window);
                        }
                    }
                });
            });
        });
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
        delete_plan(&mut model, id);
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
}

/// Removes a forked plan and any work blocks it solely owned. A fork copies the
/// parent's `root_blocks` (the same block ids), so blocks still rooted by
/// another plan are left intact — only blocks orphaned by the removal are
/// deleted.
fn delete_plan(model: &mut model::Model, plan_id: model::PlanId) {
    let Some(plan) = model.plans.remove(&plan_id) else {
        return;
    };
    for block in plan.root_blocks {
        let still_rooted = model.plans.values().any(|p| p.root_blocks.contains(&block));
        if !still_rooted {
            blocks::delete_work_block(model, block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn delete_plan_keeps_shared_blocks_removes_exclusive() {
        let mut m = model::Model::default();
        let root = m.create_plan("root", None);
        let shared = m.create_work_block("shared");
        m.plans.get_mut(&root).unwrap().root_blocks.push(shared);

        // A fork copies the parent's root_blocks (same `shared` id) and gains
        // its own exclusive block.
        let fork = m.create_plan("fork", Some(0));
        let exclusive = m.create_work_block("exclusive");
        m.plans.get_mut(&fork).unwrap().root_blocks = vec![shared, exclusive];

        delete_plan(&mut m, fork);

        assert!(!m.plans.contains_key(&fork), "fork plan removed");
        assert!(
            m.work_blocks.contains_key(&shared),
            "block shared with root is kept"
        );
        assert!(
            !m.work_blocks.contains_key(&exclusive),
            "block only the fork owned is removed"
        );
        assert!(m.plans.contains_key(&root), "root plan untouched");
    }

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
