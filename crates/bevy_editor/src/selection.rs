//! Selection: clicking in the viewport, the Outliner and the gizmo all share one selection.
//!
//! The selection lives in the inspector's [`InspectorSelection`] resource, so the Outliner and
//! the Details panel follow it without extra wiring.

use bevy::{
    camera::primitives::Aabb,
    ecs::entity_disabling::Disabled,
    gizmos::transform_gizmo::{TransformGizmoFocus, TransformGizmoState},
    inspector::InspectorSelection,
    picking::{events::PointerClick, pointer::PointerButton},
    prelude::*,
    shape::Aabb3d,
};

use crate::{
    is_in_level,
    viewport::{SuppressClick, ViewportRect},
    EditorEntity, LevelEntity,
};

/// The systems that update the selection.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionSystems;

/// Whether the transform gizmo is shown on the selection. Unreal's "Select" mode (Q) hides it.
#[derive(Resource)]
pub struct GizmoVisible(pub bool);

impl Default for GizmoVisible {
    fn default() -> Self {
        Self(true)
    }
}

/// Unreal's selection highlight colour.
pub const SELECTION_COLOR: Color = Color::srgb(1.0, 0.55, 0.05);

/// Selects what was clicked in the viewport, or clears the selection when empty space was
/// clicked.
fn select_on_click(
    mut clicks: MessageReader<PointerClick>,
    gizmo: Res<TransformGizmoState>,
    viewport: Res<ViewportRect>,
    suppress: Res<SuppressClick>,
    nodes: Query<(), With<Node>>,
    windows: Query<(), With<Window>>,
    editor: Query<(), With<EditorEntity>>,
    levels: Query<(), With<LevelEntity>>,
    parents: Query<&ChildOf>,
    names: Query<(), With<Name>>,
    mut selection: ResMut<InspectorSelection>,
) {
    for click in clicks.read() {
        if click.button != PointerButton::Primary {
            continue;
        }
        // A click that ends a gizmo drag is not a selection.
        if gizmo.active || gizmo.hovered_axis.is_some() || suppress.0 {
            continue;
        }
        let target = click.entity;
        if nodes.contains(target) || editor.contains(target) {
            continue;
        }
        if windows.contains(target) {
            if viewport
                .0
                .is_some_and(|rect| rect.contains(click.pointer.position))
            {
                selection.0 = None;
            }
            continue;
        }
        if !is_in_level(target, &levels, &parents) {
            continue;
        }
        // Select the nearest named entity, so clicking a mesh inside an imported model selects
        // the model's node rather than an anonymous primitive.
        let mut picked = target;
        while !names.contains(picked) {
            match parents.get(picked) {
                Ok(child_of) => picked = child_of.parent(),
                Err(_) => {
                    picked = target;
                    break;
                }
            }
        }
        selection.0 = Some(picked);
    }
}

/// Clears the selection when the selected entity is deleted.
fn clear_deleted_selection(
    mut selection: ResMut<InspectorSelection>,
    alive: Query<(), Without<Disabled>>,
) {
    if let Some(entity) = selection.0
        && !alive.contains(entity)
    {
        selection.0 = None;
    }
}

/// Moves the transform gizmo onto the selected entity.
fn sync_gizmo_focus(
    selection: Res<InspectorSelection>,
    visible: Res<GizmoVisible>,
    focused: Query<Entity, With<TransformGizmoFocus>>,
    transforms: Query<(), (With<Transform>, Without<EditorEntity>)>,
    mut commands: Commands,
) {
    if !selection.is_changed() && !visible.is_changed() {
        return;
    }
    let target = selection
        .0
        .filter(|entity| visible.0 && transforms.contains(*entity));
    for entity in &focused {
        if Some(entity) != target {
            commands.entity(entity).remove::<TransformGizmoFocus>();
        }
    }
    if let Some(entity) = target
        && !focused.contains(entity)
    {
        commands.entity(entity).insert(TransformGizmoFocus);
    }
}

/// Draws bounding boxes around the selected entity and its descendants.
fn draw_selection(
    selection: Res<InspectorSelection>,
    bounds: Query<(&Aabb, &GlobalTransform)>,
    children: Query<&Children>,
    mut gizmos: Gizmos,
) {
    let Some(selected) = selection.0 else {
        return;
    };
    let mut stack = vec![selected];
    while let Some(entity) = stack.pop() {
        if let Ok((aabb, transform)) = bounds.get(entity) {
            let aabb = Aabb3d::new(Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
            gizmos.aabb_3d(aabb, *transform, SELECTION_COLOR);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
}

/// Registers the selection systems.
pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GizmoVisible>()
            .add_systems(
                Update,
                (select_on_click, clear_deleted_selection, sync_gizmo_focus)
                    .chain()
                    .in_set(SelectionSystems),
            )
            .add_systems(PostUpdate, draw_selection);
    }
}
