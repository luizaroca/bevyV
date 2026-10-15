//! Runs the editor on a small demo level, or on a level file given on the command line.
//!
//! ```bash
//! cargo run -p bevy_editor
//! cargo run -p bevy_editor -- assets/levels/soft_shadows_point.level.ron
//! ```

use bevy::{
    inspector::InspectorSelection,
    prelude::*,
    render::view::screenshot::{save_to_disk, Screenshot},
};
use bevy_editor::{
    create::ActorKind,
    play::in_play_mode,
    primitives::{Primitive, PrimitiveMaterial},
    EditorPlugin, LevelEntity,
};

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
        .add_systems(
            Startup,
            (
                demo_level.run_if(|| level_from_args().is_none()),
                open_level_from_args.run_if(|| level_from_args().is_some()),
            ),
        )
        .add_systems(Update, spin.run_if(in_play_mode))
        .add_systems(Update, screenshot_and_exit)
        .run();
}

/// The level file passed on the command line, if any.
fn level_from_args() -> Option<std::path::PathBuf> {
    std::env::args_os().nth(1).map(std::path::PathBuf::from)
}

/// Opens the level file passed on the command line.
fn open_level_from_args(world: &mut World) {
    let Some(path) = level_from_args() else {
        return;
    };
    if let Err(error) = bevy_editor::level::load_level_from(world, &path) {
        error!("Could not open {}: {error}", path.display());
    }
}

/// When `BEVY_EDITOR_SCREENSHOT` is set to a file path, saves a screenshot there after a few
/// seconds and exits. Used to check the editor's layout without a person at the screen.
/// `BEVY_EDITOR_SELECT` can name an entity to select first, and `BEVY_EDITOR_ROUNDTRIP` can name
/// a level file to save to and reopen first. `BEVY_EDITOR_CREATE` can list Create menu entries to
/// add first, separated by commas, such as `Cube,Point Light`.
fn screenshot_and_exit(
    time: Res<Time>,
    mut taken: Local<bool>,
    mut round_tripped: Local<bool>,
    mut created: Local<bool>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    names: Query<(Entity, &Name)>,
    mut selection: ResMut<InspectorSelection>,
) {
    let Ok(path) = std::env::var("BEVY_EDITOR_SCREENSHOT") else {
        return;
    };
    let elapsed = time.elapsed_secs();
    if let Ok(list) = std::env::var("BEVY_EDITOR_CREATE")
        && elapsed > 0.5
        && !*created
    {
        *created = true;
        commands.queue(move |world: &mut World| {
            for kind in list.split(',').filter_map(ActorKind::from_label) {
                bevy_editor::create::create_actor(world, kind);
            }
        });
    }
    if let Ok(level) = std::env::var("BEVY_EDITOR_ROUNDTRIP")
        && elapsed > 1.0
        && !*round_tripped
    {
        // Save the level, then open it again, to check that saving loses nothing.
        *round_tripped = true;
        commands.queue(move |world: &mut World| {
            let path = std::path::PathBuf::from(level);
            let result = bevy_editor::level::save_level_to(world, &path)
                .and_then(|()| bevy_editor::level::load_level_from(world, &path));
            if let Err(error) = result {
                error!("Round trip failed: {error}");
            }
        });
    }
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

fn demo_level(mut commands: Commands) {
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
        Primitive::Cube {
            size: Vec3::new(12.0, 0.2, 12.0),
        },
        PrimitiveMaterial::color(Color::srgb(0.3, 0.32, 0.35)),
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
                Primitive::default(),
                PrimitiveMaterial::color(Color::srgb(0.8, 0.25, 0.2)),
                Transform::from_xyz(-2.0, 0.5, 0.0),
            ));
            props.spawn((
                Name::new("Blue Cube"),
                Primitive::default(),
                PrimitiveMaterial::color(Color::srgb(0.2, 0.4, 0.85)),
                Transform::from_xyz(2.0, 0.5, 0.0),
                Spin { speed: 0.25 },
            ));
        });
    commands.spawn((
        Name::new("Gold Sphere"),
        LevelEntity,
        Primitive::Sphere { radius: 0.6 },
        PrimitiveMaterial {
            base_color: Color::srgb(0.95, 0.75, 0.25),
            metallic: 1.0,
            roughness: 0.3,
            ..default()
        },
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
