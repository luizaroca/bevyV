//! Play and Stop, like Unreal's "Simulate" mode.
//!
//! While playing, systems gated with [`in_play_mode`] run. Pressing Stop (or Escape) restores
//! every level entity's transform, removes level entities spawned during play, and restores the
//! undo history to what it was before Play was pressed.

use bevy::{ecs::entity::EntityHashSet, prelude::*};

use crate::{history::History, is_in_level, LevelEntity};

/// Whether the editor is editing the level or playing it.
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditorMode {
    /// Editing: gameplay systems are paused.
    #[default]
    Editing,
    /// Playing: gameplay systems run.
    Playing,
}

/// Run condition for gameplay systems: they only run between Play and Stop.
pub fn in_play_mode(mode: Res<State<EditorMode>>) -> bool {
    *mode.get() == EditorMode::Playing
}

/// The level as it was when Play was pressed.
#[derive(Resource, Default)]
struct PlaySnapshot {
    entities: EntityHashSet,
    transforms: Vec<(Entity, Transform)>,
    history: Option<History>,
}

/// Toggles between Play and Stop.
pub fn toggle_play(world: &mut World) {
    let next = match world.resource::<State<EditorMode>>().get() {
        EditorMode::Editing => EditorMode::Playing,
        EditorMode::Playing => EditorMode::Editing,
    };
    world.resource_mut::<NextState<EditorMode>>().set(next);
}

fn take_snapshot(
    all: Query<Entity>,
    transforms: Query<(Entity, &Transform)>,
    levels: Query<(), With<LevelEntity>>,
    parents: Query<&ChildOf>,
    mut history: ResMut<History>,
    mut snapshot: ResMut<PlaySnapshot>,
) {
    snapshot.entities = all.iter().collect();
    snapshot.transforms = transforms
        .iter()
        .filter(|(entity, _)| is_in_level(*entity, &levels, &parents))
        .map(|(entity, transform)| (entity, *transform))
        .collect();
    snapshot.history = Some(core::mem::take(&mut *history));
    info!("Play");
}

fn restore_snapshot(
    all: Query<Entity>,
    mut transforms: Query<&mut Transform>,
    levels: Query<(), With<LevelEntity>>,
    parents: Query<&ChildOf>,
    mut history: ResMut<History>,
    mut snapshot: ResMut<PlaySnapshot>,
    mut commands: Commands,
) {
    for entity in &all {
        if !snapshot.entities.contains(&entity) && is_in_level(entity, &levels, &parents) {
            commands.entity(entity).try_despawn();
        }
    }
    for (entity, saved) in snapshot.transforms.drain(..) {
        if let Ok(mut transform) = transforms.get_mut(entity)
            && *transform != saved
        {
            *transform = saved;
        }
    }
    if let Some(saved) = snapshot.history.take() {
        *history = saved;
    }
    snapshot.entities.clear();
    info!("Stop");
}

/// Registers Play and Stop.
pub struct PlayPlugin;

impl Plugin for PlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<EditorMode>()
            .init_resource::<PlaySnapshot>()
            .add_systems(OnEnter(EditorMode::Playing), take_snapshot)
            .add_systems(OnExit(EditorMode::Playing), restore_snapshot);
    }
}
