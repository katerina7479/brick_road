use bevy::{
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
};

use crate::{
    constants::{PIXELS_PER_DAY, ROW_HEIGHT, SIDE_PANEL_WIDTH},
    model::Model,
    schedule::{self, ViewScope},
};

/// Left-edge margin (px) for the plan-start anchor on Home / Fit.
const HOME_LEFT_MARGIN: f32 = 24.0;
/// Top-edge margin (px) for the first row anchor on Home / Fit.
/// Sized to clear the egui top bar (~34 px) plus comfortable padding.
const HOME_TOP_MARGIN: f32 = 84.0;

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
    if (mouse_buttons.pressed(MouseButton::Middle) || mouse_buttons.pressed(MouseButton::Right))
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

/// Handles keyboard camera navigation shortcuts:
/// - `Home` → re-center at origin, reset zoom to 1.0.
/// - `F`    → fit all placed blocks into view.
///
/// Must run before `update_camera_target` so pan/scroll input can still
/// override in the same frame.
pub fn camera_nav_keys(
    mut egui_ctx: bevy_egui::EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut target: ResMut<CameraTarget>,
    model: Res<Model>,
    scope: Res<ViewScope>,
    windows: Query<&Window>,
) {
    if egui_ctx
        .ctx_mut()
        .ok()
        .is_some_and(|ctx| ctx.wants_keyboard_input())
    {
        return;
    }
    if keyboard.just_pressed(KeyCode::Home) {
        let Ok(window) = windows.single() else { return };
        let w = window.width();
        let h = window.height();
        target.zoom = 1.0;
        // Anchor day-0 / row-0 to the upper-left corner with margin.
        target.pos.x = w * 0.5 - HOME_LEFT_MARGIN;
        target.pos.y = ROW_HEIGHT * 0.5 - (h * 0.5 - HOME_TOP_MARGIN);
    }
    if keyboard.just_pressed(KeyCode::KeyF) {
        if let Some(new_target) = fit_to_blocks(&model, &scope, &windows) {
            *target = new_target;
        }
    }
}

/// Computes a `CameraTarget` that fits the *visible* blocks (respecting
/// `scope` drill-in) into the timeline area with a 15% padding margin.
/// Returns `None` when there are no placed visible blocks or no window.
pub fn fit_to_blocks(
    model: &Model,
    scope: &ViewScope,
    windows: &Query<&Window>,
) -> Option<CameraTarget> {
    let Ok(window) = windows.single() else { return None };
    let window_w = window.width();
    let window_h = window.height();

    let visible: Vec<_> = schedule::visible_blocks(model, scope)
        .into_iter()
        .filter(|wb| wb.duration_days > 0)
        .collect();
    if visible.is_empty() {
        return None;
    }

    let x_min = visible
        .iter()
        .map(|wb| wb.start_day as f32 * PIXELS_PER_DAY)
        .fold(f32::INFINITY, f32::min);
    let x_max = visible
        .iter()
        .map(|wb| (wb.start_day + wb.duration_days) as f32 * PIXELS_PER_DAY)
        .fold(f32::NEG_INFINITY, f32::max);
    let n = visible.len() as f32;
    let y_max = ROW_HEIGHT * 0.5;
    let y_min = -(n - 1.0) * ROW_HEIGHT - ROW_HEIGHT * 0.5;

    let x_span = (x_max - x_min).max(1.0);
    let y_span = (y_max - y_min).max(1.0);

    const MARGIN: f32 = 1.15;
    let avail_w = (window_w - SIDE_PANEL_WIDTH).max(1.0);
    let avail_h = (window_h - HOME_TOP_MARGIN).max(1.0);

    let zoom = ((x_span / avail_w).max(y_span / avail_h) * MARGIN).clamp(0.15, 6.0);

    // Anchor plan start at the upper-left corner of the timeline viewport.
    // pos is the world-space point at the window centre.
    let pos_x = x_min + (window_w * 0.5 - HOME_LEFT_MARGIN) * zoom;
    let pos_y = y_max - (window_h * 0.5 - HOME_TOP_MARGIN) * zoom;

    Some(CameraTarget {
        pos: Vec2::new(pos_x, pos_y),
        zoom,
    })
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
