//! Sandbox app for trying out Bevy Extend features as they are built.
//!
//! Run it with `cargo run -p bevy_extend_sandbox`.
//!
//! Controls: hold right click (or press M) to look around, WASD to move, Q/E to go down/up,
//! left shift to run and the scroll wheel to change speed.

use bevy::{
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    prelude::*,
};
use bevy_extend::ExtendPlugins;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, FreeCameraPlugin, ExtendPlugins))
        .add_systems(Startup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
    ));
    commands.spawn((
        Name::new("Cube"),
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));
    commands.spawn((
        Name::new("Sphere"),
        Mesh3d(meshes.add(Sphere::new(0.5))),
        MeshMaterial3d(materials.add(Color::srgb(0.6, 0.7, 0.9))),
        Transform::from_xyz(2.0, 0.5, 1.0),
    ));
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Name::new("Camera"),
        Camera3d::default(),
        Transform::from_xyz(-4.0, 3.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
        FreeCamera::default(),
    ));
}
