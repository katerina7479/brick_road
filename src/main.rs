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

use camera::{smooth_camera, update_camera_target, CameraTarget};
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
        .insert_resource(analysis::ScheduleAnalysis::default())
        .add_systems(Startup, (setup_db, setup_camera))
        .add_systems(Startup, setup_demo_schedule.after(setup_db))
        .add_systems(PostStartup, update_analysis.before(blocks::spawn_block_sprites))
        .add_systems(PostStartup, blocks::spawn_block_sprites)
        .add_systems(
            PostStartup,
            labels::spawn_labels.after(blocks::spawn_block_sprites),
        )
        .add_systems(Update, (update_camera_target, smooth_camera).chain())
        .add_systems(Update, draw_grid)
        .add_systems(Update, update_analysis)
        .add_systems(Update, blocks::handle_block_selection)
        .add_systems(
            Update,
            blocks::sync_block_sprites.after(blocks::handle_block_selection),
        )
        .add_systems(
            Update,
            blocks::sync_conflict_overlays.after(update_analysis),
        )
        .add_systems(Update, labels::draw_nesting_indicators)
        .add_systems(EguiPrimaryContextPass, side_panel_ui)
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

fn side_panel_ui(
    mut contexts: EguiContexts,
    selected: Res<blocks::SelectedBlock>,
    mut model: ResMut<model::Model>,
    mut schedule: ResMut<schedule::Schedule>,
    conn: NonSend<rusqlite::Connection>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::SidePanel::left("side_panel")
        .min_width(220.0)
        .show(ctx, |ui| {
            ui.heading("brick_road");
            ui.separator();

            if ui.button("Auto-schedule").clicked() {
                let plan_id = schedule.plan_id;
                if let Some(plan) = model.plans.get(&plan_id).cloned() {
                    let dep_graph = graph::build_graph(&model, &plan);
                    if let Ok(new_sched) = schedule::forward_pass(&model, &plan, &dep_graph) {
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
            let mut most_likely = wb.estimate.most_likely;
            let optimistic = wb.estimate.optimistic;
            let pessimistic = wb.estimate.pessimistic;
            let confidence = wb.estimate.confidence;

            let (start_day, end_day) = model
                .work_blocks
                .get(&sel_id)
                .map(|wb| (wb.start_day, wb.start_day + wb.duration_days))
                .unwrap_or((0.0, 0.0));

            ui.strong(&name);
            ui.separator();
            ui.label(format!("Start:  day {:.1}", start_day));
            ui.label(format!("End:    day {:.1}", end_day));
            if let Some(r) = row {
                ui.label(format!("Row:    {}", r));
            }

            ui.separator();
            ui.label("Estimate");
            let changed = ui
                .add(
                    egui::Slider::new(&mut most_likely, 1.0f32..=60.0)
                        .text("Duration (days)")
                        .step_by(0.5),
                )
                .changed();
            ui.label(format!("Optimistic:   {:.1} days", optimistic));
            ui.label(format!("Pessimistic:  {:.1} days", pessimistic));
            ui.label(format!("Confidence:   {:.0}%", confidence * 100.0));

            if changed {
                if let Some(wb) = model.work_blocks.get_mut(&sel_id) {
                    wb.estimate.most_likely = most_likely;
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }
        });
}
