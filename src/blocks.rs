use bevy::prelude::*;

use crate::{constants::PIXELS_PER_DAY, model::WorkBlockId, schedule::Schedule};

const ROW_HEIGHT: f32 = 40.0;
const BLOCK_HEIGHT: f32 = 28.0;

/// HDR linear palette — one or more channels > 1.0 so the Bloom post-process fires.
const PALETTE: &[LinearRgba] = &[
    LinearRgba::new(2.0, 0.5, 0.1, 1.0), // amber
    LinearRgba::new(0.2, 1.8, 0.5, 1.0), // green
    LinearRgba::new(0.2, 0.8, 3.0, 1.0), // cyan
    LinearRgba::new(2.2, 0.3, 1.5, 1.0), // magenta
    LinearRgba::new(2.5, 1.8, 0.1, 1.0), // yellow
    LinearRgba::new(0.5, 0.5, 3.0, 1.0), // blue
];

/// Marker: this sprite visualises one ScheduledBlock.
#[derive(Component)]
pub struct BlockSprite {
    pub work_block_id: WorkBlockId,
    pub row: usize,
}

/// Spawns (or re-spawns) one `Sprite` per `ScheduledBlock`.
/// Row is assigned by ascending `start_day`, then `WorkBlockId` for stability.
/// Should run once after the `Schedule` resource is first available, and
/// again whenever the schedule changes.
pub fn spawn_block_sprites(
    mut commands: Commands,
    schedule: Res<Schedule>,
    existing: Query<Entity, With<BlockSprite>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let mut ordered: Vec<_> = schedule.blocks.values().collect();
    ordered.sort_by(|a, b| {
        a.start_day
            .partial_cmp(&b.start_day)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.work_block_id.0.cmp(&b.work_block_id.0))
    });

    for (row, block) in ordered.iter().enumerate() {
        let width = block.duration_days * PIXELS_PER_DAY;
        // Sprite origin is at its center in Bevy 2D.
        let x = block.start_day * PIXELS_PER_DAY + width * 0.5;
        let y = -(row as f32) * ROW_HEIGHT;

        commands.spawn((
            BlockSprite { work_block_id: block.work_block_id, row },
            Sprite {
                color: Color::from(PALETTE[row % PALETTE.len()]),
                custom_size: Some(Vec2::new(width, BLOCK_HEIGHT)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
        ));
    }
}

/// Recomputes `Transform` and `Sprite::custom_size` every frame from the
/// current `Schedule` so the view stays in sync when the schedule changes.
pub fn sync_block_sprites(
    schedule: Res<Schedule>,
    mut query: Query<(&BlockSprite, &mut Transform, &mut Sprite)>,
) {
    for (block_sprite, mut transform, mut sprite) in &mut query {
        let Some(block) = schedule.blocks.get(&block_sprite.work_block_id) else {
            continue;
        };
        let width = block.duration_days * PIXELS_PER_DAY;
        let x = block.start_day * PIXELS_PER_DAY + width * 0.5;
        let y = -(block_sprite.row as f32) * ROW_HEIGHT;
        transform.translation.x = x;
        transform.translation.y = y;
        sprite.custom_size = Some(Vec2::new(width, BLOCK_HEIGHT));
    }
}
