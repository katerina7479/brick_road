use bevy::{
    core_pipeline::tonemapping::Tonemapping, post_process::bloom::Bloom, prelude::*,
    render::view::Hdr,
};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};

pub mod analysis;
pub mod blocks;
pub mod camera;
pub mod constants;
pub mod db;
pub mod graph;
pub mod labels;
pub mod model;
pub mod schedule;

use camera::{camera_nav_keys, smooth_camera, update_camera_target, CameraTarget};
use constants::PIXELS_PER_DAY;

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
        .insert_resource(schedule::ViewScope::default())
        .insert_resource(analysis::ScheduleAnalysis::default())
        .add_systems(Startup, (setup_db, setup_camera))
        .add_systems(Startup, setup_demo_schedule.after(setup_db))
        .add_systems(PostStartup, update_analysis.before(blocks::spawn_block_sprites))
        .add_systems(PostStartup, blocks::spawn_block_sprites)
        .add_systems(
            PostStartup,
            labels::spawn_labels.after(blocks::spawn_block_sprites),
        )
        .add_systems(Update, (camera_nav_keys, update_camera_target, smooth_camera).chain())
        .add_systems(Update, draw_grid)
        .add_systems(Update, update_analysis)
        .add_systems(Update, blocks::handle_name_edit)
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
            blocks::spawn_block_sprites.after(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            blocks::sync_block_sprites
                .after(blocks::handle_block_drag)
                .after(blocks::spawn_block_sprites),
        )
        .add_systems(
            Update,
            blocks::sync_conflict_overlays.after(update_analysis),
        )
        .add_systems(
            Update,
            blocks::sync_uncertainty_overlays.after(blocks::spawn_block_sprites),
        )
        .add_systems(Update, blocks::handle_dep_drag)
        .add_systems(
            Update,
            blocks::draw_dependency_edges.after(update_analysis),
        )
        .add_systems(
            Update,
            labels::spawn_labels
                .after(blocks::handle_block_selection)
                .after(blocks::spawn_block_sprites),
        )
        .add_systems(Update, labels::draw_nesting_indicators)
        .add_systems(Update, labels::draw_violation_indicators)
        .add_systems(Update, labels::scale_labels_to_zoom)
        .add_systems(EguiPrimaryContextPass, side_panel_ui)
        .add_systems(EguiPrimaryContextPass, camera_nav_ui)
        .add_systems(EguiPrimaryContextPass, blocks::draw_name_edit_overlay)
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

fn draw_grid(mut gizmos: Gizmos) {
    let line_color = Color::srgba(0.3, 0.3, 0.5, 0.15);
    let baseline_color = Color::srgba(0.4, 0.4, 0.6, 0.35);

    // Vertical lines at day boundaries
    for day in -50i32..=50 {
        let x = day as f32 * PIXELS_PER_DAY;
        gizmos.line_2d(Vec2::new(x, -5000.0), Vec2::new(x, 5000.0), line_color);
    }

    // Horizontal baseline at y=0
    gizmos.line_2d(
        Vec2::new(-5000.0, 0.0),
        Vec2::new(5000.0, 0.0),
        baseline_color,
    );
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
            });
        });
}

fn side_panel_ui(
    mut contexts: EguiContexts,
    selected: Res<blocks::SelectedBlock>,
    mut model: ResMut<model::Model>,
    mut schedule: ResMut<schedule::Schedule>,
    conn: NonSend<rusqlite::Connection>,
    mut cycle_error: Local<Option<String>>,
    mut scope: ResMut<schedule::ViewScope>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::SidePanel::left("side_panel")
        .min_width(220.0)
        .show(ctx, |ui| {
            ui.heading("brick_road");
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
            let name = wb.name.clone();
            let mut duration_days = wb.duration_days;
            let mut most_likely = wb.estimate.most_likely;
            let mut optimistic = wb.estimate.optimistic;
            let mut pessimistic = wb.estimate.pessimistic;
            let confidence = wb.estimate.confidence;
            let color = wb.color;

            let (start_day, end_day) = (wb.start_day, wb.start_day + wb.duration_days);

            ui.strong(&name);
            ui.separator();
            ui.label(format!("Start:  day {:.1}", start_day));
            ui.label(format!("End:    day {:.1}", end_day));
            if let Some(r) = row {
                ui.label(format!("Row:    {}", r));
            }

            ui.separator();
            ui.label("Duration");
            let changed = ui
                .add(
                    egui::Slider::new(&mut duration_days, 0.5f32..=60.0)
                        .text("days")
                        .step_by(0.5),
                )
                .changed();

            if changed {
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.duration_days = duration_days;
                }
                schedule::cascade_dependencies(&mut model, sel_id);
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }

            ui.separator();
            ui.label("Estimate");
            // Sliders are ordered best→expected→worst. Each slider's range is
            // bounded by its neighbours so the three-point ordering invariant
            // optimistic ≤ most_likely ≤ pessimistic is always maintained.
            let opt_changed = ui
                .add(egui::Slider::new(&mut optimistic, 0.5f32..=most_likely).text("optimistic").step_by(0.5))
                .changed();
            let ml_changed = ui
                .add(egui::Slider::new(&mut most_likely, optimistic..=pessimistic).text("most likely").step_by(0.5))
                .changed();
            let pes_changed = ui
                .add(egui::Slider::new(&mut pessimistic, most_likely..=200.0f32).text("pessimistic").step_by(0.5))
                .changed();
            ui.label(format!("Confidence:   {:.0}%", confidence * 100.0));

            if ml_changed || opt_changed || pes_changed {
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.estimate.most_likely = most_likely;
                    wb.estimate.optimistic = optimistic;
                    wb.estimate.pessimistic = pessimistic;
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

            // Custom HDR sliders — allow values > 1.0 for bloom.
            ui.label("Custom (R / G / B)");
            let mut custom = color.unwrap_or([1.0, 1.0, 1.0]);
            let cr = ui.add(egui::Slider::new(&mut custom[0], 0.0f32..=3.0).text("R").step_by(0.05)).changed();
            let cg = ui.add(egui::Slider::new(&mut custom[1], 0.0f32..=3.0).text("G").step_by(0.05)).changed();
            let cb = ui.add(egui::Slider::new(&mut custom[2], 0.0f32..=3.0).text("B").step_by(0.05)).changed();
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
        });
}
