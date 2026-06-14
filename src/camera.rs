use bevy::{
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
};

/// Desired camera state. Input systems write here; the smoothing system reads it.
#[derive(Resource)]
pub struct CameraTarget {
    pub pos: Vec2,
    pub zoom: f32,
}

impl Default for CameraTarget {
    fn default() -> Self {
        Self {
            pos: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

/// Reads mouse input and updates `CameraTarget`. Must run before `smooth_camera`.
pub fn update_camera_target(
    mut target: ResMut<CameraTarget>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
) {
    if (mouse_buttons.pressed(MouseButton::Middle)
        || mouse_buttons.pressed(MouseButton::Right))
        && mouse_motion.delta != Vec2::ZERO
    {
        target.pos.x -= mouse_motion.delta.x * target.zoom;
        target.pos.y += mouse_motion.delta.y * target.zoom;
    }

    if mouse_scroll.delta != Vec2::ZERO {
        let lines = match mouse_scroll.unit {
            MouseScrollUnit::Line => mouse_scroll.delta.y,
            MouseScrollUnit::Pixel => mouse_scroll.delta.y / 60.0,
        };
        target.zoom *= 1.0 - lines * 0.10;
        target.zoom = target.zoom.clamp(0.15, 6.0);
    }
}

/// Exponentially smooths the actual camera transform toward `CameraTarget`.
/// Must run after `update_camera_target`.
pub fn smooth_camera(
    target: Res<CameraTarget>,
    mut cam_q: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
    time: Res<Time>,
) {
    let alpha = 1.0 - (-14.0 * time.delta_secs()).exp();
    let Ok((mut transform, mut proj)) = cam_q.single_mut() else {
        return;
    };

    transform.translation.x += (target.pos.x - transform.translation.x) * alpha;
    transform.translation.y += (target.pos.y - transform.translation.y) * alpha;

    if let Projection::Orthographic(ref mut ortho) = *proj {
        ortho.scale += (target.zoom - ortho.scale) * alpha;
    }
}
