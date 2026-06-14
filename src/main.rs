use bevy::{
    core_pipeline::tonemapping::Tonemapping, post_process::bloom::Bloom, prelude::*,
    render::view::Hdr,
};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use chrono::NaiveDate;

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
use constants::{PIXELS_PER_DAY, SIDE_PANEL_WIDTH};

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
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.05)))
        .insert_resource(CameraTarget::default())
        .insert_resource(blocks::SelectedBlock::default())
        .insert_resource(blocks::NameEditState::default())
        .insert_resource(blocks::DragState::default())
        .insert_resource(blocks::ResizeDragState::default())
        .insert_resource(blocks::DepDragState::default())
        .insert_resource(blocks::DeleteConfirmState::default())
        .insert_resource(blocks::CreateModeState::default())
        .insert_resource(schedule::ViewScope::default())
        .insert_resource(schedule::TimelineViewMode::default())
        .insert_resource(schedule::VisibleBlocks::default())
        .insert_resource(analysis::ScheduleAnalysis::default())
        .insert_resource(blocks::BlockSpriteMap::default())
        .insert_resource(labels::NestingDepthMap::default())
        .add_systems(Startup, (setup_db, setup_camera))
        .add_systems(Startup, setup_demo_schedule.after(setup_db))
        .add_systems(PostStartup, update_analysis.before(blocks::reconcile_block_sprites))
        .add_systems(
            PostStartup,
            labels::compute_nesting_depths.before(blocks::reconcile_block_sprites),
        )
        .add_systems(
            PostStartup,
            schedule::update_visible_blocks.before(blocks::reconcile_block_sprites),
        )
        .add_systems(PostStartup, blocks::reconcile_block_sprites)
        .add_systems(
            PostStartup,
            labels::spawn_labels.after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            PostStartup,
            labels::spawn_day_labels.after(labels::spawn_labels),
        )
        .add_systems(Update, (camera_nav_keys, update_camera_target, smooth_camera).chain())
        .add_systems(Update, draw_grid)
        .add_systems(Update, update_analysis)
        .add_systems(
            Update,
            schedule::update_visible_blocks
                .before(blocks::reconcile_block_sprites)
                .before(blocks::sync_conflict_overlays)
                .before(blocks::sync_uncertainty_overlays)
                .before(blocks::draw_dependency_edges)
                .before(blocks::draw_block_handles),
        )
        .add_systems(Update, blocks::handle_name_edit)
        .add_systems(
            Update,
            blocks::handle_block_delete.after(blocks::handle_name_edit),
        )
        .add_systems(
            Update,
            blocks::handle_create_mode_toggle.after(blocks::handle_name_edit),
        )
        .add_systems(Update, blocks::handle_create_mode_click_exit)
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
                .after(blocks::reconcile_block_sprites)
                .run_if(task_view_active),
        )
        .add_systems(
            Update,
            blocks::sync_conflict_overlays
                .after(update_analysis)
                .run_if(task_view_active),
        )
        .add_systems(
            Update,
            blocks::draw_block_borders
                .after(blocks::sync_block_sprites)
                .run_if(task_view_active),
        )
        .add_systems(
            Update,
            blocks::sync_uncertainty_overlays
                .after(blocks::reconcile_block_sprites)
                .run_if(task_view_active),
        )
        .add_systems(
            Update,
            blocks::handle_dep_drag
                .before(blocks::handle_block_selection)
                .before(blocks::handle_block_drag)
                .before(blocks::handle_block_resize),
        )
        .add_systems(Update, blocks::draw_block_handles.run_if(task_view_active))
        .add_systems(
            Update,
            blocks::draw_dependency_edges
                .after(update_analysis)
                .run_if(task_view_active),
        )
        .add_systems(
            Update,
            labels::spawn_labels
                .after(blocks::handle_block_selection)
                .after(blocks::reconcile_block_sprites),
        )
        .add_systems(
            Update,
            labels::spawn_day_labels
                .after(update_camera_target)
                .after(smooth_camera),
        )
        .add_systems(
            Update,
            labels::compute_nesting_depths.before(labels::draw_nesting_indicators),
        )
        .add_systems(Update, labels::draw_nesting_indicators.run_if(task_view_active))
        .add_systems(Update, labels::draw_violation_indicators.run_if(task_view_active))
        .add_systems(Update, labels::scale_labels_to_zoom)
        .add_systems(
            Update,
            blocks::sync_block_labels
                .after(blocks::reconcile_block_sprites)
                .run_if(task_view_active),
        )
        .add_systems(
            Update,
            blocks::sync_block_label_names
                .after(blocks::reconcile_block_sprites)
                .before(blocks::sync_block_labels)
                .run_if(task_view_active),
        )
        .add_systems(
            Update,
            blocks::sync_description_dots
                .after(blocks::reconcile_block_sprites)
                .run_if(task_view_active),
        )
        .add_systems(Update, draw_resource_timeline)
        .add_systems(EguiPrimaryContextPass, side_panel_ui)
        .add_systems(EguiPrimaryContextPass, camera_nav_ui)
        .add_systems(EguiPrimaryContextPass, logo_ui)
        .add_systems(EguiPrimaryContextPass, resource_row_labels_ui)
        .add_systems(EguiPrimaryContextPass, blocks::draw_name_edit_overlay)
        .add_systems(EguiPrimaryContextPass, blocks::draw_delete_confirm_overlay)
        .add_systems(EguiPrimaryContextPass, blocks::draw_create_mode_overlay)
        .add_systems(EguiPrimaryContextPass, blocks::draw_description_tooltip)
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
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let line_color = Color::srgba(0.3, 0.3, 0.5, 0.15);
    let baseline_color = Color::srgba(0.4, 0.4, 0.6, 0.35);

    let Ok((cam_t, proj)) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let Ok(window) = windows.single() else { return };

    let scale = ortho.scale;
    let cam_x = cam_t.translation.x;
    let cam_y = cam_t.translation.y;

    // Visible world-space extents with a one-day margin to avoid edge pop-in.
    let half_w = (window.width() * 0.5 + PIXELS_PER_DAY) * scale;
    let half_h = (window.height() * 0.5 + 100.0) * scale;

    let x_left   = cam_x - half_w;
    let x_right  = cam_x + half_w;
    let y_bottom = cam_y - half_h;
    let y_top    = cam_y + half_h;

    let day_min = (x_left / PIXELS_PER_DAY).floor() as i32;
    let day_max = (x_right / PIXELS_PER_DAY).ceil() as i32;

    for day in day_min..=day_max {
        let x = day as f32 * PIXELS_PER_DAY;
        gizmos.line_2d(Vec2::new(x, y_bottom), Vec2::new(x, y_top), line_color);
    }

    gizmos.line_2d(Vec2::new(x_left, 0.0), Vec2::new(x_right, 0.0), baseline_color);
}

