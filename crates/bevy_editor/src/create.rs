//! Creating lights and basic shapes from the toolbar's Create menu, like Unreal's
//! "Quickly add to the project" menu and Place Actors panel.
//!
//! New actors are placed in front of the viewport camera (on the ground when the camera looks
//! down at it), selected, and recorded as one undo step. Each shape gets its own
//! [`PrimitiveMaterial`], so its colour and roughness can be edited in the Details panel without
//! affecting other shapes.

use bevy::{inspector::InspectorSelection, prelude::*};

use crate::{
    history::{CreateEntities, History},
    play::EditorMode,
    primitives::{Primitive, PrimitiveMaterial},
    viewport::EditorCamera,
    LevelEntity,
};

/// Something the Create menu can add to the level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorKind {
    /// A light shining in all directions from a point.
    PointLight,
    /// A cone of light.
    SpotLight,
    /// A sun: parallel light from far away.
    DirectionalLight,
    /// A 1 m cube.
    Cube,
    /// A sphere 1 m across.
    Sphere,
    /// A cylinder 1 m across and 1 m tall.
    Cylinder,
    /// A cone 1 m across and 1 m tall.
    Cone,
    /// A 2 m square plane.
    Plane,
    /// A capsule 1 m across and 2 m tall.
    Capsule,
    /// A torus (ring) 1 m across.
    Torus,
}

impl ActorKind {
    /// Lights, in menu order.
    pub const LIGHTS: [ActorKind; 3] = [
        ActorKind::PointLight,
        ActorKind::SpotLight,
        ActorKind::DirectionalLight,
    ];

    /// Shapes, in menu order.
    pub const SHAPES: [ActorKind; 7] = [
        ActorKind::Cube,
        ActorKind::Sphere,
        ActorKind::Cylinder,
        ActorKind::Cone,
        ActorKind::Plane,
        ActorKind::Capsule,
        ActorKind::Torus,
    ];

    /// The kind whose [`label`](Self::label) is `label`.
    pub fn from_label(label: &str) -> Option<ActorKind> {
        Self::LIGHTS
            .into_iter()
            .chain(Self::SHAPES)
            .find(|kind| kind.label().eq_ignore_ascii_case(label.trim()))
    }

    /// The name shown in the menu and given to new actors.
    pub fn label(self) -> &'static str {
        match self {
            ActorKind::PointLight => "Point Light",
            ActorKind::SpotLight => "Spot Light",
            ActorKind::DirectionalLight => "Directional Light",
            ActorKind::Cube => "Cube",
            ActorKind::Sphere => "Sphere",
            ActorKind::Cylinder => "Cylinder",
            ActorKind::Cone => "Cone",
            ActorKind::Plane => "Plane",
            ActorKind::Capsule => "Capsule",
            ActorKind::Torus => "Torus",
        }
    }

    /// The shape, for shape kinds.
    pub fn primitive(self) -> Option<Primitive> {
        Some(match self {
            ActorKind::Cube => Primitive::Cube { size: Vec3::ONE },
            ActorKind::Sphere => Primitive::Sphere { radius: 0.5 },
            ActorKind::Cylinder => Primitive::Cylinder {
                radius: 0.5,
                height: 1.0,
            },
            ActorKind::Cone => Primitive::Cone {
                radius: 0.5,
                height: 1.0,
            },
            ActorKind::Plane => Primitive::Plane {
                size: Vec2::splat(2.0),
            },
            ActorKind::Capsule => Primitive::Capsule {
                radius: 0.5,
                length: 1.0,
            },
            ActorKind::Torus => Primitive::Torus {
                minor_radius: 0.15,
                major_radius: 0.35,
            },
            _ => return None,
        })
    }
}

/// How far in front of the camera new actors go when the camera does not look at the ground.
const PLACE_DISTANCE: f32 = 5.0;
/// The farthest along the view that new actors are dropped onto the ground.
const MAX_GROUND_DISTANCE: f32 = 20.0;

/// Where to place a new actor: the point on the ground the camera looks at, if it is near
/// enough, or else a point a few metres in front of the camera.
pub fn placement_point(camera: &Transform) -> Vec3 {
    let origin = camera.translation;
    let forward = camera.forward();
    let mut distance = PLACE_DISTANCE;
    if forward.y < -0.01 && origin.y > 0.0 && origin.y / -forward.y <= MAX_GROUND_DISTANCE {
        distance = origin.y / -forward.y;
    }
    // Round to the centimetre, so the Details panel shows tidy numbers.
    ((origin + forward * distance) * 100.0).round() / 100.0
}

/// The lowest point of a shape below its origin, so it can be set down on the ground.
fn half_height(primitive: &Primitive) -> f32 {
    match *primitive {
        Primitive::Cube { size } => size.y * 0.5,
        Primitive::Sphere { radius } => radius,
        Primitive::Cylinder { height, .. } | Primitive::Cone { height, .. } => height * 0.5,
        Primitive::Plane { .. } => 0.0,
        Primitive::Capsule { radius, length } => radius + length * 0.5,
        Primitive::Torus { minor_radius, .. } => minor_radius,
    }
}

