//! A scene editor for Bevy, modelled on the Unreal Engine editor.
//!
//! The editor runs inside the app being edited, the way Unreal's editor and Play-In-Editor
//! share a process. Add [`EditorPlugin`] to an app that already has `DefaultPlugins`, and mark
//! the entities that make up the level with [`LevelEntity`] so the Outliner lists them.
//!
//! The editor is made of:
//! - [`layout`]: the editor window — menu bar, toolbar, viewport, Outliner, Details panel and
//!   status bar.
//! - [`viewport`]: the level viewport camera with Unreal-style navigation.
//! - [`selection`]: click selection in the viewport, kept in sync with the Outliner and the gizmo.
//! - [`history`]: the undo/redo stack (Unreal's transaction system).
//! - [`shortcuts`]: Unreal's default keyboard shortcuts.
//! - [`play`]: Play/Stop, which runs gameplay systems and restores the level afterwards.
//! - [`level`]: saving and opening level files.
//! - [`primitives`]: basic shapes that can be saved in a level.
//! - [`details`]: the Details panel's component order.

pub mod details;
pub mod history;
pub mod layout;
pub mod level;
pub mod play;
pub mod primitives;
pub mod selection;
pub mod shortcuts;
pub mod viewport;

use bevy::{
    dev_tools::infinite_grid::InfiniteGridPlugin,
    ecs::world::EntityRef,
    feathers::{dark_theme::create_dark_theme, theme::UiTheme, FeathersPlugins},
    gizmos::transform_gizmo::TransformGizmoPlugin,
    inspector::{InspectorPlugin, InspectorRootFilter},
    picking::mesh_picking::MeshPickingPlugin,
    prelude::*,
};

/// Marks an entity as part of the level being edited.
///
/// Root entities with this marker are listed in the Outliner, can be selected in the viewport,
/// and are saved and restored by Play/Stop. Their descendants are included automatically.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default, Clone)]
pub struct LevelEntity;

/// Marks entities that belong to the editor itself (its camera, UI and helpers).
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default, Clone)]
pub struct EditorEntity;

/// Adds the editor to an app.
pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        // `FeathersPlugins` is a plugin group, so the app must not add it as well.
        app.add_plugins(FeathersPlugins);
        if !app.is_plugin_added::<MeshPickingPlugin>() {
            app.add_plugins(MeshPickingPlugin);
        }
        if !app.is_plugin_added::<TransformGizmoPlugin>() {
            app.add_plugins(TransformGizmoPlugin);
        }
        if !app.is_plugin_added::<InfiniteGridPlugin>() {
            app.add_plugins(InfiniteGridPlugin);
        }

        app.add_plugins((
            InspectorPlugin,
            history::HistoryPlugin,
            selection::SelectionPlugin,
            viewport::ViewportPlugin,
            layout::LayoutPlugin,
            shortcuts::ShortcutsPlugin,
            play::PlayPlugin,
            level::LevelPlugin,
            primitives::PrimitivesPlugin,
            details::DetailsPlugin,
        ))
        .register_type::<LevelEntity>()
        .register_type::<EditorEntity>()
        .insert_resource(UiTheme(create_dark_theme()))
        .insert_resource(InspectorRootFilter(Some(is_level_root)));
    }
}

/// The Outliner only lists the level, the way Unreal's World Outliner lists actors.
fn is_level_root(entity: &EntityRef) -> bool {
    entity.contains::<LevelEntity>()
}

/// Whether `entity` is, or descends from, a [`LevelEntity`].
pub fn is_in_level(
    entity: Entity,
    levels: &Query<(), With<LevelEntity>>,
    parents: &Query<&ChildOf>,
) -> bool {
    let mut current = entity;
    loop {
        if levels.contains(current) {
            return true;
        }
        match parents.get(current) {
            Ok(child_of) => current = child_of.parent(),
            Err(_) => return false,
        }
    }
}