fn task_view_active(mode: Res<schedule::TimelineViewMode>) -> bool {
    *mode == schedule::TimelineViewMode::Task
}

/// Draws one row per resource block when `TimelineViewMode::Resource` is active.
/// Each row shows bars for every work block allocated to that resource, coloured
/// red if the window overlaps a detected resource conflict.
fn draw_resource_timeline(
    mode: Res<schedule::TimelineViewMode>,
    model: Res<model::Model>,
    schedule: Res<schedule::Schedule>,
    sa: Res<analysis::ScheduleAnalysis>,
    mut gizmos: Gizmos,
) {
    if *mode != schedule::TimelineViewMode::Resource {
        return;
    }

    let Some(plan) = model.plans.get(&schedule.plan_id) else { return };

    let mut resources: Vec<_> = model.resource_blocks.values().collect();
    resources.sort_by_key(|r| r.id.0);

    // Pre-index conflict windows per resource.
    let mut conflict_windows: std::collections::HashMap<model::ResourceBlockId, Vec<(f32, f32)>> =
        std::collections::HashMap::new();
    for c in &sa.resource_conflicts {
        conflict_windows
            .entry(c.resource_id)
            .or_default()
            .push((c.window_start, c.window_end));
    }

    let row_sep = Color::srgba(0.3, 0.3, 0.5, 0.2);
    let alloc_ok = Color::srgba(0.2, 1.8, 0.6, 0.7);
    let alloc_conflict = Color::srgba(2.5, 0.3, 0.3, 0.9);
    let bar_h = constants::ROW_HEIGHT * 0.65;

    for (row, resource) in resources.iter().enumerate() {
        let y = -(row as f32) * constants::ROW_HEIGHT;

        // Row separator.
        gizmos.line_2d(
            Vec2::new(-50_000.0, y - constants::ROW_HEIGHT * 0.5),
            Vec2::new(50_000.0,  y - constants::ROW_HEIGHT * 0.5),
            row_sep,
        );

        let conflicts = conflict_windows.get(&resource.id);

        for alloc in &plan.allocations {
            if alloc.resource_id != resource.id {
                continue;
            }
            let Some(wb) = model.work_blocks.get(&alloc.work_block_id) else { continue };

            let x0 = wb.start_day * PIXELS_PER_DAY;
            let x1 = (wb.start_day + wb.duration_days) * PIXELS_PER_DAY;
            let w  = (x1 - x0).max(4.0);
            let cx = x0 + w * 0.5;

            let is_conflicted = conflicts.is_some_and(|cws| {
                cws.iter().any(|&(cs, ce)| {
                    wb.start_day < ce && (wb.start_day + wb.duration_days) > cs
                })
            });

            let color = if is_conflicted { alloc_conflict } else { alloc_ok };
            let (x_lo, x_hi) = (cx - w * 0.5, cx + w * 0.5);
            let (y_lo, y_hi) = (y - bar_h * 0.5, y + bar_h * 0.5);
            gizmos.line_2d(Vec2::new(x_lo, y_lo), Vec2::new(x_hi, y_lo), color);
            gizmos.line_2d(Vec2::new(x_hi, y_lo), Vec2::new(x_hi, y_hi), color);
            gizmos.line_2d(Vec2::new(x_hi, y_hi), Vec2::new(x_lo, y_hi), color);
            gizmos.line_2d(Vec2::new(x_lo, y_hi), Vec2::new(x_lo, y_lo), color);
        }
    }

    // Unassigned row: placed blocks with no allocation in the current plan.
    let allocated: std::collections::HashSet<model::WorkBlockId> =
        plan.allocations.iter().map(|a| a.work_block_id).collect();
    let unassigned: Vec<_> = model
        .work_blocks
        .values()
        .filter(|wb| wb.duration_days > 0.0 && !allocated.contains(&wb.id))
        .collect();

    if !unassigned.is_empty() {
        let row = resources.len();
        let y = -(row as f32) * constants::ROW_HEIGHT;

        gizmos.line_2d(
            Vec2::new(-50_000.0, y - constants::ROW_HEIGHT * 0.5),
            Vec2::new(50_000.0,  y - constants::ROW_HEIGHT * 0.5),
            row_sep,
        );

        let unassigned_color = Color::srgba(0.55, 0.55, 0.55, 0.5);
        for wb in &unassigned {
            let x0 = wb.start_day * PIXELS_PER_DAY;
            let x1 = (wb.start_day + wb.duration_days) * PIXELS_PER_DAY;
            let w  = (x1 - x0).max(4.0);
            let cx = x0 + w * 0.5;
            let (x_lo, x_hi) = (cx - w * 0.5, cx + w * 0.5);
            let (y_lo, y_hi) = (y - bar_h * 0.5, y + bar_h * 0.5);
            gizmos.line_2d(Vec2::new(x_lo, y_lo), Vec2::new(x_hi, y_lo), unassigned_color);
            gizmos.line_2d(Vec2::new(x_hi, y_lo), Vec2::new(x_hi, y_hi), unassigned_color);
            gizmos.line_2d(Vec2::new(x_hi, y_hi), Vec2::new(x_lo, y_hi), unassigned_color);
            gizmos.line_2d(Vec2::new(x_lo, y_hi), Vec2::new(x_lo, y_lo), unassigned_color);
        }
    }
}

