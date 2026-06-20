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
use bevy::window::SystemCursorIcon;

use crate::{
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    db,
    model::{Day, Model, Plan, PlanId, WorkBlockId},
};

/// Pixels from a lane block's right edge that count as the resize handle.
const EDGE_GRAB_PX: f32 = 8.0;

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

/// The currently selected lane block as `(block, plan)`, if any. Carries the
/// plan because a ghost can appear in several branches — Delete must remove it
/// from the exact lane it was selected in. Drives the selection highlight and
/// arms the Delete key for lane blocks.
#[derive(Resource, Default)]
pub struct LaneSelection(pub Option<(WorkBlockId, PlanId)>);

/// How an in-progress lane-block drag is changing the block.
#[derive(Clone, Copy, PartialEq)]
enum LaneDragMode {
    Move,
    Resize,
}

/// In-progress drag of an owned lane block.
struct LaneDragActive {
    block: WorkBlockId,
    plan: PlanId,
    mode: LaneDragMode,
    /// Cursor x minus the block's start-day x at grab time (Move only).
    grab_offset: f32,
}

#[derive(Resource, Default)]
pub struct LaneDrag {
    active: Option<LaneDragActive>,
}

/// Inline rename of an owned lane block (reuses the egui overlay pattern).
#[derive(Resource, Default)]
pub struct LaneBlockRename {
    pub editing: Option<WorkBlockId>,
    pub buf: String,
    last_click: Option<(WorkBlockId, f32)>,
}

