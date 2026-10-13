//! Runs the editor on a small demo level.
//!
//! ```bash
//! cargo run -p bevy_editor
//! ```

use bevy::{
    inspector::InspectorSelection,
    prelude::*,
    render::view::screenshot::{save_to_disk, Screenshot},
};
use bevy_editor::{play::in_play_mode, EditorPlugin, LevelEntity};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy Editor".into(),
                resolution: (1600, 900).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EditorPlugin)
        .register_type::<Spin>()
        .add_systems(Startup, demo_level)
        .add_systems(Update, spin.run_if(in_play_mode))
        .add_systems(Update, screenshot_and_exit)
        .run();
}

/// When `BEVY_EDITOR_SCREENSHOT` is set to a file path, saves a screenshot there after a few
/// seconds and exits. Used to check the editor's layout without a person at the screen.
/// `BEVY_EDITOR_SELECT` can name an entity to select first.
fn screenshot_and_exit(
    time: Res<Time>,
    mut taken: Local<bool>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    names: Query<(Entity, &Name)>,
    mut selection: ResMut<InspectorSelection>,
) {
    let Ok(path) = std::env::var("BEVY_EDITOR_SCREENSHOT") else {
        return;
    };
    let elapsed = time.elapsed_secs();
    if !*taken
        && elapsed > 2.0
        && selection.0.is_none()
        && let Ok(target) = std::env::var("BEVY_EDITOR_SELECT")
        && let Some((entity, _)) = names.iter().find(|(_, name)| name.as_str() == target)
    {
        selection.0 = Some(entity);
    }
    if !*taken && elapsed > 4.0 {
        *taken = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    } else if elapsed > 6.0 {
        exit.write(AppExit::Success);
    }
}

/// Gameplay example: spins the entity while the level is playing.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Spin {
    /// Turns per second.
    speed: f32,
}

fn spin(time: Res<Time>, mut spinners: Query<(&Spin, &mut Transform)>) {
    for (spin, mut transform) in &mut spinners {
        transform.rotate_y(spin.speed * core::f32::consts::TAU * time.delta_secs());
    }
}

fn demo_level(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut material = |color: Color| materials.add(StandardMaterial::from_color(color));

    commands.spawn((
        Name::new("Sun"),
        LevelEntity,
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Name::new("Floor"),
        LevelEntity,
        Mesh3d(meshes.add(Cuboid::new(12.0, 0.2, 12.0))),
        MeshMaterial3d(material(Color::srgb(0.3, 0.32, 0.35))),
        Transform::from_xyz(0.0, -0.1, 0.0),
    ));
    commands
        .spawn((
            Name::new("Props"),
            LevelEntity,
            Transform::default(),
            Visibility::default(),
        ))
        .with_children(|props| {
            props.spawn((
                Name::new("Red Cube"),
                Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
                MeshMaterial3d(material(Color::srgb(0.8, 0.25, 0.2))),
                Transform::from_xyz(-2.0, 0.5, 0.0),
            ));
            props.spawn((
                Name::new("Blue Cube"),
                Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
                MeshMaterial3d(material(Color::srgb(0.2, 0.4, 0.85))),
                Transform::from_xyz(2.0, 0.5, 0.0),
                Spin { speed: 0.25 },
            ));
        });
    commands.spawn((
        Name::new("Gold Sphere"),
        LevelEntity,
        Mesh3d(meshes.add(Sphere::new(0.6))),
        MeshMaterial3d(material(Color::srgb(0.95, 0.75, 0.25))),
        Transform::from_xyz(0.0, 0.6, -2.5),
        Spin { speed: 0.5 },
    ));
    commands.spawn((
        Name::new("Point Light"),
        LevelEntity,
        PointLight {
            intensity: 200_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-3.0, 3.0, 3.0),
    ));
}
