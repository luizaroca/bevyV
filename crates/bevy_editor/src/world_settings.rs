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
use bevy_extend::{
    ssgi::{ScreenSpaceGlobalIllumination, ScreenSpaceGlobalIlluminationPlugin},
    ssr::{ExtendScreenSpaceReflections, ExtendScreenSpaceReflectionsPlugin},
};

use crate::{viewport::EditorCamera, LevelEntity};

/// Rendering options for the whole level.
#[derive(Component, Reflect, Clone, PartialEq, Debug)]
#[reflect(Component, Default, Clone, PartialEq)]
pub struct WorldSettings {
    /// Brightness of the light that reaches every surface from all around (Bevy's ambient
    /// light), in candela per square meter.
    #[reflect(default = "default_ambient_brightness")]
    pub ambient_brightness: f32,
    /// Screen-space ambient occlusion: darkens corners and creases. It darkens ambient light, and
    /// lamp light too while `ssao_settings.direct_light` is on (the default).
    pub ssao: bool,
    /// Screen-space global illumination: light bouncing off nearby surfaces, which tints them
    /// with each other's colors. Includes ambient occlusion.
    pub ssgi: bool,
    /// Screen-space reflections: smooth surfaces (roughness below 0.6) reflect what is on
    /// screen, including the sky. Elsewhere they keep reflecting the environment.
    #[reflect(default)]
    pub reflections: bool,
    /// Show see-through objects (alpha below 1) in reflections, while `reflections` is on. Costs
    /// an extra pass over them. This replaces `reflections_settings.glass`.
    #[reflect(default = "default_true")]
    pub transparent_reflections: bool,
    /// SSAO quality, radius and intensity, used while `ssao` is on.
    pub ssao_settings: ScreenSpaceAmbientOcclusion,
    /// SSGI strength, reach, denoising and debug views, used while `ssgi` is on.
    pub ssgi_settings: ScreenSpaceGlobalIllumination,
    /// Reflection roughness limit, ray length and steps, denoising and debug views, used while
    /// `reflections` is on.
    #[reflect(default)]
    pub reflections_settings: ExtendScreenSpaceReflections,
}

fn default_true() -> bool {
    true
}

fn default_ambient_brightness() -> f32 {
    GlobalAmbientLight::default().brightness
}

impl Default for WorldSettings {
    fn default() -> Self {
        Self {
            ambient_brightness: default_ambient_brightness(),
            ssao: false,
            ssgi: false,
            // Twice Bevy's default strength, and on lamp light too, so SSAO is noticeable in
            // scenes lit mostly by lights.
            ssao_settings: ScreenSpaceAmbientOcclusion {
                intensity: 2.0,
                direct_light: true,
                ..default()
            },
            ssgi_settings: default(),
            reflections: false,
            transparent_reflections: true,
            reflections_settings: default(),
        }
    }
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

/// Whether any effect that needs the prepasses (and so MSAA off) is on.
fn needs_prepasses(settings: &WorldSettings) -> bool {
    settings.ssao || settings.ssgi || settings.reflections
}

/// Applies the level's settings: ambient light, and SSAO, SSGI and reflections on the editor
/// camera.
fn apply_world_settings(
    settings: Query<&WorldSettings>,
    cameras: Query<Entity, With<EditorCamera>>,
    new_cameras: Query<(), Added<EditorCamera>>,
    mut applied: Local<Option<WorldSettings>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut commands: Commands,
) {
    // A deleted World Settings entity is disabled, so it no longer matches and effects turn off.
    let current = settings.iter().next().cloned().unwrap_or_default();
    if applied.as_ref() == Some(&current) && new_cameras.is_empty() {
        return;
    }

    if ambient.brightness != current.ambient_brightness {
        ambient.brightness = current.ambient_brightness;
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
        if current.reflections {
            camera.insert(ExtendScreenSpaceReflections {
                glass: current.transparent_reflections,
                ..current.reflections_settings.clone()
            });
        } else {
            camera.remove::<ExtendScreenSpaceReflections>();
        }
        if !needs_prepasses(&current) {
            // Back to the plain forward camera the editor starts with.
            camera.remove::<(
                DepthPrepass,
                NormalPrepass,
                MotionVectorPrepass,
                TonemappingPass,
            )>();
        }
    }
    *applied = Some(current);
}

/// SSAO, SSGI and reflections read the depth and normal prepasses, which don't support MSAA, so
/// MSAA is off while any of them is on. Every camera drawing to the window gets the same setting: Bevy keeps a
/// separate image per MSAA setting, so the gizmo and UI cameras, which draw over the level
/// camera, would otherwise keep showing an old frame of the level.
fn match_msaa(
    settings: Query<&WorldSettings>,
    cameras: Query<(Entity, Option<&Msaa>), With<Camera>>,
    mut commands: Commands,
) {
    let current = settings.iter().next();
    let msaa = if current.is_some_and(needs_prepasses) {
        Msaa::Off
    } else {
        Msaa::default()
    };
    for (camera, current) in &cameras {
        if current != Some(&msaa) {
            commands.entity(camera).insert(msaa);
        }
    }
}

/// Registers World Settings and the effects they control.
pub struct WorldSettingsPlugin;

impl Plugin for WorldSettingsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ScreenSpaceGlobalIlluminationPlugin>() {
            app.add_plugins(ScreenSpaceGlobalIlluminationPlugin);
        }
        if !app.is_plugin_added::<ExtendScreenSpaceReflectionsPlugin>() {
            app.add_plugins(ExtendScreenSpaceReflectionsPlugin);
        }
        app.register_type::<WorldSettings>()
            .add_systems(PostStartup, |world: &mut World| {
                ensure_world_settings(world);
            })
            .add_systems(Update, (apply_world_settings, match_msaa));
    }
}
