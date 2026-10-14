//! Soft shadows test scene.
//!
//! Mirrors the reference image: a box, a cylinder and a sphere on a grey floor, lit by one light
//! with a physical size, so shadows are sharp where objects touch the floor and blur further away.
//! It uses percentage-closer soft shadows (PCSS), which sit behind the `experimental_pbr_pcss`
//! feature. This fork reworks the PCSS shaders in `bevy_pbr/src/render/shadow_sampling.wesl` and
//! `shadows.wesl` so the light size actually controls the penumbra for every light type.
//!
//! Run it with `cargo run -p bevy_extend_sandbox --bin soft_shadows --release`.
//!
//! Controls:
//! - L: switch between a point light, a spot light and a sun (directional light)
//! - Space: toggle soft shadows (PCSS) on and off
//! - T: toggle temporal antialiasing (TAA) with the temporal shadow filter
//! - `[` and `]`: shrink or grow the light, which changes how blurry shadows get
//! - F12: save a screenshot next to where the app was started
//! - Hold right click (or press M) to look around, WASD to move, Q/E to go down/up
//!
//! For automated comparisons, `SOFT_SHADOWS_AUTOSHOT=path.png` saves a screenshot after a short
//! warm-up and quits, and `SOFT_SHADOWS_LIGHT=point|spot|sun`, `SOFT_SHADOWS_PCSS=0`,
//! `SOFT_SHADOWS_TAA=0` and `SOFT_SHADOWS_RADIUS=<meters>` override the starting settings.

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass},
    light::{CascadeShadowConfigBuilder, ShadowFilteringMethod},
    prelude::*,
    render::{
        camera::TemporalJitter,
        view::screenshot::{save_to_disk, Screenshot},
    },
};
use bevy_extend::ExtendPlugins;

/// Light size range for the `[` and `]` keys, in meters.
const LIGHT_RADIUS_RANGE: (f32, f32) = (0.02, 1.0);

/// Where the light sits: low and close, front right of the objects, like the reference.
const LIGHT_POSITION: Vec3 = Vec3::new(3.0, 2.2, 3.0);

/// What the point and spot lights aim at, and where the sun shines from.
const LIGHT_TARGET: Vec3 = Vec3::new(0.0, 0.5, -0.5);

/// Where the sun shines from (only its direction matters).
const SUN_POSITION: Vec3 = Vec3::new(4.0, 3.0, -2.5);

/// The sun's softness is the tangent of its angular radius, so the light size slider is scaled
/// down for it: 0.4 on the slider gives the sun a 0.04 penumbra spread per meter.
const SUN_SIZE_SCALE: f32 = 0.1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LightKind {
    Point,
    Spot,
    Sun,
}

impl LightKind {
    fn next(self) -> Self {
        match self {
            Self::Point => Self::Spot,
            Self::Spot => Self::Sun,
            Self::Sun => Self::Point,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Spot => "spot",
            Self::Sun => "sun",
        }
    }
}

#[derive(Resource)]
struct Settings {
    light: LightKind,
    soft_shadows: bool,
    taa: bool,
    light_radius: f32,
}

impl Default for Settings {
    fn default() -> Self {
        let env_flag = |name: &str| std::env::var(name).map_or(true, |value| value != "0");
        let light = match std::env::var("SOFT_SHADOWS_LIGHT").as_deref() {
            Ok("spot") => LightKind::Spot,
            Ok("sun") => LightKind::Sun,
            _ => LightKind::Point,
        };
        Self {
            light,
            soft_shadows: env_flag("SOFT_SHADOWS_PCSS"),
            taa: env_flag("SOFT_SHADOWS_TAA"),
            light_radius: std::env::var("SOFT_SHADOWS_RADIUS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.4),
        }
    }
}

/// Frames to wait before the automatic screenshot, so TAA has settled.
const AUTOSHOT_FRAME: u32 = 120;

#[derive(Component)]
struct HelpText;

/// Marks the scene's one light, which is respawned when the light type changes.
#[derive(Component)]
struct SceneLight;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bevy Extend: soft shadows test".into(),
                    ..default()
                }),
                ..default()
            }),
            FreeCameraPlugin,
            ExtendPlugins,
        ))
        .init_resource::<Settings>()
        // Almost no ambient light, so the shadows read as dark as in the reference.
        .insert_resource(GlobalAmbientLight {
            brightness: 20.0,
            ..default()
        })
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.02)))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (handle_input, apply_settings, take_screenshots).chain(),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let grey = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.6),
        perceptual_roughness: 0.8,
        ..default()
    });
    let glossy = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.6),
        perceptual_roughness: 0.25,
        ..default()
    });

    commands.spawn((
        Name::new("Floor"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(40.0, 40.0))),
        MeshMaterial3d(grey.clone()),
    ));
    commands.spawn((
        Name::new("Box"),
        Mesh3d(meshes.add(Cuboid::new(1.6, 1.6, 1.6))),
        MeshMaterial3d(grey.clone()),
        Transform::from_xyz(-0.6, 0.8, -0.8).with_rotation(Quat::from_rotation_y(0.5)),
    ));
    commands.spawn((
        Name::new("Cylinder"),
        Mesh3d(meshes.add(Cylinder::new(0.35, 2.0))),
        MeshMaterial3d(grey),
        Transform::from_xyz(1.6, 1.0, -0.6),
    ));
    commands.spawn((
        Name::new("Sphere"),
        Mesh3d(meshes.add(Sphere::new(0.55).mesh().uv(64, 32))),
        MeshMaterial3d(glossy),
        Transform::from_xyz(0.2, 0.55, 1.3),
    ));

    commands.spawn((
        Name::new("Camera"),
        Camera3d::default(),
        Transform::from_xyz(3.5, 5.0, 6.5).looking_at(Vec3::new(0.3, 0.4, 0.0), Vec3::Y),
        FreeCamera::default(),
        ShadowFilteringMethod::Temporal,
        TemporalAntiAliasing::default(),
        // TAA needs jitter, depth and motion vector prepasses, and MSAA off.
        TemporalJitter::default(),
        DepthPrepass,
        MotionVectorPrepass,
        Msaa::Off,
    ));

    commands.spawn((
        HelpText,
        Text::default(),
        Node {
            position_type: PositionType::Absolute,
            top: px(12),
            left: px(12),
            ..default()
        },
    ));
}

