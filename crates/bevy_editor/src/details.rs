//! Details panel layout in Unreal's order: the name and transform first, then what the actor
//! is (shape, material, light, camera), then gameplay components, with engine-computed data
//! hidden.

use bevy::{inspector::InspectorComponentOrder, prelude::*};

/// Components computed or managed by the engine or the editor. Editing them has no lasting
/// effect, so the Details panel hides them, as Unreal hides its internal state.
const HIDDEN: &[&str] = &[
    // Hierarchy and transform propagation.
    "ChildOf",
    "Children",
    "GlobalTransform",
    "TransformTreeChanged",
    // Visibility and culling.
    "InheritedVisibility",
    "ViewVisibility",
    "VisibilityClass",
    "VisibleEntities",
    "VisibleMeshEntities",
    "CubemapVisibleEntities",
    "CascadesVisibleEntities",
    "Aabb",
    "Frustum",
    "CubemapFrusta",
    "CascadesFrusta",
    "Cascades",
    "NoFrustumCulling",
    // Render world bookkeeping.
    "SyncToRenderWorld",
    "RenderEntity",
    "MeshMaterial3d",
    "Mesh3d",
    // Editor bookkeeping.
    "LevelEntity",
    "EditorEntity",
    "TransformGizmoFocus",
];

/// The rank of each component shown near the top. Anything else ranks [`OTHER`].
const RANKED: &[(&str, i32)] = &[
    ("Name", 0),
    ("Transform", 10),
    ("Primitive", 20),
    ("PrimitiveMaterial", 21),
    ("DirectionalLight", 30),
    ("PointLight", 30),
    ("SpotLight", 30),
    ("Camera", 40),
    ("Camera3d", 41),
    ("Projection", 42),
    ("Visibility", 90),
];

/// Gameplay components, and anything not listed above, come after the actor's core components.
const OTHER: i32 = 50;
/// Remaining engine components are shown last, like Unreal's "Advanced" sections.
const ENGINE: i32 = 100;

/// The type name without its module path or generic parameters.
fn short_name(type_path: &str) -> &str {
    let base = type_path.split('<').next().unwrap_or(type_path);
    base.rsplit("::").next().unwrap_or(base)
}

/// Ranks a component for the Details panel, or hides it with `None`.
pub fn rank_component(type_path: &str) -> Option<i32> {
    let name = short_name(type_path);
    if HIDDEN.contains(&name) {
        return None;
    }
    if let Some((_, rank)) = RANKED.iter().find(|(ranked, _)| *ranked == name) {
        return Some(*rank);
    }
    if type_path.starts_with("bevy_") && !type_path.starts_with("bevy_editor") {
        Some(ENGINE)
    } else {
        Some(OTHER)
    }
}

/// Applies the Details panel ordering.
pub struct DetailsPlugin;

impl Plugin for DetailsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(InspectorComponentOrder(Some(rank_component)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_and_hides() {
        assert_eq!(
            rank_component("bevy_transform::components::transform::Transform"),
            Some(10)
        );
        assert_eq!(
            rank_component("bevy_transform::components::global_transform::GlobalTransform"),
            None
        );
        assert_eq!(
            rank_component(
                "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>"
            ),
            None
        );
        assert_eq!(rank_component("my_game::Health"), Some(OTHER));
        assert_eq!(
            rank_component("bevy_light::CascadeShadowConfig"),
            Some(ENGINE)
        );
    }
}