/// One block in a lane, world coordinates.
struct BandBlock {
    id: WorkBlockId,
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
        // Drop row 0 a full row below the divider, leaving a header band for the
        // plan name with clear space above the first row of blocks.
        let row0_y = lane_top - ROW_HEIGHT;

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
                id: *id,
                cx: wb.start_day as f32 * PIXELS_PER_DAY + w * 0.5,
                cy: row0_y - wb.row as f32 * ROW_HEIGHT,
                w,
                name: wb.name.clone(),
                color: crate::blocks::block_color(wb),
                owned: !main_set.contains(id),
            });
        }

        // +2, not +1: one row to hold the last block (max_row is 0-based) and
        // one empty row of slack below it, so there's always space to click-add
        // a new row to the branch.
        let rows = (max_row + 2).max(MIN_LANE_ROWS);
        let lane_bottom = row0_y - (rows - 1) as f32 * ROW_HEIGHT - ROW_HEIGHT * 0.7;

        out.push(BandLayout {
            plan_id: branch.id,
            fork_day: fork,
            name: branch.name.clone(),
            name_x: fork as f32 * PIXELS_PER_DAY + 4.0,
            name_y: lane_top - 13.0,
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
    selection: Res<LaneSelection>,
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
    let rect = |gizmos: &mut Gizmos, b: &BandBlock, color: Color| {
        let hw = b.w * 0.5;
        let hh = GHOST_HEIGHT * 0.5;
        let tl = Vec2::new(b.cx - hw, b.cy + hh);
        let tr = Vec2::new(b.cx + hw, b.cy + hh);
        let br = Vec2::new(b.cx + hw, b.cy - hh);
        let bl = Vec2::new(b.cx - hw, b.cy - hh);
        gizmos.line_2d(tl, tr, color);
        gizmos.line_2d(tr, br, color);
        gizmos.line_2d(br, bl, color);
        gizmos.line_2d(bl, tl, color);
    };

    for band in layout_bands(&model) {
        gizmos.line_2d(
            Vec2::new(cam_x - half_w, band.lane_top),
            Vec2::new(cam_x + half_w, band.lane_top),
            divider,
        );
        for b in &band.blocks {
            // Ghosts: colored outline (transparent interior). Owned: solid bar
            // drawn as an entity, so only outline it when selected.
            if !b.owned {
                rect(&mut gizmos, b, Color::from(b.color));
            }
            if selection.0 == Some((b.id, band.plan_id)) {
                rect(&mut gizmos, b, Color::srgba(1.0, 1.0, 1.0, 0.9));
            }
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

    // A double-click on an existing lane block (ghost or owned) is a
    // rename/select, not a create.
    if lane_block_at(&model, world).is_some() {
        return;
    }

    let bands = layout_bands(&model);
    let Some(band) = band_at(&bands, world) else {
        return;
    };

    // The header strip above row 0 is reserved for the plan name — a double-click
    // there renames the plan, so don't also create a block.
    if world.y > band.row0_y + GHOST_HEIGHT * 0.5 {
        return;
    }

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

/// Outcome of an inline rename field for one frame.
enum RenameOutcome {
    Editing,
    Commit,
    Cancel,
}

/// Draws a single-line text field anchored in-place at `world_pos`, mapped to
/// the screen via the camera. Enter commits, Escape cancels, clicking away
/// commits. Shared by the plan-name and lane-block rename overlays.
fn inline_rename_field(
    ctx: &bevy_egui::egui::Context,
    id: &str,
    buf: &mut String,
    world_pos: Vec2,
    camera: &Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    keys: &ButtonInput<KeyCode>,
) -> RenameOutcome {
    if keys.just_pressed(KeyCode::Escape) {
        return RenameOutcome::Cancel;
    }
    let entered = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);

    let screen = camera
        .single()
        .ok()
        .and_then(|(cam, gt)| cam.world_to_viewport(gt, world_pos.extend(0.0)).ok())
        .map(|v| bevy_egui::egui::pos2(v.x, v.y - 10.0))
        .unwrap_or(bevy_egui::egui::pos2(60.0, 80.0));

    let mut commit = entered;
    bevy_egui::egui::Area::new(bevy_egui::egui::Id::new(id))
        .fixed_pos(screen)
        .show(ctx, |ui| {
            let resp = ui.add(
                bevy_egui::egui::TextEdit::singleline(buf)
                    .min_size(bevy_egui::egui::Vec2::new(140.0, 20.0)),
            );
            resp.request_focus();
            if resp.lost_focus() {
                commit = true;
            }
        });

    if commit {
        RenameOutcome::Commit
    } else {
        RenameOutcome::Editing
    }
}

/// In-place text field for renaming a branch, anchored at its lane name.
pub fn draw_plan_rename_overlay(
    mut contexts: bevy_egui::EguiContexts,
    mut rename: ResMut<PlanRenameState>,
    mut model: ResMut<Model>,
    conn: NonSend<rusqlite::Connection>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Some(plan_id) = rename.editing else {
        return;
    };
    let Some(pos) = layout_bands(&model)
        .iter()
        .find(|b| b.plan_id == plan_id)
        .map(|b| Vec2::new(b.name_x, b.name_y))
    else {
        rename.editing = None;
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    match inline_rename_field(&ctx, "plan_rename", &mut rename.buf, pos, &camera, &keys) {
        RenameOutcome::Editing => {}
        RenameOutcome::Commit => {
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
        }
        RenameOutcome::Cancel => rename.editing = None,
    }
}

// ── owned lane-block editing ────────────────────────────────────────────────

/// A lane block under the cursor, with the geometry needed to drag, resize, and
/// re-derive its row during a move. `owned` distinguishes the branch's own
/// blocks (editable) from inherited ghosts (selectable + removable only).
struct LaneHit {
    id: WorkBlockId,
    plan: PlanId,
    fork_day: Day,
    row0_y: f32,
    left_x: f32,
    right_x: f32,
    owned: bool,
}

/// Finds the lane block (ghost or owned) under `world`, if any.
fn lane_block_at(model: &Model, world: Vec2) -> Option<LaneHit> {
    for band in layout_bands(model) {
        for b in &band.blocks {
            let hw = b.w * 0.5;
            let hh = GHOST_HEIGHT * 0.5;
            if world.x >= b.cx - hw
                && world.x <= b.cx + hw
                && world.y >= b.cy - hh
                && world.y <= b.cy + hh
            {
                return Some(LaneHit {
                    id: b.id,
                    plan: band.plan_id,
                    fork_day: band.fork_day,
                    row0_y: band.row0_y,
                    left_x: b.cx - hw,
                    right_x: b.cx + hw,
                    owned: b.owned,
                });
            }
        }
    }
    None
}

/// The cursor hint for a lane block under `world`, mirroring main's feedback:
/// resize at the right edge of an owned block, move over its interior, and a
/// pointer over a ghost (selectable but read-only). `None` when over no block.
pub fn lane_cursor_at(model: &Model, world: Vec2) -> Option<SystemCursorIcon> {
    let hit = lane_block_at(model, world)?;
    if !hit.owned {
        return Some(SystemCursorIcon::Pointer);
    }
    if (world.x - hit.right_x).abs() <= EDGE_GRAB_PX {
        Some(SystemCursorIcon::EwResize)
    } else {
        Some(SystemCursorIcon::Move)
    }
}

/// Cursor → world helper shared by the lane interaction systems.
fn cursor_world(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform), With<Camera2d>>,
) -> Option<Vec2> {
    let window = windows.single().ok()?;
    let (cam, cam_gt) = camera.single().ok()?;
    window
        .cursor_position()
        .and_then(|c| cam.viewport_to_world_2d(cam_gt, c).ok())
}

/// Select, move, and resize owned lane blocks. Mirrors main's block drag/resize
/// but operates in lane space on the branch that owns the block:
/// - Press on an owned block selects it; near the right edge starts a resize,
///   otherwise a move. A double-click opens the rename overlay instead.
/// - Held: move slides `start_day` (clamped ≥ the fork day) and re-derives the
///   row from the cursor; resize tracks `duration_days`.
/// - Release: persist.
#[allow(clippy::too_many_arguments)]
pub fn handle_lane_block_edit(
    mut egui_ctx: bevy_egui::EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut model: ResMut<Model>,
    conn: NonSend<rusqlite::Connection>,
    mut drag: ResMut<LaneDrag>,
    mut selection: ResMut<LaneSelection>,
    mut rename: ResMut<LaneBlockRename>,
    mut main_selected: ResMut<crate::blocks::SelectedBlock>,
) {
    if rename.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }
    let Some(world) = cursor_world(&windows, &camera) else {
        return;
    };

    if mouse.just_pressed(MouseButton::Left) {
        drag.active = None;
        if keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight) {
            return; // fork gesture
        }
        if let Some(hit) = lane_block_at(&model, world) {
            // Any lane block selects (so Delete can remove a ghost from the
            // branch); lane and main selection are exclusive.
            selection.0 = Some((hit.id, hit.plan));
            main_selected.0 = None;

            // Ghosts are read-only — they track main, so no drag/resize/rename.
            if !hit.owned {
                rename.last_click = None;
                return;
            }

            // Double-click an owned block → rename; otherwise begin a drag.
            let now = time.elapsed_secs();
            let double =
                matches!(rename.last_click, Some((id, t)) if id == hit.id && now - t < 0.4);
            if double {
                rename.editing = Some(hit.id);
                rename.buf = model
                    .work_blocks
                    .get(&hit.id)
                    .map(|wb| wb.name.clone())
                    .unwrap_or_default();
                rename.last_click = None;
                return;
            }
            rename.last_click = Some((hit.id, now));

            let mode = if (world.x - hit.right_x).abs() <= EDGE_GRAB_PX {
                LaneDragMode::Resize
            } else {
                LaneDragMode::Move
            };
            drag.active = Some(LaneDragActive {
                block: hit.id,
                plan: hit.plan,
                mode,
                grab_offset: world.x - hit.left_x,
            });
        }
        return;
    }

    if mouse.pressed(MouseButton::Left) {
        let Some(a) = &drag.active else { return };
        let bands = layout_bands(&model);
        let Some(band) = bands.iter().find(|b| b.plan_id == a.plan) else {
            return;
        };
        match a.mode {
            LaneDragMode::Move => {
                let left_x = world.x - a.grab_offset;
                let day = ((left_x / PIXELS_PER_DAY).round() as Day).max(band.fork_day);
                let row = ((band.row0_y - world.y) / ROW_HEIGHT).round().max(0.0) as i32;
                model.set_block_placement(a.block, day, row);
            }
            LaneDragMode::Resize => {
                let start_x = model
                    .work_blocks
                    .get(&a.block)
                    .map(|wb| wb.start_day as f32 * PIXELS_PER_DAY)
                    .unwrap_or(0.0);
                let dur = ((world.x - start_x) / PIXELS_PER_DAY).round() as Day;
                model.set_block_duration(a.block, dur);
            }
        }
        return;
    }

    if mouse.just_released(MouseButton::Left) && drag.active.take().is_some() {
        if let Err(e) = db::save_model(&conn, &model) {
            error!("save_model failed: {e}");
        }
    }
}

/// Keeps lane and main block selection mutually exclusive: when a main block
/// becomes selected, clear the lane selection, so the Delete key targets exactly
/// one. (The lane handler already clears the main selection on a lane click.)
pub fn clear_lane_selection_on_main_select(
    main_selected: Res<crate::blocks::SelectedBlock>,
    mut lane: ResMut<LaneSelection>,
) {
    if main_selected.is_changed() && main_selected.0.is_some() {
        lane.0 = None;
    }
}

/// Delete/Backspace removes the selected lane block from its branch. For an
/// owned block this deletes the underlying WorkBlock; for a ghost it just
/// removes the membership, hiding the inherited block in this branch only (the
/// block stays in main). `Model::remove_block_from_plan` handles both.
pub fn handle_lane_block_delete(
    mut egui_ctx: bevy_egui::EguiContexts,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<LaneSelection>,
    rename: Res<LaneBlockRename>,
    mut model: ResMut<Model>,
    conn: NonSend<rusqlite::Connection>,
) {
    if rename.editing.is_some() {
        return;
    }
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        if ctx.wants_keyboard_input() {
            return;
        }
    }
    if !(keyboard.just_pressed(KeyCode::Delete) || keyboard.just_pressed(KeyCode::Backspace)) {
        return;
    }
    let Some((id, plan)) = selection.0.take() else {
        return;
    };
    model.remove_block_from_plan(plan, id);
    if let Err(e) = db::save_model(&conn, &model) {
        error!("save_model failed: {e}");
    }
}

