//! Headless UI-integration harness (#336, phase 1): runs the real gesture
//! systems in a real Bevy `App` — no window, no renderer — driving them by
//! writing `ButtonInput` directly and asserting on the resulting ECS state
//! (model mutations, selection, deferred-save marks).
//!
//! Phase 1 covers the keyboard-driven canvas gestures end-to-end: the egui
//! guard (`ctx_mut()`) errs harmlessly with no egui context entity, and
//! pointer-position lookups fall back exactly as on a cursor-less frame.
//! Phase 2 (pointer gestures) needs a mocked camera/viewport so
//! `viewport_to_world_2d` resolves — tracked in #336.

use bevy::prelude::*;

use crate::{blocks, db, model, schedule};

/// A minimal app wired with the keyboard gesture systems and every resource
/// they read, mirroring main()'s registration order for these systems.
pub fn keyboard_test_app() -> App {
    let mut app = App::new();
    // EguiContexts validates this resource even with no context entity.
    app.init_resource::<bevy_egui::EguiUserTextures>();
    app.insert_resource(ButtonInput::<KeyCode>::default())
        .insert_resource(ButtonInput::<MouseButton>::default())
        .insert_resource(model::Model::default())
        .insert_resource(db::SaveRequest::default())
        .insert_resource(blocks::SelectedBlock::default())
        .insert_resource(blocks::SelectedDependency::default())
        .insert_resource(blocks::SelectedBlocks::default())
        .insert_resource(blocks::NameEditState::default())
        .insert_resource(blocks::UndoStack::default())
        .insert_resource(blocks::Clipboard::default())
        .insert_resource(schedule::DrillScope::default())
        .add_systems(Update, blocks::handle_block_delete)
        .add_systems(Update, blocks::handle_undo)
        .add_systems(Update, blocks::handle_copy)
        .add_systems(Update, blocks::handle_paste.after(blocks::handle_copy));
    app
}

/// Presses `keys` for exactly one frame: runs one update with them down
/// (`just_pressed` true), then clears the just-pressed edge so the next
/// frame sees them merely held — matching InputPlugin's per-frame behavior.
pub fn press_keys(app: &mut App, keys: &[KeyCode]) {
    {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        for k in keys {
            input.press(*k);
        }
    }
    app.update();
    let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    input.clear();
    for k in keys {
        input.release(*k);
    }
    input.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorkBlockId;

    fn seed_block(app: &mut App, name: &str, start: i32, dur: i32) -> WorkBlockId {
        let mut m = app.world_mut().resource_mut::<model::Model>();
        let plan = m
            .main_plan_id()
            .unwrap_or_else(|| m.create_plan("main", None));
        m.add_block_to_plan(plan, name, start, dur, 0)
    }

    fn select(app: &mut App, id: WorkBlockId) {
        app.world_mut().resource_mut::<blocks::SelectedBlock>().0 = Some(id);
        let mut set = app.world_mut().resource_mut::<blocks::SelectedBlocks>();
        set.0.clear();
        set.0.insert(id);
    }

    #[test]
    fn delete_key_removes_selected_block_and_marks_save() {
        let mut app = keyboard_test_app();
        let id = seed_block(&mut app, "Doomed", 0, 5);
        select(&mut app, id);

        press_keys(&mut app, &[KeyCode::Delete]);

        let world = app.world();
        assert!(
            !world
                .resource::<model::Model>()
                .work_blocks
                .contains_key(&id),
            "Delete removes the selected block through the real system"
        );
        assert!(
            world.resource::<db::SaveRequest>().0,
            "the mutation marks the deferred save"
        );
        assert_eq!(world.resource::<blocks::SelectedBlock>().0, None);
    }

    #[test]
    fn undo_restores_a_deleted_block() {
        let mut app = keyboard_test_app();
        let id = seed_block(&mut app, "Phoenix", 3, 4);
        select(&mut app, id);
        press_keys(&mut app, &[KeyCode::Delete]);
        assert!(!app
            .world()
            .resource::<model::Model>()
            .work_blocks
            .contains_key(&id));

        press_keys(&mut app, &[KeyCode::SuperLeft, KeyCode::KeyZ]);

        let m = app.world().resource::<model::Model>();
        let restored = m.work_blocks.get(&id).expect("Ctrl/Cmd+Z restores it");
        assert_eq!(restored.name, "Phoenix");
        assert_eq!((restored.start_day, restored.duration_days), (3, 4));
    }

    #[test]
    fn copy_paste_duplicates_the_selection_with_new_id() {
        let mut app = keyboard_test_app();
        let id = seed_block(&mut app, "Original", 2, 5);
        select(&mut app, id);

        press_keys(&mut app, &[KeyCode::SuperLeft, KeyCode::KeyC]);
        press_keys(&mut app, &[KeyCode::SuperLeft, KeyCode::KeyV]);

        let m = app.world().resource::<model::Model>();
        assert_eq!(m.work_blocks.len(), 2, "paste creates a real new block");
        let (new_id, copy) = m.work_blocks.iter().find(|(i, _)| **i != id).unwrap();
        assert_eq!(copy.name, "Original");
        assert_ne!(*new_id, id, "paste re-mints the id");
        assert!(app.world().resource::<db::SaveRequest>().0);
    }

    #[test]
    fn keys_are_inert_while_a_rename_is_in_flight() {
        let mut app = keyboard_test_app();
        let id = seed_block(&mut app, "Safe", 0, 5);
        select(&mut app, id);
        app.world_mut()
            .resource_mut::<blocks::NameEditState>()
            .editing = Some(id);

        press_keys(&mut app, &[KeyCode::Delete]);

        assert!(
            app.world()
                .resource::<model::Model>()
                .work_blocks
                .contains_key(&id),
            "Delete must not fire while the name editor owns the keyboard"
        );
    }
}
