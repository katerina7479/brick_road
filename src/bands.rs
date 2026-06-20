//! Branch plans rendered as editable "swimlanes" below main.
//!
//! Main renders normally at the top (the global block pipeline). Each branch (a
//! plan with a `branch_start_day`) gets a horizontal swimlane below main: a
//! full-width divider marks its top, a faint fill tints the lane, and everything
//! beneath belongs to that branch until the next one's divider.
//!
//! Inside a lane a branch's blocks are drawn two ways:
//!   - **Ghosts** — blocks shared with main (copied forward at fork): a colored
//!     outline matching the source block, transparent interior + name. They
//!     track main and aren't edited here.
//!   - **Owned** — blocks added directly to the branch: solid bars, the branch's
//!     real work. Double-clicking empty lane space creates one.
//!
//! Outlines + dividers are gizmos drawn every frame; the lane fills, owned bars,
//! and all text are entities rebuilt only when the model changes.

use bevy::prelude::*;
use bevy::sprite::Anchor;

use crate::{
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    db,
    model::{Day, Model, Plan, PlanId},
};

/// Gap between main's lowest block and the first lane / between lanes.
const BAND_GAP: f32 = ROW_HEIGHT * 0.7;
/// Height of a block bar in a lane (matches a real block).
const GHOST_HEIGHT: f32 = 28.0;
/// Minimum rows a lane shows even when nearly empty, so there's room to click-add.
const MIN_LANE_ROWS: i32 = 2;
/// Default duration (working days) for a block added to a branch — one week.
const DEFAULT_DURATION: Day = 5;
/// Lane-fill sprite width — large enough to always span the viewport.
const LANE_FILL_WIDTH: f32 = 1_000_000.0;

#[derive(Component)]
pub struct BandEntity;

#[derive(Resource, Default)]
pub struct BandEntities(pub Vec<Entity>);

/// Inline plan-rename state: which branch is being renamed + the text buffer.
#[derive(Resource, Default)]
pub struct PlanRenameState {
    pub editing: Option<PlanId>,
    pub buf: String,
}

/// One block in a lane, world coordinates.
struct BandBlock {
    cx: f32,
    cy: f32,
    w: f32,
    name: String,
    color: LinearRgba,
    /// Owned by the branch (solid) vs inherited from main (hollow ghost).
    owned: bool,
}

/// Computed geometry for one branch's swimlane.
pub struct BandLayout {
    pub plan_id: PlanId,
    pub fork_day: Day,
    pub name: String,
    pub name_x: f32,
    pub name_y: f32,
    /// World-Y of the lane's row 0 (top row), where the name sits.
    pub row0_y: f32,
    /// Upper Y bound (the full-width divider).
    pub lane_top: f32,
    /// Lower Y bound (the next lane's `lane_top`).
    pub lane_bottom: f32,
    blocks: Vec<BandBlock>,
}

/// Main is the one root plan: no `branch_start_day`, lowest id wins.
fn main_plan(model: &Model) -> Option<&Plan> {
    model
        .plans
        .values()
        .filter(|p| p.branch_start_day.is_none())
        .min_by_key(|p| p.id.0)
}

/// World-Y just below main's lowest placed block — where the lanes begin.
fn main_bottom_y(model: &Model, main: &Plan) -> f32 {
    let mut min_y = 0.0_f32;
    for id in &main.root_blocks {
        if let Some(wb) = model.work_blocks.get(id) {
            if wb.duration_days <= 0 {
                continue;
            }
            let y = -(wb.row as f32) * ROW_HEIGHT - ROW_HEIGHT * 0.5;
            if y < min_y {
                min_y = y;
            }
        }
    }
    min_y
}

/// Lays out every branch as a contiguous swimlane stacked below main. Lanes use
/// absolute rows (row 0 at the lane top), so a click maps directly to a row.
pub fn layout_bands(model: &Model) -> Vec<BandLayout> {
    let Some(main) = main_plan(model) else {
        return Vec::new();
    };
    let main_id = main.id;
    let main_set: std::collections::HashSet<_> = main.root_blocks.iter().copied().collect();

    let mut branches: Vec<&Plan> = model
        .plans
        .values()
        .filter(|p| p.id != main_id && p.branch_start_day.is_some())
        .collect();
    branches.sort_by_key(|p| p.id.0);

    let mut out = Vec::new();
    let mut lane_top = main_bottom_y(model, main) - BAND_GAP;

    for branch in branches {
        let fork = branch.branch_start_day.unwrap_or(0);
        let row0_y = lane_top - ROW_HEIGHT * 0.7;

        let mut blocks = Vec::new();
        let mut max_row = 0;
        for id in &branch.root_blocks {
            let Some(wb) = model.work_blocks.get(id) else {
                continue;
            };
            if wb.duration_days <= 0 {
                continue;
            }
            max_row = max_row.max(wb.row);
            let w = (wb.duration_days as f32 * PIXELS_PER_DAY).max(1.0);
            blocks.push(BandBlock {
                cx: wb.start_day as f32 * PIXELS_PER_DAY + w * 0.5,
                cy: row0_y - wb.row as f32 * ROW_HEIGHT,
                w,
                name: wb.name.clone(),
                color: crate::blocks::block_color(wb),
                owned: !main_set.contains(id),
            });
        }

        let rows = (max_row + 1).max(MIN_LANE_ROWS);
        let lane_bottom = row0_y - (rows - 1) as f32 * ROW_HEIGHT - ROW_HEIGHT * 0.7;

        out.push(BandLayout {
            plan_id: branch.id,
            fork_day: fork,
            name: branch.name.clone(),
            name_x: fork as f32 * PIXELS_PER_DAY + 4.0,
            name_y: lane_top - 10.0,
            row0_y,
            lane_top,
            lane_bottom,
            blocks,
        });

        lane_top = lane_bottom; // contiguous lanes
    }
    out
}