/// In-place text field for renaming the selected owned lane block, anchored at
/// the block.
pub fn draw_lane_block_rename_overlay(
    mut contexts: bevy_egui::EguiContexts,
    mut rename: ResMut<LaneBlockRename>,
    mut model: ResMut<Model>,
    conn: NonSend<rusqlite::Connection>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Some(id) = rename.editing else {
        return;
    };
    // Anchor at the block's lane position.
    let pos = layout_bands(&model).iter().find_map(|band| {
        band.blocks
            .iter()
            .find(|b| b.id == id)
            .map(|b| Vec2::new(b.cx, b.cy))
    });
    let Some(pos) = pos else {
        rename.editing = None;
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    match inline_rename_field(&ctx, "lane_block_rename", &mut rename.buf, pos, &camera, &keys) {
        RenameOutcome::Editing => {}
        RenameOutcome::Commit => {
            let name = rename.buf.trim().to_string();
            if !name.is_empty() {
                if let Some(wb) = model.work_blocks.get_mut(&id) {
                    wb.name = name;
                }
                if let Err(e) = db::save_model(&conn, &model) {
                    error!("save_model failed: {e}");
                }
            }
            rename.editing = None;
        }
        RenameOutcome::Cancel => rename.editing = None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lane keeps an empty row of slack below its lowest block, so there's
    /// always space to click-add a new row to the branch.
    #[test]
    fn lane_has_a_clickable_gap_row_below_last_block() {
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let branch = m.fork_main(0).unwrap();
        m.add_block_to_plan(branch, "b", 0, 3, 2); // lowest block at row 2

        let bands = layout_bands(&m);
        let band = bands.iter().find(|b| b.plan_id == branch).unwrap();

        // The center of the empty row just below (row 3) must fall inside the
        // lane bounds, i.e. a click there lands in this band.
        let gap_row_y = band.row0_y - 3.0 * ROW_HEIGHT;
        assert!(gap_row_y > band.lane_bottom, "gap row is above the lane bottom");
        assert!(gap_row_y <= band.lane_top, "gap row is below the lane top");
    }

    /// The plan name sits in a header strip above row 0, clear of the
    /// block-create zone (so double-clicking the name renames, not creates).
    #[test]
    fn plan_name_is_above_the_create_zone() {
        let mut m = Model::default();
        let _main = m.create_plan("main", None);
        let branch = m.fork_main(0).unwrap();
        let bands = layout_bands(&m);
        let band = bands.iter().find(|b| b.plan_id == branch).unwrap();
        // The create zone starts at the top of row 0 (row0_y + half block); the
        // name must be above it.
        let create_zone_top = band.row0_y + GHOST_HEIGHT * 0.5;
        assert!(
            band.name_y > create_zone_top,
            "name sits in the reserved header, not the create zone"
        );
        assert!(band.name_y <= band.lane_top, "name is below the divider");
    }
}