/// Renders resource row name labels in Resource view using egui, positioned at
/// the screen Y that corresponds to each row's world-space Y coordinate.
fn resource_row_labels_ui(
    mut contexts: EguiContexts,
    mode: Res<schedule::TimelineViewMode>,
    model: Res<model::Model>,
    schedule: Res<schedule::Schedule>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    if *mode != schedule::TimelineViewMode::Resource {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Ok((cam_t, proj)) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else { return };
    let Ok(window) = windows.single() else { return };

    let cam_y = cam_t.translation.y;
    let scale = ortho.scale;
    let win_h = window.height();

    let world_y_to_screen = |world_y: f32| -> f32 {
        win_h * 0.5 - (world_y - cam_y) / scale
    };

    let Some(plan) = model.plans.get(&schedule.plan_id) else { return };

    let mut resources: Vec<_> = model.resource_blocks.values().collect();
    resources.sort_by_key(|r| r.id.0);

    let allocated: std::collections::HashSet<model::WorkBlockId> =
        plan.allocations.iter().map(|a| a.work_block_id).collect();
    let has_unassigned = model
        .work_blocks
        .values()
        .any(|wb| wb.duration_days > 0.0 && !allocated.contains(&wb.id));

    egui::Area::new(egui::Id::new("resource_row_labels"))
        .fixed_pos(egui::Pos2::ZERO)
        .interactable(false)
        .show(ctx, |ui| {
            let label_x = constants::SIDE_PANEL_WIDTH + 6.0;
            for (row, resource) in resources.iter().enumerate() {
                let world_y = -(row as f32) * constants::ROW_HEIGHT;
                let sy = world_y_to_screen(world_y);
                ui.put(
                    egui::Rect::from_min_size(
                        egui::Pos2::new(label_x, sy - 8.0),
                        egui::Vec2::new(150.0, 16.0),
                    ),
                    egui::Label::new(
                        egui::RichText::new(resource.name.as_str())
                            .size(12.0)
                            .color(egui::Color32::from_rgb(180, 180, 210)),
                    ),
                );
            }
            if has_unassigned {
                let row = resources.len();
                let world_y = -(row as f32) * constants::ROW_HEIGHT;
                let sy = world_y_to_screen(world_y);
                ui.put(
                    egui::Rect::from_min_size(
                        egui::Pos2::new(label_x, sy - 8.0),
                        egui::Vec2::new(150.0, 16.0),
                    ),
                    egui::Label::new(
                        egui::RichText::new("Unassigned")
                            .size(12.0)
                            .color(egui::Color32::from_rgba_unmultiplied(140, 140, 170, 180)),
                    ),
                );
            }
        });
}

fn setup_demo_schedule(mut model: ResMut<model::Model>, mut commands: Commands) {
    use model::{DependencyType, Estimate};

    let est = |d: f32| Estimate {
        most_likely: d,
        optimistic: d * 0.7,
        pessimistic: d * 1.5,
        confidence: 0.8,
    };

    let world_id = model.create_world("Demo");
    let plan_id = model.create_plan("Demo Plan", world_id);

    let design = model.create_work_block("Design", est(5.0));
    let build = model.create_work_block("Build", est(8.0));
    let test = model.create_work_block("Test", est(4.0));
    let review = model.create_work_block("Review", est(2.0));
    let deploy = model.create_work_block("Deploy", est(1.0));

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

fn update_analysis(
    model: Res<model::Model>,
    mut sa: ResMut<analysis::ScheduleAnalysis>,
) {
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

/// Renders Re-center and Fit-to-view buttons in a small floating area
/// anchored to the top-right of the window. Keyboard shortcuts (Home / F)
/// are handled by `camera_nav_keys` in `camera.rs`.
fn camera_nav_ui(
    mut contexts: EguiContexts,
    mut target: ResMut<CameraTarget>,
    model: Res<model::Model>,
    scope: Res<schedule::ViewScope>,
    windows: Query<&Window>,
    mut view_mode: ResMut<schedule::TimelineViewMode>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::Area::new(egui::Id::new("camera_nav"))
        .anchor(egui::Align2::RIGHT_TOP, egui::Vec2::new(-8.0, 8.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("Re-center [Home]").clicked() {
                    target.pos = Vec2::ZERO;
                    target.zoom = 1.0;
                }
                if ui.small_button("Fit to view [F]").clicked() {
                    if let Some(new_target) = camera::fit_to_blocks(&model, &scope, &windows) {
                        *target = new_target;
                    }
                }
                ui.separator();
                let (label, next) = match *view_mode {
                    schedule::TimelineViewMode::Task =>
                        ("Resource View", schedule::TimelineViewMode::Resource),
                    schedule::TimelineViewMode::Resource =>
                        ("Task View", schedule::TimelineViewMode::Task),
                };
                if ui.small_button(label).clicked() {
                    *view_mode = next;
                }
            });
        });
}

/// Renders the brick_road logo as a floating button anchored to the upper-left
/// corner of the window. The logo renders on top of the side panel and serves
/// as a persistent home/brand button — clicking it triggers fit-to-view,
/// identical to the keyboard shortcut `F`.
///
/// The amber warm-glow styling complements the HDR bloom aesthetic of the
/// main timeline canvas.
fn logo_ui(
    mut contexts: EguiContexts,
    mut target: ResMut<CameraTarget>,
    model: Res<model::Model>,
    scope: Res<schedule::ViewScope>,
    windows: Query<&Window>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::Area::new(egui::Id::new("brick_road_logo"))
        .anchor(egui::Align2::LEFT_TOP, egui::Vec2::new(8.0, 8.0))
        .interactable(true)
        .show(ctx, |ui| {
            let text = egui::RichText::new("brick_road")
                .size(18.0)
                .color(egui::Color32::from_rgb(250, 165, 40));
            let btn = egui::Button::new(text)
                .fill(egui::Color32::from_rgba_unmultiplied(22, 14, 4, 215))
                .stroke(egui::Stroke::new(1.5, egui::Color32::from_rgb(180, 105, 25)));
            if ui.add(btn).on_hover_text("Fit to view [F]").clicked() {
                if let Some(new_target) = camera::fit_to_blocks(&model, &scope, &windows) {
                    *target = new_target;
                }
            }
        });
}

/// Maps a confidence level to (optimistic_factor, pessimistic_factor).
/// Factors multiply `duration_days` to derive the uncertainty spread.
fn confidence_to_factors(confidence: f32) -> (f32, f32) {
    if confidence >= 1.0 {
        (1.0, 1.0) // Actual — no uncertainty
    } else if confidence >= 0.75 {
        (0.7, 1.4) // 75% — confident, modest spread
    } else {
        (0.5, 2.0) // 50% — rough guess, wide spread
    }
}

fn side_panel_ui(
    mut contexts: EguiContexts,
    mut selected: ResMut<blocks::SelectedBlock>,
    mut model: ResMut<model::Model>,
    mut schedule: ResMut<schedule::Schedule>,
    conn: NonSend<rusqlite::Connection>,
    mut cycle_error: Local<Option<String>>,
    mut scope: ResMut<schedule::ViewScope>,
    mut create_state: ResMut<blocks::CreateModeState>,
    mut new_size_label: Local<String>,
    mut new_size_error: Local<Option<String>>,
    mut camera_target: ResMut<CameraTarget>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::SidePanel::left("side_panel")
        .min_width(SIDE_PANEL_WIDTH)
        .show(ctx, |ui| {
            // Space reserved for the floating logo button (logo_ui anchored at LEFT_TOP+(8,8)).
            ui.add_space(28.0);

            // Plan selector — tabs across the top of the panel.
            {
                let mut sorted_plans: Vec<_> = model
                    .plans
                    .values()
                    .map(|p| (p.id, p.name.clone()))
                    .collect();
                sorted_plans.sort_by_key(|(id, _)| id.0);

                let current_plan_id = schedule.plan_id;
                let mut switch_to: Option<model::PlanId> = None;
                let mut create_plan = false;

                ui.horizontal_wrapped(|ui| {
                    for (pid, name) in &sorted_plans {
                        if ui.selectable_label(current_plan_id == *pid, name).clicked()
                            && current_plan_id != *pid
                        {
                            switch_to = Some(*pid);
                        }
                    }
                    if ui.small_button("+").on_hover_text("New plan").clicked() {
                        create_plan = true;
                    }
                });

                if let Some(target_id) = switch_to {
                    if let Some(plan) = model.plans.get(&target_id).cloned() {
                        let dep_graph = graph::build_graph(&model, &plan);
                        *schedule = schedule::forward_pass(&model, &plan, &dep_graph)
                            .unwrap_or_else(|_| schedule::Schedule::new(target_id));
                    }
                    scope.scope_stack.clear();
                    selected.0 = None;
                }

                if create_plan {
                    let world_id = model
                        .plans
                        .get(&current_plan_id)
                        .map(|p| p.world_id)
                        .or_else(|| model.worlds.keys().next().copied());
                    if let Some(wid) = world_id {
                        let n = model.plans.len() + 1;
                        let new_id = model.create_plan(format!("Plan {n}"), wid);
                        *schedule = schedule::Schedule::new(new_id);
                        scope.scope_stack.clear();
                        selected.0 = None;
                        if let Err(e) = db::save_model(&conn, &model) {
                            error!("save_model failed: {e}");
                        }
                    }
                }
            }

            // Breadcrumb: show full navigation path when drilled in.
            // Clicking an ancestor segment truncates the stack back to that level.
            if !scope.scope_stack.is_empty() {
                let stack_len = scope.scope_stack.len();
                let names: Vec<String> = scope
                    .scope_stack
                    .iter()
                    .map(|&id| {
                        model
                            .work_blocks
                            .get(&id)
                            .map(|wb| wb.name.clone())
                            .unwrap_or_else(|| "?".to_string())
                    })
                    .collect();
                let mut truncate_to: Option<usize> = None;
                ui.horizontal(|ui| {
                    if ui.small_button("Root").clicked() {
                        truncate_to = Some(0);
                    }
                    for (i, name) in names.iter().enumerate() {
                        ui.label("›");
                        if i + 1 < stack_len {
                            if ui.small_button(name.as_str()).clicked() {
                                truncate_to = Some(i + 1);
                            }
                        } else {
                            ui.label(name.as_str());
                        }
                    }
                });
                if let Some(depth) = truncate_to {
                    scope.scope_stack.truncate(depth);
                }
            }
            ui.separator();

            if ui.button("Auto-schedule").clicked() {
                let plan_id = schedule.plan_id;
                if let Some(plan) = model.plans.get(&plan_id).cloned() {
                    let dep_graph = graph::build_graph(&model, &plan);
                    match schedule::forward_pass(&model, &plan, &dep_graph) {
                        Ok(new_sched) => {
                            *cycle_error = None;
                            for sb in new_sched.blocks.values() {
                                if let Some(wb) = model.work_blocks.get_mut(&sb.work_block_id) {
                                    wb.start_day = sb.start_day;
                                    wb.duration_days = sb.duration_days;
                                }
                            }
                            *schedule = new_sched;
                        }
                        Err(_) => {
                            *cycle_error =
                                Some("Cycle detected — fix dependencies first".to_string());
                        }
                    }
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            if let Some(msg) = &*cycle_error {
                ui.colored_label(egui::Color32::from_rgb(220, 60, 60), msg);
            }

            ui.separator();
            ui.label("Calendar");

            let cal_start = model.calendar.start_date;
            let cal_wdpw = model.calendar.working_days_per_week;
            let mut date_str = cal_start.format("%Y-%m-%d").to_string();
            let mut new_wdpw = cal_wdpw;

            ui.label("Plan Start Date");
            let date_changed = ui.text_edit_singleline(&mut date_str).changed();

            ui.label("Working Days / Week");
            ui.horizontal(|ui| {
                for days in [4u8, 5, 6, 7] {
                    if ui.radio(cal_wdpw == days, days.to_string()).clicked() {
                        new_wdpw = days;
                    }
                }
            });

            if date_changed {
                if let Ok(d) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
                    model.calendar.start_date = d;
                    if let Err(e) = db::save_model(&conn, &model) {
                        error!("save_model failed: {e}");
                    }
                }
            }
            if new_wdpw != cal_wdpw {
                model.calendar.working_days_per_week = new_wdpw;
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            ui.separator();

            let label = if create_state.active {
                "⏹ Creating Blocks [N]"
            } else {
                "＋ Create Blocks [N]"
            };
            if ui.selectable_label(create_state.active, label).clicked() {
                create_state.active = !create_state.active;
                if !create_state.active {
                    create_state.text_buf.clear();
                }
            }

            ui.separator();
            ui.collapsing("Size Mapping", |ui| {
                let mut mapping_changed = false;
                let mut to_remove: Option<usize> = None;
                for (i, size) in model.t_shirt_sizes.iter_mut().enumerate() {
                    let row = ui.horizontal(|ui| {
                        let label_changed = ui
                            .add(egui::TextEdit::singleline(&mut size.label).desired_width(36.0))
                            .lost_focus();
                        let days_changed = ui
                            .add(
                                egui::DragValue::new(&mut size.days)
                                    .speed(0.5)
                                    .range(0.5f32..=120.0)
                                    .suffix(" d"),
                            )
                            .changed();
                        let removed = ui.small_button("×").clicked();
                        if removed {
                            to_remove = Some(i);
                        }
                        label_changed || days_changed || removed
                    });
                    if row.inner {
                        mapping_changed = true;
                    }
                }
                if let Some(idx) = to_remove {
                    model.t_shirt_sizes.remove(idx);
                }

                // New-size input row: validate uniqueness before inserting.
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut *new_size_label)
                            .desired_width(36.0)
                            .hint_text("label"),
                    );
                    if ui.small_button("+ Add").clicked() {
                        let label = new_size_label.trim().to_string();
                        if label.is_empty() {
                            *new_size_error = Some("Label cannot be empty".to_string());
                        } else if model.t_shirt_sizes.iter().any(|s| s.label == label) {
                            *new_size_error = Some(format!("'{label}' already exists"));
                        } else {
                            model.t_shirt_sizes.push(model::TShirtSize { label, days: 1.0 });
                            new_size_label.clear();
                            *new_size_error = None;
                            mapping_changed = true;
                        }
                    }
                });
                if let Some(ref err) = *new_size_error {
                    ui.colored_label(egui::Color32::from_rgb(220, 60, 60), err);
                }

                if mapping_changed {
                    // Guard against duplicate labels from inline edits before saving.
                    let unique: std::collections::HashSet<_> =
                        model.t_shirt_sizes.iter().map(|s| &s.label).collect();
                    if unique.len() < model.t_shirt_sizes.len() {
                        *new_size_error =
                            Some("Duplicate size label — rename before saving".to_string());
                    } else {
                        *new_size_error = None;
                        if let Err(e) = db::save_model(&conn, &model) {
                            error!("save_model failed: {e}");
                        }
                    }
                }
            });

            ui.separator();

            let Some(sel_id) = selected.0 else {
                ui.label("Click a block to inspect.");
                return;
            };

            // Compute row index using the canonical sort order shared with block sprites.
            let row = schedule::sorted_blocks(&model)
                .iter()
                .position(|b| b.id == sel_id);

            // Clone display values before any mutable borrow of model.
            let Some(wb) = model.work_blocks.get(&sel_id) else {
                return;
            };
            let mut name = wb.name.clone();
            let mut description = wb.description.clone();
            let mut duration_days = wb.duration_days;
            let confidence = wb.estimate.confidence;
            let color = wb.color;
            let priority = wb.priority;
            let block_variant_ids = wb.variants.clone();
            let current_t_shirt_size = wb.t_shirt_size.clone();

            let (start_day, end_day) = (wb.start_day, wb.start_day + wb.duration_days);

            // Clone t-shirt sizes and variant names before any mutable model borrow.
            let t_shirt_sizes: Vec<(String, f32)> = model
                .t_shirt_sizes
                .iter()
                .map(|s| (s.label.clone(), s.days))
                .collect();
            let variant_names: Vec<(model::VariantId, String)> = block_variant_ids
                .iter()
                .filter_map(|&vid| model.variants.get(&vid).map(|v| (vid, v.name.clone())))
                .collect();
            let plan_id = schedule.plan_id;
            let current_var = model
                .plans
                .get(&plan_id)
                .and_then(|p| p.selected_variants.get(&sel_id).copied());

            let name_changed = ui.text_edit_singleline(&mut name).changed();
            if name_changed && !name.trim().is_empty() {
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.name = name.trim().to_string();
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            ui.label("Notes");
            let desc_changed = ui
                .add(egui::TextEdit::multiline(&mut description).desired_rows(3))
                .changed();
            if desc_changed {
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.description = description;
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            // Variant selector — only shown when the block has variants.
            if !variant_names.is_empty() {
                ui.separator();
                ui.label("Variants");
                let mut new_sel: Option<Option<model::VariantId>> = None;
                if ui.radio(current_var.is_none(), "None").clicked() {
                    new_sel = Some(None);
                }
                for &(var_id, ref var_name) in &variant_names {
                    if ui.radio(current_var == Some(var_id), var_name.as_str()).clicked() {
                        new_sel = Some(Some(var_id));
                    }
                }
                if let Some(selection) = new_sel {
                    // Zero out the old variant's children so they disappear from the timeline.
                    if let Some(old_vid) = current_var {
                        if let Some(old_v) = model.variants.get(&old_vid) {
                            let children: Vec<_> = old_v.children.clone();
                            for child_id in children {
                                if let Some(wb) = model.work_blocks.get_mut(&child_id) {
                                    wb.start_day = 0.0;
                                    wb.duration_days = 0.0;
                                }
                            }
                        }
                    }
                    // Apply new selection (or remove it when "None" was chosen).
                    if let Some(plan) = model.plans.get_mut(&plan_id) {
                        match selection {
                            Some(var_id) => {
                                plan.selected_variants.insert(sel_id, var_id);
                            }
                            None => {
                                plan.selected_variants.remove(&sel_id);
                            }
                        }
                    }
                    // Recompute schedule from the updated variant selection.
                    if let Some(plan) = model.plans.get(&plan_id).cloned() {
                        let dep_graph = graph::build_graph(&model, &plan);
                        if let Ok(new_sched) = schedule::forward_pass(&model, &plan, &dep_graph) {
                            *cycle_error = None;
                            for sb in new_sched.blocks.values() {
                                if let Some(wb) = model.work_blocks.get_mut(&sb.work_block_id) {
                                    wb.start_day = sb.start_day;
                                    wb.duration_days = sb.duration_days;
                                }
                            }
                            *schedule = new_sched;
                        }
                    }
                    if let Err(e) = db::save_model(&conn, &model) {
                        error!("save_model failed: {e}");
                    }
                }
            }

            ui.separator();
            {
                let cal = &model.calendar;
                let start_date = schedule::working_day_to_date(start_day, cal);
                let end_date = schedule::working_day_to_date(end_day, cal);
                let cal_days = schedule::calendar_span(start_day, duration_days, cal);
                ui.label(format!("Start:  {} (day {:.0})", start_date.format("%b %-d"), start_day));
                ui.label(format!(
                    "End:    {} ({:.0}d effort / {} cal)",
                    end_date.format("%b %-d"),
                    duration_days,
                    cal_days
                ));
            }
            if let Some(r) = row {
                ui.label(format!("Row:    {}", r));
            }

            ui.separator();
            ui.label("Size");
            let mut size_chosen: Option<(String, f32)> = None;
            ui.horizontal_wrapped(|ui| {
                for (label, days) in &t_shirt_sizes {
                    let active = current_t_shirt_size.as_deref() == Some(label.as_str());
                    let btn = egui::Button::new(label.as_str()).min_size(egui::Vec2::new(32.0, 22.0));
                    let btn = if active {
                        btn.stroke(egui::Stroke::new(2.0, egui::Color32::WHITE))
                    } else {
                        btn
                    };
                    if ui.add(btn).on_hover_text(format!("{} days", days)).clicked() {
                        size_chosen = Some((label.clone(), *days));
                    }
                }
            });

            if let Some((label, days)) = size_chosen {
                duration_days = days;
                let (opt_f, pes_f) = confidence_to_factors(confidence);
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.t_shirt_size = Some(label);
                    wb.duration_days = days;
                    wb.estimate.most_likely = days;
                    wb.estimate.optimistic = days * opt_f;
                    wb.estimate.pessimistic = days * pes_f;
                }
                schedule::cascade_dependencies(&mut model, sel_id);
                if let Err(e) = db::record_estimate_snapshot(&conn, sel_id.0, duration_days, confidence) {
                    error!("record_estimate_snapshot failed: {e}");
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            // Custom numeric override — clears the t-shirt size label.
            let dur_changed = ui.horizontal(|ui| {
                ui.label("Custom:");
                ui.add(
                    egui::DragValue::new(&mut duration_days)
                        .speed(0.5)
                        .range(0.5f32..=60.0)
                        .suffix(" days"),
                ).changed()
            }).inner;

            if dur_changed {
                let (opt_f, pes_f) = confidence_to_factors(confidence);
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.t_shirt_size = None;
                    wb.duration_days = duration_days;
                    wb.estimate.most_likely = duration_days;
                    wb.estimate.optimistic = duration_days * opt_f;
                    wb.estimate.pessimistic = duration_days * pes_f;
                }
                schedule::cascade_dependencies(&mut model, sel_id);
                if let Err(e) = db::record_estimate_snapshot(&conn, sel_id.0, duration_days, confidence) {
                    error!("record_estimate_snapshot failed: {e}");
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            ui.separator();
            ui.label("Confidence");
            let mut new_confidence = confidence;
            ui.horizontal(|ui| {
                if ui.radio((confidence - 0.5).abs() < 0.01, "50%").clicked() {
                    new_confidence = 0.5;
                }
                if ui.radio((confidence - 0.75).abs() < 0.01, "75%").clicked() {
                    new_confidence = 0.75;
                }
                if ui.radio((confidence - 1.0).abs() < 0.01, "Actual").clicked() {
                    new_confidence = 1.0;
                }
            });

            if (new_confidence - confidence).abs() > 0.001 {
                let (opt_f, pes_f) = confidence_to_factors(new_confidence);
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.estimate.confidence = new_confidence;
                    wb.estimate.most_likely = duration_days;
                    wb.estimate.optimistic = duration_days * opt_f;
                    wb.estimate.pessimistic = duration_days * pes_f;
                }
                if let Err(e) = db::record_estimate_snapshot(&conn, sel_id.0, duration_days, new_confidence) {
                    error!("record_estimate_snapshot failed: {e}");
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            ui.separator();
            ui.label("Color");

            // Preset HDR-friendly swatches — channels > 1.0 trigger bloom.
            const PRESETS: &[(&str, [f32; 3])] = &[
                ("Amber",   [2.0, 0.5, 0.1]),
                ("Green",   [0.2, 1.8, 0.5]),
                ("Cyan",    [0.2, 0.8, 3.0]),
                ("Magenta", [2.2, 0.3, 1.5]),
                ("Yellow",  [2.5, 1.8, 0.1]),
                ("Blue",    [0.5, 0.5, 3.0]),
                ("Pink",    [2.5, 0.3, 2.0]),
                ("Teal",    [0.2, 2.5, 1.5]),
                ("Orange",  [3.0, 1.0, 0.1]),
                ("Purple",  [1.2, 0.2, 2.5]),
            ];

            let mut color_changed = false;
            let mut new_color = color;

            ui.horizontal_wrapped(|ui| {
                for (label, rgb) in PRESETS {
                    let [r, g, b] = *rgb;
                    // Tone-map HDR → 8-bit for the swatch background.
                    let fill = egui::Color32::from_rgb(
                        ((r / 3.5).min(1.0) * 220.0) as u8,
                        ((g / 3.5).min(1.0) * 220.0) as u8,
                        ((b / 3.5).min(1.0) * 220.0) as u8,
                    );
                    let active = color.is_some_and(|c| {
                        (c[0] - r).abs() < 0.01
                            && (c[1] - g).abs() < 0.01
                            && (c[2] - b).abs() < 0.01
                    });
                    let mut btn = egui::Button::new("")
                        .fill(fill)
                        .min_size(egui::Vec2::splat(18.0));
                    if active {
                        btn = btn.stroke(egui::Stroke::new(2.0, egui::Color32::WHITE));
                    }
                    if ui.add(btn).on_hover_text(*label).clicked() {
                        new_color = Some(*rgb);
                        color_changed = true;
                    }
                }
                if ui
                    .small_button("×")
                    .on_hover_text("Reset to palette color")
                    .clicked()
                {
                    new_color = None;
                    color_changed = true;
                }
            });

            // Custom HDR inputs — allow values > 1.0 for bloom.
            ui.label("Custom (R / G / B)");
            let mut custom = color.unwrap_or([1.0, 1.0, 1.0]);
            let (cr, cg, cb) = ui.horizontal(|ui| {
                let cr = ui.add(egui::DragValue::new(&mut custom[0]).speed(0.05).range(0.0f32..=3.0).prefix("R ")).changed();
                let cg = ui.add(egui::DragValue::new(&mut custom[1]).speed(0.05).range(0.0f32..=3.0).prefix("G ")).changed();
                let cb = ui.add(egui::DragValue::new(&mut custom[2]).speed(0.05).range(0.0f32..=3.0).prefix("B ")).changed();
                (cr, cg, cb)
            }).inner;
            if cr || cg || cb {
                new_color = Some(custom);
                color_changed = true;
            }

            if color_changed {
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.color = new_color;
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            ui.separator();
            ui.label("Priority");
            let mut new_priority = priority;
            ui.horizontal(|ui| {
                for (label, val) in [("Low", 0u8), ("Normal", 1), ("High", 2), ("Critical", 3)] {
                    if ui.radio(priority == val, label).clicked() {
                        new_priority = val;
                    }
                }
            });
            if new_priority != priority {
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.priority = new_priority;
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            ui.separator();
            ui.label("Dependencies");

            // Snapshot before any mutation to avoid borrow conflict.
            let predecessors: Vec<_> = model
                .dependencies
                .values()
                .filter(|d| d.successor == sel_id)
                .map(|d| (d.id, d.predecessor, d.dependency_type))
                .collect();
            let successors: Vec<_> = model
                .dependencies
                .values()
                .filter(|d| d.predecessor == sel_id)
                .map(|d| (d.id, d.successor, d.dependency_type))
                .collect();

            let mut dep_to_delete: Option<model::DependencyId> = None;
            let mut jump_to: Option<model::WorkBlockId> = None;

            if predecessors.is_empty() && successors.is_empty() {
                ui.weak("None");
            } else {
                if !predecessors.is_empty() {
                    ui.weak("Predecessors");
                    for (dep_id, pred_id, dep_type) in &predecessors {
                        let pred_name = model
                            .work_blocks
                            .get(pred_id)
                            .map(|wb| wb.name.clone())
                            .unwrap_or_else(|| "?".to_string());
                        ui.horizontal(|ui| {
                            if ui
                                .link(format!("{} [{}]", pred_name, dep_type_abbrev(dep_type)))
                                .clicked()
                            {
                                jump_to = Some(*pred_id);
                            }
                            if ui.small_button("×").on_hover_text("Remove dependency").clicked() {
                                dep_to_delete = Some(*dep_id);
                            }
                        });
                    }
                }
                if !successors.is_empty() {
                    ui.weak("Successors");
                    for (dep_id, succ_id, dep_type) in &successors {
                        let succ_name = model
                            .work_blocks
                            .get(succ_id)
                            .map(|wb| wb.name.clone())
                            .unwrap_or_else(|| "?".to_string());
                        ui.horizontal(|ui| {
                            if ui
                                .link(format!("{} [{}]", succ_name, dep_type_abbrev(dep_type)))
                                .clicked()
                            {
                                jump_to = Some(*succ_id);
                            }
                            if ui.small_button("×").on_hover_text("Remove dependency").clicked() {
                                dep_to_delete = Some(*dep_id);
                            }
                        });
                    }
                }
            }

            if let Some(dep_id) = dep_to_delete {
                model.dependencies.remove(&dep_id);
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }
            if let Some(target_id) = jump_to {
                selected.0 = Some(target_id);
                // Pan the camera to centre the target block in the timeline.
                if let Some(wb) = model.work_blocks.get(&target_id) {
                    if wb.duration_days > 0.0 {
                        let sorted = schedule::sorted_blocks(&model);
                        let row = sorted.iter().position(|b| b.id == target_id).unwrap_or(0);
                        let cx = (wb.start_day + wb.duration_days * 0.5) * PIXELS_PER_DAY;
                        let cy = -(row as f32) * constants::ROW_HEIGHT;
                        camera_target.pos = Vec2::new(cx, cy);
                    }
                }
            }

            ui.separator();
            ui.label("Variants");

            let variant_ids: Vec<_> = model
                .work_blocks
                .get(&sel_id)
                .map(|wb| wb.variants.clone())
                .unwrap_or_default();

            let mut drill_in = false;

            for vid in &variant_ids {
                if let Some(v) = model.variants.get(vid) {
                    let label = format!("{} ({} blocks)", v.name, v.children.len());
                    if ui.link(label).on_hover_text("Drill in to edit").clicked() {
                        drill_in = true;
                    }
                }
            }

            if ui.button("+ New Variant").clicked() {
                let variant_name = format!("Variant {}", variant_ids.len() + 1);
                let vid = model.create_variant(&variant_name, sel_id);
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.variants.push(vid);
                }
                drill_in = true;
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            if drill_in && !scope.scope_stack.contains(&sel_id) {
                scope.scope_stack.push(sel_id);
            }
        });
}

fn dep_type_abbrev(t: &model::DependencyType) -> &'static str {
    match t {
        model::DependencyType::FinishToStart  => "F→S",
        model::DependencyType::StartToStart   => "S→S",
        model::DependencyType::FinishToFinish => "F→F",
        model::DependencyType::StartToFinish  => "S→F",
    }
}
