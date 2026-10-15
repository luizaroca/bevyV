//! Saving and opening levels, like Unreal's File > Save Current Level / Open Level.
//!
//! A level file holds every [`LevelEntity`] root and its descendants as a Bevy world file
//! (`.level.ron`, readable RON). Engine-computed components are left out, as are components that
//! cannot be written to a file, such as meshes and materials created in code; use
//! [`Primitive`](crate::primitives::Primitive) shapes for those.

use std::{
    any::TypeId,
    fs,
    path::{Path, PathBuf},
};

use bevy::{
    camera::{
        primitives::{Aabb, CascadesFrusta, CubemapFrusta, Frustum},
        visibility::{
            CascadesVisibleEntities, CubemapVisibleEntities, VisibleEntities, VisibleMeshEntities,
        },
    },
    ecs::{component::ComponentInfo, entity::EntityHashMap, entity_disabling::Disabled},
    gizmos::transform_gizmo::TransformGizmoFocus,
    inspector::InspectorSelection,
    platform::collections::HashSet,
    prelude::*,
    window::PrimaryWindow,
    world_serialization::{
        serde::WorldDeserializer, DynamicWorld, DynamicWorldBuilder, WorldFilter,
    },
};
use serde::de::DeserializeSeed;

use crate::{history::History, LevelEntity};

/// The file extension of level files.
pub const EXTENSION: &str = "level.ron";

/// The level being edited.
#[derive(Resource, Default)]
pub struct CurrentLevel {
    /// Where the level was last saved or opened from.
    pub path: Option<PathBuf>,
    /// The [`History::version`] at the last save, used to tell whether there are unsaved changes.
    saved_version: u64,
}

impl CurrentLevel {
    /// Whether the level has edits that are not saved.
    pub fn is_dirty(&self, history: &History) -> bool {
        history.version() != self.saved_version
    }

    /// The level's display name.
    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| {
                name.to_string_lossy()
                    .trim_end_matches(".level.ron")
                    .to_string()
            })
            .unwrap_or_else(|| "Untitled".into())
    }
}

