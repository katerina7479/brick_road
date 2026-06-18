//! Parallel "band" rendering for non-active plans.
//!
//! Only one plan is edited at a time (the *active* plan, identified by
//! `Schedule.plan_id`), shown full-size in the main timeline. Every other plan
//! in the same world is drawn as a compact horizontal band below the active
//! content: its effective blocks rendered as dimmed mini-bars at their real
//! day positions, with the plan's name on the left and a diverging connector
//! from its branch point. Clicking a band makes that plan active ("switch to
//! it"), so a branch and its parent trade places.
//!
//! Bands are world-space (they pan and zoom with the day grid), so a band lines
//! up day-for-day under the active timeline. They are rebuilt from scratch
//! whenever the model or the active plan changes — there are only a handful, so
//! a full despawn/respawn is cheaper than incremental reconciliation.

use bevy::prelude::*;
use bevy::sprite::Anchor;
use std::collections::HashMap;

use crate::{
    blocks,
    constants::{PIXELS_PER_DAY, ROW_HEIGHT},
    db, graph,
    model::{self, Day, Model, PlanId, WorkBlockId},
    schedule,
};

/// Vertical gap between the bottom of the active plan's content and the first
/// band lane.
const BAND_GAP: f32 = 70.0;
/// Vertical distance between successive band lanes.
const BAND_PITCH: f32 = 60.0;
/// Height of a band's mini-block bars.
pub const BAND_BLOCK_HEIGHT: f32 = 16.0;
/// Extra world-space width left of a band's first block, treated as part of the
/// band's clickable area so the name label is also a switch target.
const BAND_LABEL_HIT_WIDTH: f32 = 140.0;

/// One block as it appears in a band: just enough to place a mini-bar.
pub struct BandBlock {
    pub id: WorkBlockId,
    pub start_day: Day,
    pub duration_days: Day,
}

/// Computed geometry for one non-active plan's band lane.
pub struct BandLayout {
    pub plan_id: PlanId,
    /// 0-based lane index from the top of the band strip (drives palette + Y).
    pub index: usize,
    /// World-space Y of the band's horizontal centerline.
    pub center_y: f32,
    pub name: String,
    pub branch_start_day: Option<Day>,
    /// Day extent of the band's blocks (for the clickable rect + connector).
    pub day_min: Day,
    pub day_max: Day,
    pub blocks: Vec<BandBlock>,
}

/// Marker: this sprite is one mini-bar inside a plan band.
#[derive(Component)]
pub struct BandSprite {
    pub plan_id: PlanId,
    pub work_block_id: WorkBlockId,
}

/// Marker: this is a band's plan-name label.
#[derive(Component)]
pub struct BandLabel {
    pub plan_id: PlanId,
}

/// Tracks the entities currently rendering each band so they can be despawned
/// on the next rebuild.
#[derive(Resource, Default)]
pub struct BandSpriteMap {
    pub blocks: HashMap<(PlanId, WorkBlockId), Entity>,
    pub labels: HashMap<PlanId, Entity>,
}

/// World-space Y just below the active plan's lowest block — where the band
/// strip begins. Bands stack downward from here.
fn band_strip_top(model: &Model, active_id: PlanId) -> f32 {
    let mut min_y = 0.0_f32;
    for id in model.effective_root_blocks(active_id) {
        if let Some(wb) = model.work_blocks.get(&id) {
            if wb.duration_days <= 0 {
                continue;
            }
            let y = -(wb.row as f32) * ROW_HEIGHT - ROW_HEIGHT * 0.5;
            if y < min_y {
                min_y = y;
            }
        }
    }
    min_y - BAND_GAP
}

/// Builds the band layout for every plan in the active plan's world except the
/// active one, ordered by plan id for stable lanes and palette assignment.
pub fn compute_band_layout(model: &Model, active_id: PlanId) -> Vec<BandLayout> {
    let world = model.plans.get(&active_id).map(|p| p.world_id);
    let strip_top = band_strip_top(model, active_id);

    let mut plans: Vec<&model::Plan> = model
        .plans
        .values()
        .filter(|p| p.id != active_id && Some(p.world_id) == world)
        .collect();
    plans.sort_by_key(|p| p.id.0);

    let mut out = Vec::with_capacity(plans.len());
    for (index, plan) in plans.iter().enumerate() {
        let mut blocks = Vec::new();
        let mut day_min = Day::MAX;
        let mut day_max = Day::MIN;
        for id in model.effective_root_blocks(plan.id) {
            let Some(wb) = model.work_blocks.get(&id) else {
                continue;
            };
            if wb.duration_days <= 0 {
                continue;
            }
            day_min = day_min.min(wb.start_day);
            day_max = day_max.max(wb.start_day + wb.duration_days);
            blocks.push(BandBlock {
                id,
                start_day: wb.start_day,
                duration_days: wb.duration_days,
            });
        }
        out.push(BandLayout {
            plan_id: plan.id,
            index,
            center_y: strip_top - index as f32 * BAND_PITCH,
            name: plan.name.clone(),
            branch_start_day: plan.branch_start_day,
            day_min: if day_min == Day::MAX { 0 } else { day_min },
            day_max: if day_max == Day::MIN { 0 } else { day_max },
            blocks,
        });
    }
    out
}

