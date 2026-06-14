use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    post_process::bloom::Bloom,
    prelude::*,
    render::view::Hdr,
};

const PIXELS_PER_DAY: f32 = 100.0;

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
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.05)))
        .add_systems(Startup, setup_camera)
        .add_systems(Update, draw_grid)
        .run();
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