/// World-Y of the top of the band strip (first lane's divider), if any branches
/// exist. Block creation/selection on main bails below this so lane clicks are
/// owned by the band handlers.
pub fn bands_top_y(model: &Model) -> Option<f32> {
    layout_bands(model).first().map(|b| b.lane_top)
}

/// Per-frame gizmos: each lane's full-width top divider and every *ghost's*
/// colored outline (owned blocks are drawn solid as entities instead).
pub fn draw_band_overlays(
    mut gizmos: Gizmos,
    model: Res<Model>,
    cam_q: Query<(&Transform, &Projection), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Ok((cam_t, proj)) = cam_q.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let half_w = window.width() * 0.5 * ortho.scale + PIXELS_PER_DAY;
    let cam_x = cam_t.translation.x;
    let divider = Color::srgba(0.55, 0.60, 0.78, 0.30);

    for band in layout_bands(&model) {
        gizmos.line_2d(
            Vec2::new(cam_x - half_w, band.lane_top),
            Vec2::new(cam_x + half_w, band.lane_top),
            divider,
        );
        for b in &band.blocks {
            if b.owned {
                continue; // solid bar drawn as an entity
            }
            let hw = b.w * 0.5;
            let hh = GHOST_HEIGHT * 0.5;
            let tl = Vec2::new(b.cx - hw, b.cy + hh);
            let tr = Vec2::new(b.cx + hw, b.cy + hh);
            let br = Vec2::new(b.cx + hw, b.cy - hh);
            let bl = Vec2::new(b.cx - hw, b.cy - hh);
            let c = Color::from(b.color);
            gizmos.line_2d(tl, tr, c);
            gizmos.line_2d(tr, br, c);
            gizmos.line_2d(br, bl, c);
            gizmos.line_2d(bl, tl, c);
        }
    }
}

/// Rebuilds lane fills, owned (solid) bars, and all text when the model changes.
pub fn sync_band_visuals(mut commands: Commands, model: Res<Model>, mut ents: ResMut<BandEntities>) {
    if !model.is_changed() {
        return;
    }
    for e in ents.0.drain(..) {
        commands.entity(e).despawn();
    }

    for (i, band) in layout_bands(&model).into_iter().enumerate() {
        // Faint full-width lane fill, alternating so adjacent lanes read apart.
        let lane_h = (band.lane_top - band.lane_bottom).max(GHOST_HEIGHT);
        let tint = if i % 2 == 0 { 0.05 } else { 0.025 };
        let fill = commands
            .spawn((
                BandEntity,
                Sprite {
                    color: Color::srgba(0.60, 0.66, 0.85, tint),
                    custom_size: Some(Vec2::new(LANE_FILL_WIDTH, lane_h)),
                    ..default()
                },
                Transform::from_xyz(0.0, (band.lane_top + band.lane_bottom) * 0.5, -2.0),
            ))
            .id();
        ents.0.push(fill);

        // Editable lane name, left-anchored at the fork point.
        let name = commands
            .spawn((
                BandEntity,
                Text2d::new(band.name.clone()),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgba(0.85, 0.88, 0.96, 0.9)),
                Anchor::CENTER_LEFT,
                Transform::from_xyz(band.name_x, band.name_y, -1.0),
            ))
            .id();
        ents.0.push(name);

        for b in &band.blocks {
            // Owned blocks render as solid bars (the branch's real work).
            if b.owned {
                let bar = commands
                    .spawn((
                        BandEntity,
                        Sprite {
                            color: Color::from(b.color),
                            custom_size: Some(Vec2::new(b.w, GHOST_HEIGHT)),
                            ..default()
                        },
                        Transform::from_xyz(b.cx, b.cy, -0.8),
                    ))
                    .id();
                ents.0.push(bar);
            }
            // Name (dark text + light halo) for ghosts and owned alike.
            let halo = commands
                .spawn((
                    BandEntity,
                    Text2d::new(b.name.clone()),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(Color::srgba(1.0, 1.0, 1.0, 0.5)),
                    Anchor::CENTER,
                    Transform::from_xyz(b.cx, b.cy, -0.7),
                ))
                .id();
            ents.0.push(halo);
            let label = commands
                .spawn((
                    BandEntity,
                    Text2d::new(b.name.clone()),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(Color::srgba(0.12, 0.12, 0.15, 1.0)),
                    Anchor::CENTER,
                    Transform::from_xyz(b.cx, b.cy, -0.65),
                ))
                .id();
            ents.0.push(label);
        }
    }
}