/// Dimmed fill color for band `index`, drawn from the branch palette.
fn band_color(index: usize) -> Color {
    let c = blocks::BRANCH_PALETTE[index % blocks::BRANCH_PALETTE.len()];
    Color::from(LinearRgba::new(
        c.red * 0.6,
        c.green * 0.6,
        c.blue * 0.6,
        0.85,
    ))
}

/// Rebuilds all band sprites + labels when the model or active plan changes.
pub fn render_bands(
    mut commands: Commands,
    model: Res<Model>,
    schedule: Res<schedule::Schedule>,
    mut map: ResMut<BandSpriteMap>,
) {
    if !model.is_changed() && !schedule.is_changed() {
        return;
    }

    for (_, entity) in map.blocks.drain() {
        commands.entity(entity).despawn();
    }
    for (_, entity) in map.labels.drain() {
        commands.entity(entity).despawn();
    }

    for band in compute_band_layout(&model, schedule.plan_id) {
        let color = band_color(band.index);
        for b in &band.blocks {
            let width = (b.duration_days as f32 * PIXELS_PER_DAY).max(1.0);
            let x = b.start_day as f32 * PIXELS_PER_DAY + width * 0.5;
            let entity = commands
                .spawn((
                    BandSprite {
                        plan_id: band.plan_id,
                        work_block_id: b.id,
                    },
                    Sprite {
                        color,
                        custom_size: Some(Vec2::new(width, BAND_BLOCK_HEIGHT)),
                        ..default()
                    },
                    Transform::from_xyz(x, band.center_y, -0.5),
                ))
                .id();
            map.blocks.insert((band.plan_id, b.id), entity);
        }

        // Plan name to the left of the band's first block, right-aligned so it
        // tucks up against the band. Doubles as a switch target (see hit rect).
        let label_x = band.day_min as f32 * PIXELS_PER_DAY - 10.0;
        let entity = commands
            .spawn((
                BandLabel {
                    plan_id: band.plan_id,
                },
                Text2d::new(band.name.clone()),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgba(0.85, 0.88, 0.96, 0.9)),
                Anchor::CENTER_RIGHT,
                Transform::from_xyz(label_x, band.center_y, -0.4),
            ))
            .id();
        map.labels.insert(band.plan_id, entity);
    }
}

/// Draws a diverging connector from each band's branch point up to the active
/// timeline baseline, signalling where the branch forks off.
pub fn draw_band_connectors(
    mut gizmos: Gizmos,
    model: Res<Model>,
    schedule: Res<schedule::Schedule>,
) {
    let strip_top = band_strip_top(&model, schedule.plan_id);
    for band in compute_band_layout(&model, schedule.plan_id) {
        let Some(branch_day) = band.branch_start_day else {
            continue;
        };
        let x = branch_day as f32 * PIXELS_PER_DAY;
        let c = blocks::BRANCH_PALETTE[band.index % blocks::BRANCH_PALETTE.len()];
        let color = Color::from(LinearRgba::new(c.red * 0.7, c.green * 0.7, c.blue * 0.7, 0.5));
        // Vertical drop from the active baseline to the band centerline, then a
        // short elbow toward the band's first block.
        gizmos.line_2d(
            Vec2::new(x, strip_top),
            Vec2::new(x, band.center_y),
            color,
        );
        gizmos.line_2d(
            Vec2::new(x, band.center_y),
            Vec2::new(band.day_min as f32 * PIXELS_PER_DAY, band.center_y),
            color,
        );
    }
}