/// Root level entities and all their descendants, leaving out deleted ones.
fn level_entities(world: &mut World) -> Vec<Entity> {
    let mut roots = world.query_filtered::<Entity, (With<LevelEntity>, Without<ChildOf>)>();
    let mut stack: Vec<Entity> = roots.iter(world).collect();
    let mut entities = Vec::new();
    while let Some(entity) = stack.pop() {
        if world.get::<Disabled>(entity).is_some() {
            continue;
        }
        entities.push(entity);
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    entities
}

/// Components that are computed at runtime and are never written to a level file.
fn computed_components() -> HashSet<TypeId> {
    [
        TypeId::of::<Children>(),
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<TransformTreeChanged>(),
        TypeId::of::<bevy::camera::primitives::Sphere>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<Aabb>(),
        TypeId::of::<Frustum>(),
        TypeId::of::<CubemapFrusta>(),
        TypeId::of::<CascadesFrusta>(),
        TypeId::of::<VisibleEntities>(),
        TypeId::of::<VisibleMeshEntities>(),
        TypeId::of::<CubemapVisibleEntities>(),
        TypeId::of::<CascadesVisibleEntities>(),
        TypeId::of::<bevy::light::Cascades>(),
        TypeId::of::<Mesh3d>(),
        TypeId::of::<MeshMaterial3d<StandardMaterial>>(),
        TypeId::of::<TransformGizmoFocus>(),
        TypeId::of::<Disabled>(),
    ]
    .into_iter()
    .collect()
}

/// Whether a single component type survives being written and read back.
fn round_trips(world: &World, entities: &[Entity], type_id: TypeId) -> bool {
    let registry = world.resource::<AppTypeRegistry>().read();
    let probe = DynamicWorldBuilder::from_world(world, &registry)
        .with_component_filter(WorldFilter::deny_all().allow_by_id(type_id))
        .extract_entities(entities.iter().copied())
        .build();
    let Ok(text) = probe.serialize(&registry) else {
        return false;
    };
    let mut asset_server = world.resource::<AssetServer>();
    let Ok(mut deserializer) = ron::de::Deserializer::from_str(&text) else {
        return false;
    };
    WorldDeserializer {
        type_registry: &registry,
        load_from_path: &mut asset_server,
    }
    .deserialize(&mut deserializer)
    .is_ok()
}

/// Writes the level to `path`.
pub fn save_level_to(world: &mut World, path: &Path) -> Result<(), String> {
    let mut entities = level_entities(world);
    // Keep the order the entities were created in, so the Outliner lists them the same way after
    // the level is opened again.
    entities.sort_unstable_by_key(|entity| entity.index());

    // Work out which component types to write.
    let computed = computed_components();
    let mut types = HashSet::new();
    for &entity in &entities {
        for component in world.entity(entity).archetype().components() {
            if let Some(type_id) = world
                .components()
                .get_info(*component)
                .and_then(ComponentInfo::type_id)
            {
                types.insert(type_id);
            }
        }
    }
    let mut filter = WorldFilter::deny_all();
    for type_id in types {
        if computed.contains(&type_id) {
            continue;
        }
        if round_trips(world, &entities, type_id) {
            filter = filter.allow_by_id(type_id);
        } else {
            let registry = world.resource::<AppTypeRegistry>().read();
            let name = registry
                .get(type_id)
                .map(|registration| registration.type_info().type_path().to_string())
                .unwrap_or_else(|| format!("{type_id:?}"));
            debug!("Not saving `{name}`: it cannot be written to a level file");
        }
    }

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let level = DynamicWorldBuilder::from_world(world, &registry)
        .with_component_filter(filter)
        .extract_entities(entities.into_iter())
        .build();
    let text = level
        .serialize(&registry)
        .map_err(|error| error.to_string())?;
    drop(registry);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, text).map_err(|error| error.to_string())?;

    let version = world.resource::<History>().version();
    let mut current = world.resource_mut::<CurrentLevel>();
    current.path = Some(path.to_path_buf());
    current.saved_version = version;
    info!("Saved level to {}", path.display());
    Ok(())
}

/// Despawns every level entity, including deleted ones kept for undo.
fn clear_level(world: &mut World) {
    let mut roots = world.query_filtered::<Entity, (With<LevelEntity>, Without<ChildOf>)>();
    let mut disabled_roots =
        world.query_filtered::<Entity, (With<LevelEntity>, Without<ChildOf>, With<Disabled>)>();
    let mut all: Vec<Entity> = roots.iter(world).collect();
    all.extend(disabled_roots.iter(world));
    for entity in all {
        world.entity_mut(entity).despawn();
    }
    world.resource_mut::<InspectorSelection>().0 = None;
    let mut history = world.resource_mut::<History>();
    history.clear();
    let version = history.version();
    world.resource_mut::<CurrentLevel>().saved_version = version;
}

/// Replaces the current level with the one in `path`.
pub fn load_level_from(world: &mut World, path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let level: DynamicWorld = {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let mut asset_server = world.resource::<AssetServer>().clone();
        let mut deserializer =
            ron::de::Deserializer::from_str(&text).map_err(|error| error.to_string())?;
        WorldDeserializer {
            type_registry: &registry,
            load_from_path: &mut asset_server,
        }
        .deserialize(&mut deserializer)
        .map_err(|error| error.to_string())?
    };

    clear_level(world);
    let mut entity_map = EntityHashMap::default();
    level
        .write_to_world(world, &mut entity_map)
        .map_err(|error| error.to_string())?;

    // `ChildOf` is written before its entity reference is remapped to the new parent, so the
    // parent's `Children` misses the child and transforms do not propagate. Inserting it again
    // runs the relationship hooks with the right parent.
    for &entity in entity_map.values() {
        if let Some(parent) = world.get::<ChildOf>(entity).map(ChildOf::parent) {
            world.entity_mut(entity).insert(ChildOf(parent));
        }
    }
    crate::world_settings::ensure_world_settings(world);
    world.resource_mut::<CurrentLevel>().path = Some(path.to_path_buf());
    info!("Opened level {}", path.display());
    Ok(())
}

