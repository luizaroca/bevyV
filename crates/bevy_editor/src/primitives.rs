//! Basic shapes, like Unreal's "Basic" actors (Cube, Sphere, Cylinder, Plane, Cone, Capsule,
//! Torus).
//!
//! Meshes and materials created in code are assets without a file, so they cannot be saved in a
//! level. A [`Primitive`] and a [`PrimitiveMaterial`] describe the shape and look instead; the
//! mesh and material are generated from them, and regenerated whenever they are edited in the
//! Details panel.

use bevy::{platform::collections::HashMap, prelude::*};

/// A basic shape. The entity's mesh is generated from it.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[require(Transform, Visibility, PrimitiveMaterial)]
pub enum Primitive {
    /// A box.
    Cube {
        /// Size along each axis.
        size: Vec3,
    },
    /// A sphere.
    Sphere {
        /// Radius.
        radius: f32,
    },
    /// An upright cylinder.
    Cylinder {
        /// Radius.
        radius: f32,
        /// Height.
        height: f32,
    },
    /// An upright cone.
    Cone {
        /// Radius of the base.
        radius: f32,
        /// Height.
        height: f32,
    },
    /// A flat, upward-facing plane.
    Plane {
        /// Size along X and Z.
        size: Vec2,
    },
    /// An upright capsule.
    Capsule {
        /// Radius.
        radius: f32,
        /// Length of the middle section.
        length: f32,
    },
    /// A ring lying flat on the ground.
    Torus {
        /// Radius of the tube.
        minor_radius: f32,
        /// Distance from the centre to the middle of the tube.
        major_radius: f32,
    },
}

impl Default for Primitive {
    fn default() -> Self {
        Self::Cube { size: Vec3::ONE }
    }
}

impl Primitive {
    /// Builds the mesh for this shape.
    pub fn mesh(&self) -> Mesh {
        match *self {
            Primitive::Cube { size } => Cuboid::from_size(size.max(Vec3::splat(0.001))).into(),
            Primitive::Sphere { radius } => Sphere::new(radius.max(0.001)).mesh().uv(32, 18),
            Primitive::Cylinder { radius, height } => {
                Cylinder::new(radius.max(0.001), height.max(0.001)).into()
            }
            Primitive::Cone { radius, height } => Cone {
                radius: radius.max(0.001),
                height: height.max(0.001),
            }
            .into(),
            Primitive::Plane { size } => Plane3d::default()
                .mesh()
                .size(size.x.max(0.001), size.y.max(0.001))
                .into(),
            Primitive::Capsule { radius, length } => {
                Capsule3d::new(radius.max(0.001), length.max(0.0)).into()
            }
            Primitive::Torus {
                minor_radius,
                major_radius,
            } => Torus {
                minor_radius: minor_radius.max(0.001),
                major_radius: major_radius.max(0.002),
            }
            .into(),
        }
    }
}

/// The surface of a [`Primitive`]. The entity's material is generated from it.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
pub struct PrimitiveMaterial {
    /// Base colour. An alpha below 1 makes the surface see-through, like glass.
    pub base_color: Color,
    /// 0 is dielectric, 1 is metal.
    pub metallic: f32,
    /// 0 is mirror-smooth, 1 is fully rough.
    pub roughness: f32,
    /// Light emitted by the surface.
    pub emissive: LinearRgba,
}

impl Default for PrimitiveMaterial {
    fn default() -> Self {
        Self {
            base_color: Color::srgb(0.8, 0.8, 0.8),
            metallic: 0.0,
            roughness: 0.5,
            emissive: LinearRgba::BLACK,
        }
    }
}

impl PrimitiveMaterial {
    /// A material with the given colour and default settings.
    pub fn color(base_color: Color) -> Self {
        Self {
            base_color,
            ..default()
        }
    }
}

/// Meshes already built for each shape, so identical shapes share one mesh, like a static mesh
/// asset in Unreal.
#[derive(Resource, Default)]
struct PrimitiveMeshes(HashMap<[u32; 4], Handle<Mesh>>);

impl Primitive {
    /// A key identifying the shape and its dimensions exactly.
    fn key(&self) -> [u32; 4] {
        let bits = f32::to_bits;
        match *self {
            Primitive::Cube { size } => [0, bits(size.x), bits(size.y), bits(size.z)],
            Primitive::Sphere { radius } => [1, bits(radius), 0, 0],
            Primitive::Cylinder { radius, height } => [2, bits(radius), bits(height), 0],
            Primitive::Cone { radius, height } => [3, bits(radius), bits(height), 0],
            Primitive::Plane { size } => [4, bits(size.x), bits(size.y), 0],
            Primitive::Capsule { radius, length } => [5, bits(radius), bits(length), 0],
            Primitive::Torus {
                minor_radius,
                major_radius,
            } => [6, bits(minor_radius), bits(major_radius), 0],
        }
    }
}

fn build_meshes(
    shapes: Query<(Entity, &Primitive), Changed<Primitive>>,
    mut cache: ResMut<PrimitiveMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    for (entity, shape) in &shapes {
        let mesh = cache
            .0
            .entry(shape.key())
            .or_insert_with(|| meshes.add(shape.mesh()))
            .clone();
        commands.entity(entity).insert(Mesh3d(mesh));
    }
}

fn build_materials(
    surfaces: Query<(Entity, &PrimitiveMaterial), Changed<PrimitiveMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (entity, surface) in &surfaces {
        let material = StandardMaterial {
            base_color: surface.base_color,
            metallic: surface.metallic.clamp(0.0, 1.0),
            perceptual_roughness: surface.roughness.clamp(0.089, 1.0),
            emissive: surface.emissive,
            // See-through when the colour's alpha is below 1, like glass.
            alpha_mode: if surface.base_color.alpha() < 1.0 {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            },
            ..default()
        };
        commands
            .entity(entity)
            .insert(MeshMaterial3d(materials.add(material)));
    }
}

/// Registers the basic shapes.
pub struct PrimitivesPlugin;

impl Plugin for PrimitivesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PrimitiveMeshes>()
            .register_type::<Primitive>()
            .register_type::<PrimitiveMaterial>()
            .add_systems(PostUpdate, (build_meshes, build_materials));
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn each_shape_has_its_own_material() {
        let mut world = World::new();
        world.init_resource::<Assets<StandardMaterial>>();
        let a = world.spawn(PrimitiveMaterial::default()).id();
        let b = world.spawn(PrimitiveMaterial::default()).id();
        world.run_system_once(build_materials).unwrap();

        world.get_mut::<PrimitiveMaterial>(a).unwrap().roughness = 0.9;
        world.get_mut::<PrimitiveMaterial>(a).unwrap().base_color = Color::srgb(1.0, 0.0, 0.0);
        world.run_system_once(build_materials).unwrap();

        let material = |entity| {
            let handle = world
                .get::<MeshMaterial3d<StandardMaterial>>(entity)
                .unwrap();
            world
                .resource::<Assets<StandardMaterial>>()
                .get(handle)
                .unwrap()
                .clone()
        };
        assert_eq!(material(a).perceptual_roughness, 0.9);
        assert_eq!(material(a).base_color, Color::srgb(1.0, 0.0, 0.0));
        assert_eq!(material(b).perceptual_roughness, 0.5);
    }
}
