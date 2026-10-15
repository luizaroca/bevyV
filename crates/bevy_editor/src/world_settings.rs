//! World Settings, like Unreal's World Settings panel: rendering options that belong to the
//! level rather than to any one actor.
//!
//! Every level has one "World Settings" entity with a [`WorldSettings`] component. It is listed
//! in the Outliner and selected with the toolbar's World Settings button, so its options show
//! in the Details panel, and it is saved in the level file. The editor camera follows it.

use bevy::{
    camera::TonemappingPass,
    core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass, NormalPrepass},
    inspector::InspectorSelection,
    pbr::ScreenSpaceAmbientOcclusion,
    prelude::*,
};
use bevy_extend::ssgi::{ScreenSpaceGlobalIllumination, ScreenSpaceGlobalIlluminationPlugin};

use crate::{viewport::EditorCamera, LevelEntity};

/// Rendering options for the whole level.
#[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
#[reflect(Component, Default, Clone, PartialEq)]
pub struct WorldSettings {
    /// Screen-space ambient occlusion: darkens corners and creases. Not needed when SSGI is on,
    /// since SSGI darkens them too.
    pub ssao: bool,
    /// Screen-space global illumination: light bouncing off nearby surfaces, which tints them
    /// with each other's colors. Includes ambient occlusion.
    pub ssgi: bool,
    /// SSAO quality and radius, used while `ssao` is on.
    pub ssao_settings: ScreenSpaceAmbientOcclusion,
    /// SSGI strength, reach, denoising and debug views, used while `ssgi` is on.
    pub ssgi_settings: ScreenSpaceGlobalIllumination,
}

/// The name of the entity that holds [`WorldSettings`].
pub const WORLD_SETTINGS_NAME: &str = "World Settings";

/// Gives the level a World Settings entity if it has none, and returns it.
pub fn ensure_world_settings(world: &mut World) -> Entity {
    let mut existing = world.query_filtered::<Entity, With<WorldSettings>>();
    if let Some(entity) = existing.iter(world).next() {
        return entity;
    }
    world
        .spawn((
            Name::new(WORLD_SETTINGS_NAME),
            LevelEntity,
            WorldSettings::default(),
        ))
        .id()
}

/// Shows the World Settings in the Details panel.
pub fn select_world_settings(world: &mut World) {
    let entity = ensure_world_settings(world);
    world.resource_mut::<InspectorSelection>().0 = Some(entity);
}

/// Turns SSAO and SSGI on the editor camera on and off to match the level's settings.
fn apply_world_settings(
    settings: Query<&WorldSettings>,
    cameras: Query<Entity, With<EditorCamera>>,
    new_cameras: Query<(), Added<EditorCamera>>,
    mut applied: Local<Option<WorldSettings>>,
    mut commands: Commands,
) {
    // A deleted World Settings entity is disabled, so it no longer matches and effects turn off.
    let current = settings.iter().next().cloned().unwrap_or_default();
    if applied.as_ref() == Some(&current) && new_cameras.is_empty() {
        return;
    }

    for camera in &cameras {
        let mut camera = commands.entity(camera);
        if current.ssao {
            camera.insert(current.ssao_settings.clone());
        } else {
            camera.remove::<ScreenSpaceAmbientOcclusion>();
        }
        if current.ssgi {
            camera.insert(current.ssgi_settings.clone());
        } else {
            camera.remove::<ScreenSpaceGlobalIllumination>();
        }
        if current.ssao || current.ssgi {
            // Both read the depth and normal prepasses, which don't support MSAA.
            camera.insert(Msaa::Off);
        } else {
            // Back to the plain forward camera the editor starts with.
            camera.insert(Msaa::default()).remove::<(
                DepthPrepass,
                NormalPrepass,
                MotionVectorPrepass,
                TonemappingPass,
            )>();
        }
    }
    *applied = Some(current);
}

/// Registers World Settings and the effects they control.
pub struct WorldSettingsPlugin;

impl Plugin for WorldSettingsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ScreenSpaceGlobalIlluminationPlugin>() {
            app.add_plugins(ScreenSpaceGlobalIlluminationPlugin);
        }
        app.register_type::<WorldSettings>()
            .add_systems(PostStartup, |world: &mut World| {
                ensure_world_settings(world);
            })
            .add_systems(Update, apply_world_settings);
    }
}