/// The folder levels are saved in by default.
fn levels_folder() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join("assets")
        .join("levels")
}

fn file_dialog(world: &World) -> rfd::FileDialog {
    let current = world.resource::<CurrentLevel>();
    let folder = current
        .path
        .as_ref()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(levels_folder);
    let _ = fs::create_dir_all(&folder);
    rfd::FileDialog::new()
        .add_filter("Bevy level", &["ron"])
        .set_directory(folder)
}

fn report(result: Result<(), String>, action: &str) {
    if let Err(error) = result {
        error!("Could not {action} the level: {error}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Bevy Editor")
            .set_description(format!("Could not {action} the level:\n{error}"))
            .show();
    }
}

/// Saves the level to a file chosen by the user.
pub fn save_level_as(world: &mut World) {
    let name = format!("{}.{EXTENSION}", world.resource::<CurrentLevel>().name());
    let Some(mut path) = file_dialog(world).set_file_name(name).save_file() else {
        return;
    };
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    if !file_name.ends_with(EXTENSION) {
        let stem = file_name.trim_end_matches(".ron").to_string();
        path.set_file_name(format!("{stem}.{EXTENSION}"));
    }
    let result = save_level_to(world, &path);
    report(result, "save");
}

/// Saves the level to its file, asking for one if it has never been saved.
pub fn save_level(world: &mut World) {
    match world.resource::<CurrentLevel>().path.clone() {
        Some(path) => {
            let result = save_level_to(world, &path);
            report(result, "save");
        }
        None => save_level_as(world),
    }
}

/// Asks whether to save unsaved changes. Returns `false` if the user cancelled.
fn confirm_discard(world: &mut World) -> bool {
    if !world
        .resource::<CurrentLevel>()
        .is_dirty(world.resource::<History>())
    {
        return true;
    }
    let name = world.resource::<CurrentLevel>().name();
    let choice = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Bevy Editor")
        .set_description(format!("Save changes to {name}?"))
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show();
    match choice {
        rfd::MessageDialogResult::Yes => {
            save_level(world);
            !world
                .resource::<CurrentLevel>()
                .is_dirty(world.resource::<History>())
        }
        rfd::MessageDialogResult::No => true,
        _ => false,
    }
}

/// Opens a level chosen by the user.
pub fn open_level(world: &mut World) {
    if !confirm_discard(world) {
        return;
    }
    let Some(path) = file_dialog(world).pick_file() else {
        return;
    };
    let result = load_level_from(world, &path);
    report(result, "open");
}

/// Starts a new, empty level with a light and a floor.
pub fn new_level(world: &mut World) {
    if !confirm_discard(world) {
        return;
    }
    clear_level(world);
    world.resource_mut::<CurrentLevel>().path = None;
    world.spawn((
        Name::new("Sun"),
        LevelEntity,
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    world.spawn((
        Name::new("Floor"),
        LevelEntity,
        crate::primitives::Primitive::Plane {
            size: Vec2::splat(20.0),
        },
    ));
    crate::world_settings::ensure_world_settings(world);
}

/// Shows the level name and unsaved state in the window title, like "Bevy Editor - Arena*".
fn update_title(
    current: Res<CurrentLevel>,
    history: Res<History>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let dirty = if current.is_dirty(&history) { "*" } else { "" };
    let title = format!("Bevy Editor - {}{dirty}", current.name());
    if window.title != title {
        window.title = title;
    }
}

/// Registers level saving and loading.
pub struct LevelPlugin;

impl Plugin for LevelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CurrentLevel>()
            .add_systems(Update, update_title);
    }
}