fn handle_input(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<Settings>) {
    if keys.just_pressed(KeyCode::KeyL) {
        settings.light = settings.light.next();
    }
    if keys.just_pressed(KeyCode::Space) {
        settings.soft_shadows = !settings.soft_shadows;
    }
    if keys.just_pressed(KeyCode::KeyT) {
        settings.taa = !settings.taa;
    }
    let (min, max) = LIGHT_RADIUS_RANGE;
    if keys.just_pressed(KeyCode::BracketLeft) {
        settings.light_radius = (settings.light_radius / 1.5).max(min);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        settings.light_radius = (settings.light_radius * 1.5).min(max);
    }
}

fn apply_settings(
    mut commands: Commands,
    settings: Res<Settings>,
    lights: Query<Entity, With<SceneLight>>,
    mut cameras: Query<(Entity, &mut ShadowFilteringMethod)>,
    mut help: Single<&mut Text, With<HelpText>>,
) {
    if !settings.is_changed() {
        return;
    }

    // Respawning the light is the simplest way to switch types and settings.
    for light in &lights {
        commands.entity(light).despawn();
    }
    spawn_light(&mut commands, &settings);

    for (camera, mut filtering) in &mut cameras {
        if settings.taa {
            *filtering = ShadowFilteringMethod::Temporal;
            commands
                .entity(camera)
                .insert(TemporalAntiAliasing::default());
        } else {
            *filtering = ShadowFilteringMethod::Gaussian;
            commands.entity(camera).remove::<TemporalAntiAliasing>();
        }
    }

    let on_off = |on: bool| if on { "on" } else { "off" };
    let size = match settings.light {
        LightKind::Sun => format!(
            "{:.3} spread per meter",
            settings.light_radius * SUN_SIZE_SCALE
        ),
        _ => format!("{:.2} m radius", settings.light_radius),
    };
    help.0 = format!(
        "Soft shadows test (Bevy Extend PCSS)\n\
         [L] Light: {}\n\
         [Space] Soft shadows: {}\n\
         [T] TAA + temporal filter: {}\n\
         [ / ] Light size: {}",
        settings.light.label(),
        on_off(settings.soft_shadows),
        on_off(settings.taa),
        size,
    );
}

fn spawn_light(commands: &mut Commands, settings: &Settings) {
    let transform = Transform::from_translation(LIGHT_POSITION).looking_at(LIGHT_TARGET, Vec3::Y);
    match settings.light {
        LightKind::Point => {
            commands.spawn((
                Name::new("Point light"),
                SceneLight,
                PointLight {
                    intensity: 2_000_000.0,
                    range: 30.0,
                    radius: settings.light_radius,
                    shadow_maps_enabled: true,
                    soft_shadows_enabled: settings.soft_shadows,
                    ..default()
                },
                transform,
            ));
        }
        LightKind::Spot => {
            commands.spawn((
                Name::new("Spot light"),
                SceneLight,
                SpotLight {
                    intensity: 2_000_000.0,
                    range: 30.0,
                    radius: settings.light_radius,
                    outer_angle: 1.0,
                    inner_angle: 0.6,
                    shadow_maps_enabled: true,
                    soft_shadows_enabled: settings.soft_shadows,
                    ..default()
                },
                transform,
            ));
        }
        LightKind::Sun => {
            // The sun comes from the side so its long shadows aren't hidden behind the objects
            // from the starting camera angle.
            let transform =
                Transform::from_translation(SUN_POSITION).looking_at(LIGHT_TARGET, Vec3::Y);
            commands.spawn((
                Name::new("Sun"),
                SceneLight,
                DirectionalLight {
                    illuminance: 10_000.0,
                    shadow_maps_enabled: true,
                    soft_shadow_size: settings
                        .soft_shadows
                        .then_some(settings.light_radius * SUN_SIZE_SCALE),
                    ..default()
                },
                transform,
                // The scene is small, so tight cascades give sharper contact shadows.
                CascadeShadowConfigBuilder {
                    first_cascade_far_bound: 6.0,
                    maximum_distance: 25.0,
                    ..default()
                }
                .build(),
            ));
        }
    }
}

fn take_screenshots(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut frame: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    *frame += 1;
    if keys.just_pressed(KeyCode::F12) {
        let path = format!("soft_shadows_{}.png", *frame);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if let Ok(path) = std::env::var("SOFT_SHADOWS_AUTOSHOT") {
        if *frame == AUTOSHOT_FRAME {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        } else if *frame == AUTOSHOT_FRAME + 30 {
            exit.write(AppExit::Success);
        }
    }
}
