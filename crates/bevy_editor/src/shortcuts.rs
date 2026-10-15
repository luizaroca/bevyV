//! Unreal's default editor keyboard shortcuts.
//!
//! | Key | Action |
//! | --- | --- |
//! | Q / W / E / R | Select / Move / Rotate / Scale |
//! | Space | Cycle Move, Rotate, Scale |
//! | Ctrl + \` | Toggle world / local gizmo space |
//! | F | Frame selection |
//! | Delete | Delete selection |
//! | Ctrl + Z | Undo |
//! | Ctrl + Y, Ctrl + Shift + Z | Redo |
//! | Ctrl + N | New level |
//! | Ctrl + O | Open level |
//! | Ctrl + S, Ctrl + Shift + S | Save level, Save level as |
//! | Alt + P | Play / Stop |
//! | Escape | Stop playing, or clear the selection |
//!
//! Shortcuts are ignored while a text field has focus or while flying with the right mouse button.

use bevy::{
    gizmos::transform_gizmo::{TransformGizmoMode, TransformGizmoSettings, TransformGizmoSpace},
    input_focus::InputFocus,
    inspector::InspectorSelection,
    prelude::*,
    text::EditableText,
};

use crate::{
    history::{self, DeleteEntities},
    level,
    play::{self, EditorMode},
    selection::GizmoVisible,
    viewport,
};

/// Switches the gizmo to `mode` and shows it.
pub fn set_gizmo_mode(world: &mut World, mode: TransformGizmoMode) {
    world.resource_mut::<TransformGizmoSettings>().mode = mode;
    world.resource_mut::<GizmoVisible>().0 = true;
}

/// Hides the gizmo, like Unreal's Select mode.
pub fn set_select_mode(world: &mut World) {
    world.resource_mut::<GizmoVisible>().0 = false;
}

/// Toggles the gizmo between world and local space.
pub fn toggle_gizmo_space(world: &mut World) {
    let mut settings = world.resource_mut::<TransformGizmoSettings>();
    settings.space = match settings.space {
        TransformGizmoSpace::World => TransformGizmoSpace::Local,
        TransformGizmoSpace::Local => TransformGizmoSpace::World,
    };
}

/// Deletes the selected entity, as an undoable step.
pub fn delete_selection(world: &mut World) {
    let Some(selected) = world.resource::<InspectorSelection>().0 else {
        return;
    };
    world.resource_mut::<InspectorSelection>().0 = None;
    let command = DeleteEntities::apply(world, vec![selected]);
    world.resource_mut::<history::History>().push(command);
}

fn text_field_focused(world: &World) -> bool {
    world
        .get_resource::<InputFocus>()
        .and_then(InputFocus::get)
        .is_some_and(|entity| world.get::<EditableText>(entity).is_some())
}

fn handle_shortcuts(world: &mut World) {
    if text_field_focused(world)
        || world
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Right)
    {
        return;
    }
    let keys = world.resource::<ButtonInput<KeyCode>>();
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let pressed: Vec<KeyCode> = keys.get_just_pressed().copied().collect();

    for key in pressed {
        match (key, ctrl, alt) {
            (KeyCode::KeyZ, true, false) if shift => history::redo(world),
            (KeyCode::KeyZ, true, false) => history::undo(world),
            (KeyCode::KeyY, true, false) => history::redo(world),
            (KeyCode::KeyS, true, false) if shift => level::save_level_as(world),
            (KeyCode::KeyS, true, false) => level::save_level(world),
            (KeyCode::KeyO, true, false) => level::open_level(world),
            (KeyCode::KeyN, true, false) => level::new_level(world),
            (KeyCode::Backquote, true, false) => toggle_gizmo_space(world),
            (KeyCode::KeyP, false, true) => play::toggle_play(world),
            (KeyCode::KeyQ, false, false) => set_select_mode(world),
            (KeyCode::KeyW, false, false) => set_gizmo_mode(world, TransformGizmoMode::Translate),
            (KeyCode::KeyE, false, false) => set_gizmo_mode(world, TransformGizmoMode::Rotate),
            (KeyCode::KeyR, false, false) => set_gizmo_mode(world, TransformGizmoMode::Scale),
            (KeyCode::Space, false, false) => {
                let next = match world.resource::<TransformGizmoSettings>().mode {
                    TransformGizmoMode::Translate => TransformGizmoMode::Rotate,
                    TransformGizmoMode::Rotate => TransformGizmoMode::Scale,
                    TransformGizmoMode::Scale => TransformGizmoMode::Translate,
                };
                set_gizmo_mode(world, next);
            }
            (KeyCode::KeyF, false, false) => viewport::frame_selection(world),
            (KeyCode::Delete, false, false) => delete_selection(world),
            (KeyCode::Escape, false, false) => {
                if *world.resource::<State<EditorMode>>().get() == EditorMode::Playing {
                    play::toggle_play(world);
                } else {
                    world.resource_mut::<InspectorSelection>().0 = None;
                }
            }
            _ => {}
        }
    }
}

/// Registers the keyboard shortcuts.
pub struct ShortcutsPlugin;

impl Plugin for ShortcutsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, handle_shortcuts);
    }
}
