use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    post_process::bloom::Bloom,
    prelude::*,
    render::view::Hdr,
};

pub mod camera;

use camera::{smooth_camera, update_camera_target, CameraTarget};

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
        .insert_resource(CameraTarget::default())
        .add_systems(Startup, setup_camera)
        .add_systems(Update, (update_camera_target, smooth_camera).chain())
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
