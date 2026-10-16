//! Screen-space reflections (SSR) test scene.
//!
//! A polished floor made of six strips, each rougher than the last (0.0, 0.1, 0.2, 0.35, 0.5,
//! 0.7), in front of a wall of colored panels and a row of objects: a glass cube, a chrome
//! sphere, a rough gold sphere, a spinning green cube, a blue cylinder, a white sphere and a
//! glowing bar. Each strip shows how reflections hold up at its roughness.
//!
//! SSR is being built in phases (see `/notes/ssr-plan.md` in the project files). Two versions
//! can be compared:
//! - Bevy Extend's own SSR (`ExtendScreenSpaceReflections`), which works with forward rendering.
//!   Its reflections light the scene, include the sky and glass, and are denoised, so rough
//!   strips are smooth without TAA.
//! - Bevy's built-in `ScreenSpaceReflections`, the baseline. It only works with the deferred
//!   renderer, so it needs `SSR_DEFERRED=1`. Things to look for: the 0.7 strip is past its
//!   default roughness range (up to 0.6), the sky never appears in reflections, and rough strips
//!   are very noisy without TAA.
//!
//! Run it with `cargo run -p bevy_extend_sandbox --bin ssr --release`.
//!
//! Controls:
//! - Space: turn SSR on and off, to compare
//! - B: switch between Bevy Extend's SSR and Bevy's built-in one
//! - G: cycle Bevy Extend's SSR debug view (off, reflections, confidence, roughness)
//! - N: toggle Bevy Extend's denoising, to see the raw trace
//! - K: toggle glass (transparent objects) in Bevy Extend's reflections
//! - T: toggle temporal antialiasing (TAA)
//! - P: pause or resume the spinning cube
//! - F12: save a screenshot next to where the app was started
//! - Hold right click (or press M) to look around, WASD to move, Q/E to go down/up
//!
//! For automated checks, `SSR_AUTOSHOT=path.png` saves a screenshot after a short warm-up and
//! quits, `SSR=0` starts with SSR off, `SSR_BUILTIN=1` starts with Bevy's built-in SSR,
//! `SSR_DEBUG=off|reflections|confidence|roughness` picks the starting debug view, `SSR_TAA=0`
//! starts with TAA off, `SSR_DENOISE=0` starts with denoising off, `SSR_GLASS=0` starts with
//! glass out of reflections, `SSR_SPIN=0` starts with the cube still and `SSR_DEFERRED=1` renders
//! with Bevy's deferred renderer instead of forward. `SSR_CAMERA=x,y,z,tx,ty,tz` places the camera
//! at x,y,z looking at tx,ty,tz.

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    camera::Hdr,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    core_pipeline::prepass::DeferredPrepass,
    light::Skybox,
    pbr::{DefaultOpaqueRendererMethod, ScreenSpaceReflections},
    prelude::*,
    render::view::screenshot::{save_to_disk, Screenshot},
};
use bevy_extend::{
    ssr::{ExtendScreenSpaceReflections, SsrDebugView},
    ExtendPlugins,
};

/// Where the camera looks at the scene from, and what it looks at.
const CAMERA_POSITION: Vec3 = Vec3::new(0.0, 1.4, 6.0);
const CAMERA_TARGET: Vec3 = Vec3::new(0.0, 0.7, -2.0);

/// Perceptual roughness of each floor strip, left to right.
const STRIP_ROUGHNESS: [f32; 6] = [0.0, 0.1, 0.2, 0.35, 0.5, 0.7];
const STRIP_WIDTH: f32 = 1.5;
const FLOOR_DEPTH: f32 = 12.0;

/// Where the back wall stands, and where the row of objects stands in front of it.
const WALL_Z: f32 = -4.0;
const OBJECTS_Z: f32 = -2.5;

/// Brightness of the sky and of the light it casts.
const SKY_BRIGHTNESS: f32 = 2000.0;

