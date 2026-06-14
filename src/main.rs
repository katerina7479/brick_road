use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    post_process::bloom::Bloom,
    prelude::*,
    render::view::Hdr,
};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};

pub mod blocks;
pub mod camera;
pub mod constants;
pub mod db;
pub mod graph;
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
        .add_systems(Startup, (setup_db, setup_camera))
        .add_systems(Startup, setup_demo_schedule.after(setup_db))
        .add_systems(PostStartup, blocks::spawn_block_sprites)
        .add_systems(Update, (update_camera_target, smooth_camera).chain())
        .add_systems(Update, draw_grid)
        .add_systems(Update, blocks::handle_block_selection)
        .add_systems(Update, blocks::sync_block_sprites.after(blocks::handle_block_selection))
        .add_systems(EguiPrimaryContextPass, side_panel_ui)
        .run();
}

fn setup_db(world: &mut World) {
    let conn = rusqlite::Connection::open("brick_road.db")
        .expect("failed to open brick_road.db");
    db::create_tables(&conn).expect("failed to create DB tables");
    let model = db::load_model(&conn).expect("failed to load model");
    world.insert_resource(model);
    world.insert_non_send_resource(conn);
}

fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Hdr,
        Tonemapping::TonyMcMapface,
        Bloom::default(),
    ));
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
    gizmos.line_2d(Vec2::new(-5000.0, 0.0), Vec2::new(5000.0, 0.0), baseline_color);
}

fn setup_demo_schedule(mut model: ResMut<model::Model>, mut commands: Commands) {
    use model::{DependencyType, Estimate};

    let est = |d: f32| Estimate { most_likely: d, optimistic: d * 0.7, pessimistic: d * 1.5, confidence: 0.8 };

    let world_id = model.create_world("Demo");
    let plan_id  = model.create_plan("Demo Plan", world_id);

    let design  = model.create_work_block("Design",   est(5.0));
    let build   = model.create_work_block("Build",    est(8.0));
    let test    = model.create_work_block("Test",     est(4.0));
    let review  = model.create_work_block("Review",   est(2.0));
    let deploy  = model.create_work_block("Deploy",   est(1.0));

    model.create_dependency(design, build,  DependencyType::FinishToStart);
    model.create_dependency(build,  test,   DependencyType::FinishToStart);
    model.create_dependency(test,   review, DependencyType::FinishToStart);
    model.create_dependency(review, deploy, DependencyType::FinishToStart);

    let plan = {
        let p = model.plans.get_mut(&plan_id).unwrap();
        p.root_blocks = vec![design, build, test, review, deploy];
        p.clone()
    };

    let dep_graph = graph::build_graph(&model, &plan);
    if let Ok(sched) = schedule::forward_pass(&model, &plan, &dep_graph) {
        commands.insert_resource(sched);
    }
}

fn side_panel_ui(mut contexts: EguiContexts) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::SidePanel::left("side_panel").show(ctx, |ui| {
        ui.heading("brick_road");
        ui.label("(panel placeholder)");
    });
}
