//! Screen-space global illumination (SSGI) test scene.
//!
//! A Cornell box: red left wall, green right wall, white floor, ceiling and back wall, a tall and
//! a short block, and a light near the ceiling. With bounce light, the white surfaces pick up red
//! and green near the side walls, which makes SSGI easy to judge by eye.
//!
//! SSGI is being built in phases. It now lights the scene with bounce light traced from the
//! previous frame's lit image. It is not denoised yet, so it relies on TAA to smooth the noise.
//!
//! Run it with `cargo run -p bevy_extend_sandbox --bin ssgi --release`.
//!
//! Controls:
//! - Space: turn SSGI on and off, to compare
//! - G: cycle the SSGI debug view (off, bounce light, ambient occlusion, history, reprojected
//!   history, reprojection error)
//! - `[` and `]`: shrink or grow the SSGI radius
//! - P: pause or resume the spinning block
//! - T: toggle temporal antialiasing (TAA)
//! - F12: save a screenshot next to where the app was started
//! - Hold right click (or press M) to look around, WASD to move, Q/E to go down/up
//!
//! For automated checks, `SSGI_AUTOSHOT=path.png` saves a screenshot after a short warm-up and
//! quits, `SSGI_DEBUG=off|bounce|ao|history|reprojected|error` picks the starting debug view,
//! `SSGI_ORBIT=1` slowly swings the camera so reprojection can be checked while moving, and
//! `SSGI_TAA=0` starts with TAA off, `SSGI=0` starts with SSGI off, and `SSGI_DEFERRED=1`
//! renders with Bevy's deferred renderer instead of forward.

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    core_pipeline::prepass::DeferredPrepass,
    pbr::DefaultOpaqueRendererMethod,
    prelude::*,
    render::view::screenshot::{save_to_disk, Screenshot},
};
use bevy_extend::{
    ssgi::{ScreenSpaceGlobalIllumination, SsgiDebugView},
    ExtendPlugins,
};

/// Inside size of the box, in meters.
const ROOM_SIZE: f32 = 4.0;
const WALL_THICKNESS: f32 = 0.1;

/// Where the camera looks at the box from, and what it looks at.
const CAMERA_POSITION: Vec3 = Vec3::new(0.0, 2.0, 7.5);
const CAMERA_TARGET: Vec3 = Vec3::new(0.0, 1.8, 0.0);

/// Frames to wait before the automatic screenshot, so TAA and the history have settled.
const AUTOSHOT_FRAME: u32 = 120;

#[derive(Resource)]
struct Settings {
    ssgi: bool,
    debug_view: SsgiDebugView,
    radius: f32,
    taa: bool,
    spinning: bool,
    orbit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let debug_view = match std::env::var("SSGI_DEBUG").as_deref() {
            Ok("bounce") => SsgiDebugView::BounceLight,
            Ok("ao") => SsgiDebugView::AmbientOcclusion,
            Ok("history") => SsgiDebugView::History,
            Ok("reprojected") => SsgiDebugView::Reprojected,
            Ok("error") => SsgiDebugView::ReprojectionError,
            _ => SsgiDebugView::Off,
        };
        Self {
            ssgi: std::env::var("SSGI").map_or(true, |value| value != "0"),
            debug_view,
            radius: ScreenSpaceGlobalIllumination::default().radius,
            taa: std::env::var("SSGI_TAA").map_or(true, |value| value != "0"),
            spinning: true,
            orbit: std::env::var("SSGI_ORBIT").is_ok_and(|value| value == "1"),
        }
    }
}

#[derive(Component)]
struct HelpText;

#[derive(Component)]
struct Spinning;