/// Frames to wait before the automatic screenshot, so TAA has settled.
const AUTOSHOT_FRAME: u32 = 120;

#[derive(Resource)]
struct Settings {
    ssr: bool,
    /// Bevy's built-in SSR instead of Bevy Extend's.
    builtin: bool,
    debug_view: SsrDebugView,
    denoise: bool,
    glass: bool,
    taa: bool,
    spinning: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let debug_view = match std::env::var("SSR_DEBUG").as_deref() {
            Ok("reflections") => SsrDebugView::Reflections,
            Ok("confidence") => SsrDebugView::Confidence,
            Ok("roughness") => SsrDebugView::Roughness,
            _ => SsrDebugView::Off,
        };
        Self {
            ssr: std::env::var("SSR").map_or(true, |value| value != "0"),
            builtin: std::env::var("SSR_BUILTIN").is_ok_and(|value| value == "1"),
            debug_view,
            denoise: std::env::var("SSR_DENOISE").map_or(true, |value| value != "0"),
            glass: std::env::var("SSR_GLASS").map_or(true, |value| value != "0"),
            taa: std::env::var("SSR_TAA").map_or(true, |value| value != "0"),
            spinning: std::env::var("SSR_SPIN").map_or(true, |value| value != "0"),
        }
    }
}

#[derive(Component)]
struct HelpText;

#[derive(Component)]
struct Spinning;

/// The camera's starting place: `SSR_CAMERA` if set, otherwise the default view.
fn camera_transform() -> Transform {
    let numbers: Vec<f32> = std::env::var("SSR_CAMERA")
        .unwrap_or_default()
        .split(',')
        .filter_map(|number| number.trim().parse().ok())
        .collect();
    let (position, target) = match numbers[..] {
        [x, y, z, tx, ty, tz] => (Vec3::new(x, y, z), Vec3::new(tx, ty, tz)),
        _ => (CAMERA_POSITION, CAMERA_TARGET),
    };
    Transform::from_translation(position).looking_at(target, Vec3::Y)
}

fn deferred() -> bool {
    std::env::var("SSR_DEFERRED").is_ok_and(|value| value == "1")
}