/// A name for a new actor that no other level actor has yet: "Cube", then "Cube 2", and so on.
fn unique_name(world: &mut World, base: &str) -> String {
    let mut names = world.query_filtered::<&Name, With<LevelEntity>>();
    let taken: Vec<String> = names.iter(world).map(ToString::to_string).collect();
    if !taken.iter().any(|name| name == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken.contains(name))
        .unwrap()
}

/// Adds an actor of `kind` to the level in front of the camera, selects it, and records an undo
/// step. Does nothing while playing. Returns the new entity.
pub fn create_actor(world: &mut World, kind: ActorKind) -> Option<Entity> {
    if world
        .get_resource::<State<EditorMode>>()
        .is_some_and(|mode| *mode.get() == EditorMode::Playing)
    {
        return None;
    }
    let camera = {
        let mut cameras = world.query_filtered::<&Transform, With<EditorCamera>>();
        cameras.iter(world).next().copied().unwrap_or_default()
    };
    let point = placement_point(&camera);
    let name = Name::new(unique_name(world, kind.label()));

    let entity = match kind {
        ActorKind::PointLight => world.spawn((
            name,
            LevelEntity,
            PointLight {
                intensity: 200_000.0,
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_translation(point + Vec3::Y * 2.0),
        )),
        ActorKind::SpotLight => world.spawn((
            name,
            LevelEntity,
            SpotLight {
                intensity: 400_000.0,
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_translation(point + Vec3::Y * 3.0).looking_at(point, Vec3::Z),
        )),
        ActorKind::DirectionalLight => world.spawn((
            name,
            LevelEntity,
            DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_translation(point + Vec3::Y * 3.0)
                .looking_to(Vec3::new(-0.5, -1.0, -0.4), Vec3::Y),
        )),
        _ => {
            let primitive = kind.primitive()?;
            world.spawn((
                name,
                LevelEntity,
                primitive,
                PrimitiveMaterial::default(),
                Transform::from_translation(point + Vec3::Y * half_height(&primitive)),
            ))
        }
    }
    .id();

    world.resource_mut::<InspectorSelection>().0 = Some(entity);
    let label = format!("Create {}", kind.label());
    world
        .resource_mut::<History>()
        .push(CreateEntities::new(label, vec![entity]));
    info!("Created {}", kind.label());
    Some(entity)
}

#[cfg(test)]
mod tests {
    use bevy::ecs::entity_disabling::Disabled;

    use super::*;

    #[test]
    fn places_on_the_ground_in_view() {
        let camera = Transform::from_xyz(0.0, 4.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y);
        assert_eq!(placement_point(&camera), Vec3::ZERO);

        // Looking at the horizon: a few metres in front instead.
        let camera = Transform::from_xyz(0.0, 2.0, 0.0).looking_to(Vec3::NEG_Z, Vec3::Y);
        assert_eq!(
            placement_point(&camera),
            Vec3::new(0.0, 2.0, -PLACE_DISTANCE)
        );
    }

    #[test]
    fn every_shape_kind_has_a_primitive() {
        for kind in ActorKind::SHAPES {
            assert!(kind.primitive().is_some(), "{kind:?}");
        }
        for kind in ActorKind::LIGHTS {
            assert!(kind.primitive().is_none(), "{kind:?}");
        }
    }

    #[test]
    fn create_selects_and_undo_removes() {
        let mut world = World::new();
        world.init_resource::<InspectorSelection>();
        world.init_resource::<History>();

        let entity = create_actor(&mut world, ActorKind::Sphere).unwrap();
        assert_eq!(world.resource::<InspectorSelection>().0, Some(entity));
        assert_eq!(
            world.get::<Primitive>(entity),
            Some(&Primitive::Sphere { radius: 0.5 })
        );
        // Resting on the ground in front of the default camera.
        assert_eq!(
            world.get::<Transform>(entity).unwrap().translation,
            Vec3::new(0.0, 0.5, -PLACE_DISTANCE)
        );

        crate::history::undo(&mut world);
        assert!(world.entity(entity).contains::<Disabled>());
        assert_eq!(world.resource::<InspectorSelection>().0, None);
        crate::history::redo(&mut world);
        assert!(!world.entity(entity).contains::<Disabled>());
    }

    #[test]
    fn names_are_unique() {
        let mut world = World::new();
        world.spawn((Name::new("Cube"), LevelEntity));
        world.spawn((Name::new("Cube 2"), LevelEntity));
        assert_eq!(unique_name(&mut world, "Cube"), "Cube 3");
        assert_eq!(unique_name(&mut world, "Sphere"), "Sphere");
    }
}
