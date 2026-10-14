//! Soft shadows test scene: a baseline of what stock Bevy does today.
//!
//! Mirrors the reference image: a box, a cylinder and a sphere on a grey floor, lit by one close
//! point light with a physical size, so shadows are sharp where objects touch the floor and blur
//! further away. It uses Bevy's built-in percentage-closer soft shadows (PCSS), which sit behind
//! the `experimental_pbr_pcss` feature.
//!
//! Run it with `cargo run -p bevy_extend_sandbox --bin soft_shadows --release`.
//!
//! Controls:
//! - Space: toggle soft shadows (PCSS) on and off
//! - T: toggle temporal antialiasing (TAA) with the temporal shadow filter
//! - `[` and `]`: shrink or grow the light, which changes how blurry shadows get
//! - Hold right click (or press M) to look around, WASD to move, Q/E to go down/up

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass},
    light::ShadowFilteringMethod,
    prelude::*,
    render::camera::TemporalJitter,
};
use bevy_extend::ExtendPlugins;

/// Light size range for the `[` and `]` keys, in meters.
const LIGHT_RADIUS_RANGE: (f32, f32) = (0.02, 1.5);

/// Where the light sits: low and close, front right of the objects, like the reference.
const LIGHT_POSITION: Vec3 = Vec3::new(3.0, 2.2, 3.0);

#[derive(Resource)]
struct Settings {
    soft_shadows: bool,
    taa: bool,
    light_radius: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            soft_shadows: true,
            taa: true,
            light_radius: 0.4,
        }
    }
}

#[derive(Component)]
struct HelpText;

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
        .add_systems(Update, (handle_input, apply_settings).chain())
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
        Name::new("Light"),
        PointLight {
            intensity: 2_000_000.0,
            range: 30.0,
            radius: Settings::default().light_radius,
            shadow_maps_enabled: true,
            soft_shadows_enabled: true,
            ..default()
        },
        Transform::from_translation(LIGHT_POSITION),
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
    mut lights: Query<&mut PointLight>,
    mut cameras: Query<(Entity, &mut ShadowFilteringMethod)>,
    mut help: Single<&mut Text, With<HelpText>>,
) {
    if !settings.is_changed() {
        return;
    }

    for mut light in &mut lights {
        light.soft_shadows_enabled = settings.soft_shadows;
        light.radius = settings.light_radius;
    }

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
    help.0 = format!(
        "Stock Bevy PCSS baseline\n\
         [Space] Soft shadows: {}\n\
         [T] TAA + temporal filter: {}\n\
         [ / ] Light radius: {:.2} m",
        on_off(settings.soft_shadows),
        on_off(settings.taa),
        settings.light_radius,
    );
}