fn main() {
    let mut app = App::new();
    if deferred() {
        // Bevy's built-in SSR reads the deferred G-buffer, so opaque materials render deferred.
        app.insert_resource(DefaultOpaqueRendererMethod::deferred());
    }
    app.add_plugins((
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bevy Extend: SSR test".into(),
                    ..default()
                }),
                ..default()
            })
            // The sky comes from the repository's shared `assets` folder. Bevy looks for
            // assets next to this crate when started with `cargo run`.
            .set(AssetPlugin {
                file_path: "../../assets".into(),
                ..default()
            }),
        FreeCameraPlugin,
        ExtendPlugins,
    ))
    .init_resource::<Settings>()
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (handle_input, apply_settings, spin, take_screenshots).chain(),
    )
    .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    // Floor strips: dark and polished, so reflections are easy to see.
    let strip_mesh = meshes.add(Cuboid::new(STRIP_WIDTH, 0.1, FLOOR_DEPTH));
    let floor_width = STRIP_WIDTH * STRIP_ROUGHNESS.len() as f32;
    for (index, roughness) in STRIP_ROUGHNESS.into_iter().enumerate() {
        let x = -floor_width / 2.0 + STRIP_WIDTH * (index as f32 + 0.5);
        commands.spawn((
            Name::new(format!("Floor strip (roughness {roughness})")),
            Mesh3d(strip_mesh.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.03, 0.03, 0.035),
                perceptual_roughness: roughness,
                reflectance: 1.0,
                ..default()
            })),
            Transform::from_xyz(x, -0.05, WALL_Z + FLOOR_DEPTH / 2.0),
        ));
    }

    // Back wall: matte colored panels, one per strip.
    let panel_colors = [
        Color::srgb(0.8, 0.1, 0.1),
        Color::srgb(0.9, 0.45, 0.05),
        Color::srgb(0.85, 0.8, 0.1),
        Color::srgb(0.1, 0.65, 0.15),
        Color::srgb(0.1, 0.3, 0.85),
        Color::srgb(0.5, 0.15, 0.7),
    ];
    let panel_mesh = meshes.add(Cuboid::new(STRIP_WIDTH, 2.5, 0.1));
    for (index, color) in panel_colors.into_iter().enumerate() {
        let x = -floor_width / 2.0 + STRIP_WIDTH * (index as f32 + 0.5);
        commands.spawn((
            Name::new(format!("Wall panel {}", index + 1)),
            Mesh3d(panel_mesh.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                perceptual_roughness: 0.9,
                ..default()
            })),
            Transform::from_xyz(x, 1.25, WALL_Z),
        ));
    }

    // A row of objects in front of the wall.
    let sphere = meshes.add(Sphere::new(0.45).mesh().uv(48, 24));
    let row_x = |index: usize| -floor_width / 2.0 + STRIP_WIDTH * (index as f32 + 0.5);
    commands.spawn((
        Name::new("White sphere"),
        Mesh3d(sphere.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.85, 0.85),
            perceptual_roughness: 0.6,
            ..default()
        })),
        Transform::from_xyz(row_x(5), 0.45, OBJECTS_Z),
    ));
    commands.spawn((
        Name::new("Chrome sphere"),
        Mesh3d(sphere.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.95, 0.95),
            metallic: 1.0,
            perceptual_roughness: 0.1,
            ..default()
        })),
        Transform::from_xyz(row_x(1), 0.45, OBJECTS_Z),
    ));
    commands.spawn((
        Name::new("Gold sphere"),
        Mesh3d(sphere),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.77, 0.34),
            metallic: 1.0,
            perceptual_roughness: 0.35,
            ..default()
        })),
        Transform::from_xyz(row_x(2), 0.45, OBJECTS_Z),
    ));
    commands.spawn((
        Name::new("Green cube"),
        Spinning,
        Mesh3d(meshes.add(Cuboid::new(0.8, 0.8, 0.8))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.7, 0.2),
            perceptual_roughness: 0.5,
            ..default()
        })),
        Transform::from_xyz(row_x(3), 0.4, OBJECTS_Z).with_rotation(Quat::from_rotation_y(0.4)),
    ));
    commands.spawn((
        Name::new("Blue cylinder"),
        Mesh3d(meshes.add(Cylinder::new(0.35, 1.6))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.15, 0.3, 0.9),
            perceptual_roughness: 0.4,
            ..default()
        })),
        Transform::from_xyz(row_x(4), 0.8, OBJECTS_Z),
    ));
    // Transparent, so it is drawn after the deferred lighting and SSR never sees it.
    commands.spawn((
        Name::new("Glass cube"),
        Mesh3d(meshes.add(Cuboid::new(0.8, 0.8, 0.8))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.9, 0.95, 1.0, 0.35),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.05,
            ..default()
        })),
        Transform::from_xyz(row_x(0), 0.4, OBJECTS_Z),
    ));
    // A glowing bar standing on the floor, near the camera: a bright, easy-to-spot reflection.
    commands.spawn((
        Name::new("Glowing bar"),
        Mesh3d(meshes.add(Cuboid::new(0.08, 1.2, 0.08))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            emissive: LinearRgba::rgb(8.0, 7.0, 5.0),
            ..default()
        })),
        Transform::from_xyz(0.0, 0.6, OBJECTS_Z + 1.5),
    ));

    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 3000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let camera = commands
        .spawn((
            Name::new("Camera"),
            Camera3d::default(),
            Hdr,
            camera_transform(),
            FreeCamera::default(),
            // Both SSRs read prepasses, which (like deferred rendering) don't support MSAA.
            Msaa::Off,
            // The sky, and the light it casts. SSR falls back to this where its rays miss.
            EnvironmentMapLight {
                diffuse_map: asset_server.load("environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2"),
                specular_map: asset_server.load("environment_maps/pisa_specular_rgb9e5_zstd.ktx2"),
                intensity: SKY_BRIGHTNESS,
                ..default()
            },
            Skybox {
                image: Some(asset_server.load("environment_maps/pisa_specular_rgb9e5_zstd.ktx2")),
                brightness: SKY_BRIGHTNESS,
                ..default()
            },
        ))
        .id();
    if deferred() {
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
    if keys.just_pressed(KeyCode::Space) {
        settings.ssr = !settings.ssr;
    }
    if keys.just_pressed(KeyCode::KeyB) {
        settings.builtin = !settings.builtin;
    }
    if keys.just_pressed(KeyCode::KeyG) {
        settings.debug_view = settings.debug_view.next();
    }
    if keys.just_pressed(KeyCode::KeyN) {
        settings.denoise = !settings.denoise;
    }
    if keys.just_pressed(KeyCode::KeyK) {
        settings.glass = !settings.glass;
    }
    if keys.just_pressed(KeyCode::KeyT) {
        settings.taa = !settings.taa;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        settings.spinning = !settings.spinning;
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

    // Bevy's built-in SSR would turn the camera's deferred prepass on, but without deferred
    // materials it would have nothing to read.
    let builtin = settings.ssr && settings.builtin && deferred();
    let extend = settings.ssr && !settings.builtin;
    for camera in &cameras {
        if builtin {
            commands
                .entity(camera)
                .insert(ScreenSpaceReflections::default());
        } else {
            commands.entity(camera).remove::<ScreenSpaceReflections>();
        }
        if extend {
            commands
                .entity(camera)
                .insert(ExtendScreenSpaceReflections {
                    debug_view: settings.debug_view,
                    denoise: settings.denoise,
                    glass: settings.glass,
                    max_history_frames: std::env::var("SSR_HISTORY")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(16),
                    ..default()
                });
        } else {
            commands
                .entity(camera)
                .remove::<ExtendScreenSpaceReflections>();
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
    let version = match (settings.builtin, deferred()) {
        (false, _) => "Bevy Extend's",
        (true, true) => "Bevy's built-in",
        (true, false) => "Bevy's built-in (needs SSR_DEFERRED=1, so off)",
    };
    let debug_view = match settings.debug_view {
        SsrDebugView::Off => "off",
        SsrDebugView::Reflections => "reflections",
        SsrDebugView::Confidence => "confidence (white: hit)",
        SsrDebugView::Roughness => "roughness from the prepass",
    };
    help.0 = format!(
        "SSR test (Bevy Extend), phase 4: denoised\n\
         Renderer: {}\n\
         Floor roughness, left to right: 0.0, 0.1, 0.2, 0.35, 0.5, 0.7\n\
         [Space] SSR: {}\n\
         [B] Version: {version}\n\
         [G] Debug view (Bevy Extend's): {debug_view}\n\
         [N] Denoising (Bevy Extend's): {}\n\
         [K] Glass in reflections (Bevy Extend's): {}\n\
         [T] TAA: {}\n\
         [P] Spinning cube: {}",
        if deferred() { "deferred" } else { "forward" },
        on_off(settings.ssr),
        on_off(settings.denoise),
        on_off(settings.glass),
        on_off(settings.taa),
        on_off(settings.spinning),
    );
}

fn spin(
    time: Res<Time>,
    settings: Res<Settings>,
    mut cubes: Query<&mut Transform, With<Spinning>>,
) {
    if !settings.spinning {
        return;
    }
    for mut transform in &mut cubes {
        transform.rotate_y(0.6 * time.delta_secs());
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
        let path = format!("ssr_{}.png", *frame);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if let Ok(path) = std::env::var("SSR_AUTOSHOT") {
        if *frame == AUTOSHOT_FRAME {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        } else if *frame == AUTOSHOT_FRAME + 30 {
            exit.write(AppExit::Success);
        }
    }
}