fn main() {
    let deferred = std::env::var("SSGI_DEFERRED").is_ok_and(|value| value == "1");
    let mut app = App::new();
    if deferred {
        app.insert_resource(DefaultOpaqueRendererMethod::deferred());
    }
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy Extend: SSGI test".into(),
                ..default()
            }),
            ..default()
        }),
        FreeCameraPlugin,
        ExtendPlugins,
    ))
    .init_resource::<Settings>()
    // Almost no ambient light, so any bounce light SSGI adds later stands out.
    .insert_resource(GlobalAmbientLight {
        brightness: 20.0,
        ..default()
    })
    .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.02)))
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            handle_input,
            apply_settings,
            spin,
            orbit_camera,
            take_screenshots,
        )
            .chain(),
    )
    .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let matte = |color: Color| StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.9,
        ..default()
    };
    let white = materials.add(matte(Color::srgb(0.75, 0.75, 0.75)));
    let red = materials.add(matte(Color::srgb(0.75, 0.05, 0.05)));
    let green = materials.add(matte(Color::srgb(0.05, 0.6, 0.08)));

    let half = ROOM_SIZE / 2.0;
    let span = ROOM_SIZE + 2.0 * WALL_THICKNESS;
    let flat = meshes.add(Cuboid::new(span, WALL_THICKNESS, span));
    let side = meshes.add(Cuboid::new(WALL_THICKNESS, span, span));
    let back = meshes.add(Cuboid::new(span, span, WALL_THICKNESS));
    let offset = half + WALL_THICKNESS / 2.0;
    let walls = [
        (
            "Floor",
            flat.clone(),
            white.clone(),
            Vec3::new(0.0, -WALL_THICKNESS / 2.0, 0.0),
        ),
        (
            "Ceiling",
            flat,
            white.clone(),
            Vec3::new(0.0, ROOM_SIZE + WALL_THICKNESS / 2.0, 0.0),
        ),
        (
            "Back wall",
            back,
            white.clone(),
            Vec3::new(0.0, half, -offset),
        ),
        (
            "Left wall",
            side.clone(),
            red,
            Vec3::new(-offset, half, 0.0),
        ),
        ("Right wall", side, green, Vec3::new(offset, half, 0.0)),
    ];
    for (name, mesh, material, position) in walls {
        commands.spawn((
            Name::new(name),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(position),
        ));
    }

    commands.spawn((
        Name::new("Tall block"),
        Mesh3d(meshes.add(Cuboid::new(1.2, 2.4, 1.2))),
        MeshMaterial3d(white.clone()),
        Transform::from_xyz(-0.75, 1.2, -0.7).with_rotation(Quat::from_rotation_y(0.3)),
    ));
    commands.spawn((
        Name::new("Short block"),
        Spinning,
        Mesh3d(meshes.add(Cuboid::new(1.2, 1.2, 1.2))),
        MeshMaterial3d(white),
        Transform::from_xyz(0.8, 0.6, 0.6).with_rotation(Quat::from_rotation_y(-0.3)),
    ));

    // A glowing panel in the ceiling marks where the light comes from.
    commands.spawn((
        Name::new("Ceiling lamp"),
        Mesh3d(meshes.add(Cuboid::new(1.0, 0.02, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            emissive: LinearRgba::rgb(20.0, 18.0, 15.0),
            ..default()
        })),
        Transform::from_xyz(0.0, ROOM_SIZE - 0.01, 0.0),
    ));
    commands.spawn((
        Name::new("Ceiling light"),
        PointLight {
            intensity: 400_000.0,
            color: Color::srgb(1.0, 0.92, 0.82),
            range: 20.0,
            radius: 0.3,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(0.0, ROOM_SIZE - 0.3, 0.0),
    ));

    let camera = commands
        .spawn((
            Name::new("Camera"),
            Camera3d::default(),
            Transform::from_translation(CAMERA_POSITION).looking_at(CAMERA_TARGET, Vec3::Y),
            FreeCamera::default(),
            // SSGI adds the prepasses it needs; it does need MSAA off.
            ScreenSpaceGlobalIllumination::default(),
            Msaa::Off,
        ))
        .id();
    if std::env::var("SSGI_DEFERRED").is_ok_and(|value| value == "1") {
        commands.entity(camera).insert(DeferredPrepass);
    }

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
    if keys.just_pressed(KeyCode::KeyG) {
        settings.debug_view = settings.debug_view.next();
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        settings.radius = (settings.radius / 1.25).max(0.25);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        settings.radius = (settings.radius * 1.25).min(8.0);
    }
    if keys.just_pressed(KeyCode::Space) {
        settings.ssgi = !settings.ssgi;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        settings.spinning = !settings.spinning;
    }
    if keys.just_pressed(KeyCode::KeyT) {
        settings.taa = !settings.taa;
    }
}

fn apply_settings(
    mut commands: Commands,
    settings: Res<Settings>,
    cameras: Query<Entity, With<Camera3d>>,
    mut help: Single<&mut Text, With<HelpText>>,
) {
    if !settings.is_changed() {
        return;
    }

    for camera in &cameras {
        if settings.ssgi {
            commands
                .entity(camera)
                .insert(ScreenSpaceGlobalIllumination {
                    radius: settings.radius,
                    debug_view: settings.debug_view,
                    ..default()
                });
        } else {
            commands
                .entity(camera)
                .remove::<ScreenSpaceGlobalIllumination>();
        }
        if settings.taa {
            commands
                .entity(camera)
                .insert(TemporalAntiAliasing::default());
        } else {
            commands.entity(camera).remove::<TemporalAntiAliasing>();
        }
    }

    let on_off = |on: bool| if on { "on" } else { "off" };
    let debug_view = match settings.debug_view {
        SsgiDebugView::Off => "off",
        SsgiDebugView::BounceLight => "bounce light (raw, noisy)",
        SsgiDebugView::AmbientOcclusion => "ambient occlusion (raw, noisy)",
        SsgiDebugView::History => "history (last frame)",
        SsgiDebugView::Reprojected => "reprojected history (magenta: newly visible)",
        SsgiDebugView::ReprojectionError => "reprojection error (black is good)",
    };
    help.0 = format!(
        "SSGI test (Bevy Extend), phase 3: lighting, not denoised yet\n\
         [Space] SSGI: {}\n\
         [G] Debug view: {debug_view}\n\
         [ / ] Radius: {:.2} m\n\
         [P] Spinning block: {}\n\
         [T] TAA: {}",
        on_off(settings.ssgi),
        settings.radius,
        on_off(settings.spinning),
        on_off(settings.taa),
    );
}

fn spin(
    time: Res<Time>,
    settings: Res<Settings>,
    mut blocks: Query<&mut Transform, With<Spinning>>,
) {
    if !settings.spinning {
        return;
    }
    for mut transform in &mut blocks {
        transform.rotate_y(0.6 * time.delta_secs());
    }
}

/// Swings the camera slowly from side to side, for checking reprojection while moving.
fn orbit_camera(
    time: Res<Time>,
    settings: Res<Settings>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    if !settings.orbit {
        return;
    }
    let angle = 0.25 * ops::sin(0.8 * time.elapsed_secs());
    let position = CAMERA_TARGET + Quat::from_rotation_y(angle) * (CAMERA_POSITION - CAMERA_TARGET);
    for mut transform in &mut cameras {
        *transform = Transform::from_translation(position).looking_at(CAMERA_TARGET, Vec3::Y);
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
        let path = format!("ssgi_{}.png", *frame);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if let Ok(path) = std::env::var("SSGI_AUTOSHOT") {
        if *frame == AUTOSHOT_FRAME {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        } else if *frame == AUTOSHOT_FRAME + 30 {
            exit.write(AppExit::Success);
        }
    }
}