/// Maps a world position to the band that contains it (between `lane_top` and
/// `lane_bottom`), returning its index in the current layout.
fn band_at<'a>(bands: &'a [BandLayout], world: Vec2) -> Option<&'a BandLayout> {
    bands
        .iter()
        .find(|b| world.y <= b.lane_top && world.y > b.lane_bottom)
}

/// Double-clicking empty lane space creates a real block owned by that branch,
/// at the clicked day (clamped to ≥ the fork day) and row.
pub fn handle_band_block_create(
    mut egui_ctx: bevy_egui::EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut model: ResMut<Model>,
    conn: NonSend<rusqlite::Connection>,
    mut last_click: Local<f32>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    if keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight) {
        return; // Ctrl+click is the fork gesture
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }
    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_gt)) = camera.single() else {
        return;
    };
    let Some(world) = window
        .cursor_position()
        .and_then(|c| cam.viewport_to_world_2d(cam_gt, c).ok())
    else {
        return;
    };

    let bands = layout_bands(&model);
    let Some(band) = band_at(&bands, world) else {
        return;
    };

    // Require a double-click (≤ 0.4s) to create, matching main's empty-space create.
    let now = time.elapsed_secs();
    if now - *last_click >= 0.4 {
        *last_click = now;
        return;
    }
    *last_click = 0.0;

    let plan_id = band.plan_id;
    let day = (world.x / PIXELS_PER_DAY).round() as Day;
    let day = day.max(band.fork_day);
    let row = ((band.row0_y - world.y) / ROW_HEIGHT).round().max(0.0) as i32;

    model.add_block_to_plan(plan_id, "New Block", day, DEFAULT_DURATION, row);
    if let Err(e) = db::save_model(&conn, &model) {
        error!("save_model failed: {e}");
    }
}

/// Double-clicking a lane's name opens an inline rename.
pub fn handle_band_rename_click(
    mut egui_ctx: bevy_egui::EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    model: Res<Model>,
    mut rename: ResMut<PlanRenameState>,
    mut last_click: Local<Option<(PlanId, f32)>>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    if keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight) {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }
    let Ok(window) = windows.single() else { return };
    let Ok((cam, cam_gt)) = camera.single() else {
        return;
    };
    let Some(world) = window
        .cursor_position()
        .and_then(|c| cam.viewport_to_world_2d(cam_gt, c).ok())
    else {
        return;
    };

    let now = time.elapsed_secs();
    for band in layout_bands(&model) {
        let w = (band.name.len() as f32 * 8.0).max(48.0);
        let in_box = world.x >= band.name_x - 4.0
            && world.x <= band.name_x + w
            && (world.y - band.name_y).abs() <= ROW_HEIGHT * 0.4;
        if in_box {
            let double =
                matches!(*last_click, Some((pid, t)) if pid == band.plan_id && now - t < 0.4);
            if double {
                rename.editing = Some(band.plan_id);
                rename.buf = band.name.clone();
                *last_click = None;
            } else {
                *last_click = Some((band.plan_id, now));
            }
            return;
        }
    }
}

/// egui overlay: focused single-line field for the lane being renamed.
pub fn draw_plan_rename_overlay(
    mut contexts: bevy_egui::EguiContexts,
    mut rename: ResMut<PlanRenameState>,
    mut model: ResMut<Model>,
    conn: NonSend<rusqlite::Connection>,
) {
    let Some(plan_id) = rename.editing else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let mut commit = false;
    let mut cancel = false;
    bevy_egui::egui::Window::new("Rename plan")
        .collapsible(false)
        .resizable(false)
        .anchor(bevy_egui::egui::Align2::CENTER_TOP, [0.0, 60.0])
        .show(ctx, |ui| {
            let resp = ui.add(
                bevy_egui::egui::TextEdit::singleline(&mut rename.buf)
                    .desired_width(220.0)
                    .hint_text("Plan name"),
            );
            resp.request_focus();
            if resp.lost_focus() && ui.input(|i| i.key_pressed(bevy_egui::egui::Key::Enter)) {
                commit = true;
            }
            if ui.input(|i| i.key_pressed(bevy_egui::egui::Key::Escape)) {
                cancel = true;
            }
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    commit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if commit {
        let name = rename.buf.trim().to_string();
        if !name.is_empty() {
            if let Some(plan) = model.plans.get_mut(&plan_id) {
                plan.name = name;
            }
            if let Err(e) = db::save_model(&conn, &model) {
                error!("save_model failed: {e}");
            }
        }
        rename.editing = None;
    } else if cancel {
        rename.editing = None;
    }
}
