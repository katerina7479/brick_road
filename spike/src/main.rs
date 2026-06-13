use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    post_process::bloom::Bloom,
    prelude::*,
    render::view::Hdr,
    window::PrimaryWindow,
};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};

const PIXELS_PER_DAY: f32 = 80.0;
const ROW_HEIGHT: f32 = 90.0;
const BAR_HEIGHT: f32 = 55.0;

#[derive(Component)]
struct WorkBlock {
    start_day: f32,
    duration_days: f32,
    row: i32,
}

#[derive(Resource)]
struct CameraTarget {
    pos: Vec2,
    zoom: f32,
}

impl Default for CameraTarget {
    fn default() -> Self {
        Self {
            pos: Vec2::new(900.0, 90.0),
            zoom: 1.0,
        }
    }
}

#[derive(Resource, Default)]
struct SelectionState {
    entity: Option<Entity>,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "brick_road — Bevy spike".to_string(),
                resolution: (1400u32, 700u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.05)))
        .insert_resource(CameraTarget::default())
        .insert_resource(SelectionState::default())
        .add_systems(Startup, setup)
        .add_systems(EguiPrimaryContextPass, side_panel_ui)
        .add_systems(Update, (camera_pan_zoom, sync_blocks, handle_click, draw_grid))
        .run();
}

fn setup(mut commands: Commands) {
    // HDR camera with bloom — Hdr is a marker component in 0.18; linear colors > 1.0 glow
    commands.spawn((
        Camera2d,
        Hdr,
        Tonemapping::TonyMcMapface,
        Bloom::default(),
    ));

    // 6 work blocks across 3 rows
    let blocks: &[(f32, f32, i32)] = &[
        (0.0, 5.0, 0),
        (8.0, 9.0, 0),
        (3.0, 11.0, 1),
        (17.0, 6.0, 1),
        (6.0, 13.0, 2),
        (22.0, 8.0, 2),
    ];

    // Linear RGB with channels > 1.0 triggers bloom
    let row_colors: [Color; 3] = [
        Color::linear_rgb(0.1, 3.2, 4.8),  // teal
        Color::linear_rgb(0.4, 1.0, 4.5),  // blue
        Color::linear_rgb(1.8, 0.6, 4.2),  // violet
    ];

    for &(start_day, duration_days, row) in blocks {
        let color = row_colors[row as usize % 3];
        let width = duration_days * PIXELS_PER_DAY;
        let x = start_day * PIXELS_PER_DAY + width / 2.0;
        let y = row as f32 * ROW_HEIGHT;

        commands.spawn((
            Sprite {
                color,
                custom_size: Some(Vec2::new(width, BAR_HEIGHT)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
            WorkBlock {
                start_day,
                duration_days,
                row,
            },
        ));
    }
}

fn sync_blocks(mut query: Query<(&WorkBlock, &mut Transform, &mut Sprite)>) {
    for (block, mut transform, mut sprite) in query.iter_mut() {
        let width = block.duration_days * PIXELS_PER_DAY;
        let x = block.start_day * PIXELS_PER_DAY + width / 2.0;
        let y = block.row as f32 * ROW_HEIGHT;
        transform.translation.x = x;
        transform.translation.y = y;
        sprite.custom_size = Some(Vec2::new(width, BAR_HEIGHT));
    }
}

fn camera_pan_zoom(
    mut cam_target: ResMut<CameraTarget>,
    mut cam_q: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
) {
    // Pan: right- or middle-click drag
    if (mouse_buttons.pressed(MouseButton::Middle)
        || mouse_buttons.pressed(MouseButton::Right))
        && mouse_motion.delta != Vec2::ZERO
    {
        cam_target.pos.x -= mouse_motion.delta.x * cam_target.zoom;
        cam_target.pos.y += mouse_motion.delta.y * cam_target.zoom;
    }

    // Zoom: scroll wheel
    if mouse_scroll.delta != Vec2::ZERO {
        let scroll = match mouse_scroll.unit {
            MouseScrollUnit::Line => mouse_scroll.delta.y,
            MouseScrollUnit::Pixel => mouse_scroll.delta.y / 60.0,
        };
        cam_target.zoom *= 1.0 - scroll * 0.10;
        cam_target.zoom = cam_target.zoom.clamp(0.15, 6.0);
    }

    // Exponential smoothing toward target — this is what makes it feel like a place
    let dt = time.delta_secs();
    let smooth = 1.0 - (-14.0 * dt).exp();

    let Ok((mut transform, mut proj)) = cam_q.single_mut() else {
        return;
    };

    transform.translation.x += (cam_target.pos.x - transform.translation.x) * smooth;
    transform.translation.y += (cam_target.pos.y - transform.translation.y) * smooth;

    if let Projection::Orthographic(ref mut ortho) = *proj {
        ortho.scale += (cam_target.zoom - ortho.scale) * smooth;
    }
}

fn handle_click(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    blocks: Query<(Entity, &Transform, &Sprite), With<WorkBlock>>,
    mut selection: ResMut<SelectionState>,
) {
    if !mouse_buttons.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Ok((camera, cam_transform)) = cam_q.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };
    let Ok(world_pos) = camera.viewport_to_world_2d(cam_transform, cursor_pos) else { return };

    let mut hit = None;
    for (entity, transform, sprite) in blocks.iter() {
        let size = sprite.custom_size.unwrap_or(Vec2::new(1.0, BAR_HEIGHT));
        let pos = transform.translation.truncate();
        let half = size / 2.0;
        if world_pos.x >= pos.x - half.x
            && world_pos.x <= pos.x + half.x
            && world_pos.y >= pos.y - half.y
            && world_pos.y <= pos.y + half.y
        {
            hit = Some(entity);
            break;
        }
    }

    selection.entity = hit;
}

fn side_panel_ui(
    mut contexts: EguiContexts,
    selection: Res<SelectionState>,
    mut blocks: Query<&mut WorkBlock>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    egui::SidePanel::left("inspector")
        .min_width(220.0)
        .show(ctx, |ui| {
            ui.heading("brick_road spike");
            ui.separator();
            ui.label("Pan:  right-click or middle-click drag");
            ui.label("Zoom: scroll wheel");
            ui.label("Select: left-click a bar");
            ui.separator();

            if let Some(entity) = selection.entity {
                if let Ok(mut block) = blocks.get_mut(entity) {
                    ui.label(format!("Row {}", block.row));
                    ui.label(format!("Start: day {:.0}", block.start_day));
                    ui.add(
                        egui::Slider::new(&mut block.duration_days, 1.0..=60.0)
                            .text("Duration (days)"),
                    );
                }
            } else {
                ui.label("No block selected");
            }
        });
}

fn draw_grid(mut gizmos: Gizmos) {
    let grid_col = Color::srgba(0.18, 0.18, 0.40, 0.35);
    let base_col = Color::srgba(0.25, 0.25, 0.55, 0.55);

    // Horizontal baseline for each row
    for row in -1..=3 {
        let y = row as f32 * ROW_HEIGHT - ROW_HEIGHT * 0.5;
        gizmos.line_2d(
            Vec2::new(-PIXELS_PER_DAY, y),
            Vec2::new(35.0 * PIXELS_PER_DAY, y),
            base_col,
        );
    }

    // Vertical gridlines every 5 days
    for day in (0..=35i32).step_by(5) {
        let x = day as f32 * PIXELS_PER_DAY;
        gizmos.line_2d(
            Vec2::new(x, -ROW_HEIGHT),
            Vec2::new(x, 3.5 * ROW_HEIGHT),
            grid_col,
        );
    }
}
