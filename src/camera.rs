use bevy::{
    input::{
        gestures::PinchGesture,
        mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    },
    prelude::*,
};

use crate::{
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    model::Model,
    schedule,
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

/// Camera target for "Home": anchors **today** to the upper-left of the timeline
/// viewport (so you open looking at today and the work ahead) with the main
/// plan's row 0 at the top, at 1:1 zoom. Shared by the Home key, the Re-center
/// button, and the initial view on launch so they all agree.
pub fn home_target(window: &Window, today_day: i32) -> CameraTarget {
    let w = window.width();
    let h = window.height();
    let today_x = today_day as f32 * PIXELS_PER_DAY;
    CameraTarget {
        zoom: 1.0,
        pos: Vec2::new(
            today_x + w * 0.5 - HOME_LEFT_MARGIN,
            ROW_HEIGHT * 0.5 - (h * 0.5 - HOME_TOP_MARGIN),
        ),
    }
}

/// Camera target that frames the day span `[start_day, end_day]` centered, with
/// generous horizontal margin so there's room to place blocks beyond it (used
/// when drilling into a block — you see the parent's span plus slack on either
/// side). Row 0 anchors near the top, like Home.
pub fn frame_day_span(window: &Window, start_day: i32, end_day: i32) -> CameraTarget {
    let w = window.width();
    let h = window.height();
    let x_min = start_day as f32 * PIXELS_PER_DAY;
    let x_max = end_day as f32 * PIXELS_PER_DAY;
    let span = (x_max - x_min).max(PIXELS_PER_DAY);
    // 1.8 leaves ~45% of the width as slack around the parent's span.
    const MARGIN: f32 = 1.8;
    let avail_w = (w - 2.0 * HOME_LEFT_MARGIN).max(1.0);
    let zoom = ((span / avail_w) * MARGIN).clamp(0.3, 6.0);
    CameraTarget {
        zoom,
        pos: Vec2::new(
            (x_min + x_max) * 0.5,
            ROW_HEIGHT * 0.5 - (h * 0.5 - HOME_TOP_MARGIN) * zoom,
        ),
    }
}

/// Reads mouse / trackpad input and updates `CameraTarget`. Must run before
/// `smooth_camera`.
///
/// - Middle/right-drag pans.
/// - Two-finger trackpad scroll pans at constant zoom — the Mac-native gesture
///   for moving along the timeline.
/// - Trackpad pinch zooms.
/// - Cmd/Ctrl + scroll zooms, so a plain mouse wheel can still zoom.
pub fn update_camera_target(
    mut target: ResMut<CameraTarget>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut pinch_events: MessageReader<PinchGesture>,
) {
    if (mouse_buttons.pressed(MouseButton::Middle) || mouse_buttons.pressed(MouseButton::Right))
        && mouse_motion.delta != Vec2::ZERO
    {
        target.pos.x -= mouse_motion.delta.x * target.zoom;
        target.pos.y += mouse_motion.delta.y * target.zoom;
    }

    // Trackpad pinch → zoom.
    let pinch: f32 = pinch_events.read().map(|e| e.0).sum();
    if pinch != 0.0 {
        target.zoom *= 1.0 - pinch * 2.5;
        target.zoom = target.zoom.clamp(0.15, 6.0);
    }

    if mouse_scroll.delta != Vec2::ZERO {
        let zoom_modifier = keyboard.pressed(KeyCode::SuperLeft)
            || keyboard.pressed(KeyCode::SuperRight)
            || keyboard.pressed(KeyCode::ControlLeft)
            || keyboard.pressed(KeyCode::ControlRight);

        if zoom_modifier {
            // Cmd/Ctrl + scroll → zoom (mouse-wheel fallback).
            let lines = match mouse_scroll.unit {
                MouseScrollUnit::Line => mouse_scroll.delta.y,
                MouseScrollUnit::Pixel => mouse_scroll.delta.y / 60.0,
            };
            target.zoom *= 1.0 - lines * 0.10;
            target.zoom = target.zoom.clamp(0.15, 6.0);
        } else {
            // Plain two-finger scroll → pan along the timeline at constant zoom.
            // Convert to world units via the current zoom so the pan tracks the
            // cursor regardless of scale.
            let px = match mouse_scroll.unit {
                MouseScrollUnit::Line => mouse_scroll.delta * 20.0,
                MouseScrollUnit::Pixel => mouse_scroll.delta,
            };
            target.pos.x -= px.x * target.zoom;
            target.pos.y += px.y * target.zoom;
        }
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
    schedule: Res<schedule::Schedule>,
    today: Res<schedule::TodayMarker>,
    name_edit: Res<crate::blocks::NameEditState>,
    windows: Query<&Window>,
) {
    // A rename in progress owns the keyboard — don't let typing a name that
    // contains 'f' fit-to-view, or 'Home' jump, etc. (`handle_type_to_rename`
    // runs first and sets this on the same keystroke that opens the editor.)
    if name_edit.editing.is_some() {
        return;
    }
    if egui_ctx
        .ctx_mut()
        .ok()
        .is_some_and(|ctx| ctx.wants_keyboard_input())
    {
        return;
    }
    if keyboard.just_pressed(KeyCode::Home) {
        let Ok(window) = windows.single() else { return };
        *target = home_target(window, today.day);
    }
    if keyboard.just_pressed(KeyCode::KeyF) {
        if let Some(new_target) = fit_to_blocks(&model, schedule.plan_id, &windows) {
            *target = new_target;
        }
    }
}

/// Computes a `CameraTarget` that fits the visible (placed) blocks into the
/// timeline area with a 15% padding margin.
/// Returns `None` when there are no placed visible blocks or no window.
pub fn fit_to_blocks(
    model: &Model,
    plan_id: crate::model::PlanId,
    windows: &Query<&Window>,
) -> Option<CameraTarget> {
    let Ok(window) = windows.single() else {
        return None;
    };
    let window_w = window.width();
    let window_h = window.height();

    let visible: Vec<_> = schedule::visible_blocks(model, plan_id, None)
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
    // Rows are explicit and can be sparse/negative, so frame the real lane range.
    let min_row = visible.iter().map(|wb| wb.row).min().unwrap_or(0) as f32;
    let max_row = visible.iter().map(|wb| wb.row).max().unwrap_or(0) as f32;
    let y_max = -min_row * ROW_HEIGHT + ROW_HEIGHT * 0.5;
    let y_min = -max_row * ROW_HEIGHT - ROW_HEIGHT * 0.5;

    let x_span = (x_max - x_min).max(1.0);
    let y_span = (y_max - y_min).max(1.0);

    const MARGIN: f32 = 1.15;
    // The side panel is gone, so the whole window width is available (minus a
    // symmetric margin matching the left anchor).
    let avail_w = (window_w - 2.0 * HOME_LEFT_MARGIN).max(1.0);
    let avail_h = (window_h - HOME_TOP_MARGIN).max(1.0);

    // Clamp the lower bound to 1.0 so fitting a sparse plan (e.g. a single small
    // block) frames it at 1:1 instead of magnifying it to fill the screen.
    let zoom = ((x_span / avail_w).max(y_span / avail_h) * MARGIN).clamp(1.0, 6.0);

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