/// Left-click on a band switches the active plan to it ("switch to it"). Plain
/// left-click only — Ctrl+click is the fork gesture, handled elsewhere.
pub fn handle_band_click(
    mut egui_ctx: bevy_egui::EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    model: Res<Model>,
    mut schedule: ResMut<schedule::Schedule>,
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
    let Ok((cam, cam_gt)) = camera.single() else { return };
    let Some(world) = window
        .cursor_position()
        .and_then(|cursor| cam.viewport_to_world_2d(cam_gt, cursor).ok())
    else {
        return;
    };

    for band in compute_band_layout(&model, schedule.plan_id) {
        // Switch target is the band's blocks (not the name label, which is
        // reserved for double-click rename). Empty bands get a minimum width so
        // they remain clickable.
        let x0 = band.day_min as f32 * PIXELS_PER_DAY;
        let x1 = (band.day_max as f32 * PIXELS_PER_DAY).max(x0 + 40.0);
        let y0 = band.center_y - BAND_BLOCK_HEIGHT;
        let y1 = band.center_y + BAND_BLOCK_HEIGHT;
        if world.x >= x0 && world.x <= x1 && world.y >= y0 && world.y <= y1 {
            switch_active_plan(&model, &mut schedule, band.plan_id);
            return;
        }
    }
}

/// Makes `plan_id` the active plan and rebuilds the `Schedule` resource for it
/// (graph + forward pass over the plan's effective blocks). Mutating the
/// resource marks it changed, so `update_visible_blocks`, the band renderer and
/// the timeline renderers all pick up the switch on the next tick. Falls back
/// to an empty schedule if the plan is missing or its graph has a cycle.
pub fn switch_active_plan(model: &Model, schedule: &mut schedule::Schedule, plan_id: PlanId) {
    if let Some(plan) = model.plans.get(&plan_id) {
        let graph = graph::build_graph(model, plan);
        *schedule = schedule::forward_pass(model, plan, &graph)
            .unwrap_or_else(|_| schedule::Schedule::new(plan_id));
    } else {
        *schedule = schedule::Schedule::new(plan_id);
    }
}

/// State for inline renaming of a band's plan via double-click on its label.
#[derive(Resource, Default)]
pub struct BandRenameState {
    pub editing: Option<PlanId>,
    pub buf: String,
    /// (plan_id, elapsed_secs) of the most recent click on a band label, for
    /// double-click detection.
    last_click: Option<(PlanId, f32)>,
}

/// Double-clicking a band label opens an inline rename; tracked here so the
/// egui overlay can show a focused text field.
pub fn handle_band_label_doubleclick(
    mut egui_ctx: bevy_egui::EguiContexts,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    model: Res<Model>,
    schedule: Res<schedule::Schedule>,
    mut rename: ResMut<BandRenameState>,
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
    let Ok((cam, cam_gt)) = camera.single() else { return };
    let Some(world) = window
        .cursor_position()
        .and_then(|cursor| cam.viewport_to_world_2d(cam_gt, cursor).ok())
    else {
        return;
    };

    let now = time.elapsed_secs();
    for band in compute_band_layout(&model, schedule.plan_id) {
        // The label-hit zone is the left padding of the band's clickable rect.
        let x0 = band.day_min as f32 * PIXELS_PER_DAY - BAND_LABEL_HIT_WIDTH;
        let x1 = band.day_min as f32 * PIXELS_PER_DAY;
        let y0 = band.center_y - BAND_BLOCK_HEIGHT;
        let y1 = band.center_y + BAND_BLOCK_HEIGHT;
        if world.x >= x0 && world.x <= x1 && world.y >= y0 && world.y <= y1 {
            let double = matches!(rename.last_click, Some((pid, t)) if pid == band.plan_id && now - t < 0.4);
            if double {
                rename.editing = Some(band.plan_id);
                rename.buf = band.name.clone();
                rename.last_click = None;
            } else {
                rename.last_click = Some((band.plan_id, now));
            }
            return;
        }
    }
}

/// egui overlay: a focused single-line field for renaming the band being
/// edited. Enter commits + saves; Escape cancels.
pub fn draw_band_rename_overlay(
    mut contexts: bevy_egui::EguiContexts,
    mut rename: ResMut<BandRenameState>,
    mut model: ResMut<Model>,
    conn: NonSend<rusqlite::Connection>,
) {
    let Some(plan_id) = rename.editing else { return };
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut commit = false;
    let mut cancel = false;
    bevy_egui::egui::Window::new("Rename branch")
        .collapsible(false)
        .resizable(false)
        .anchor(bevy_egui::egui::Align2::CENTER_TOP, [0.0, 60.0])
        .show(ctx, |ui| {
            let resp = ui.add(
                bevy_egui::egui::TextEdit::singleline(&mut rename.buf)
                    .desired_width(220.0)
                    .hint_text("Branch name"),
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
