//! Undo and redo, modelled on Unreal's transaction system.
//!
//! Every edit made through the editor is recorded as an [`EditorCommand`] on the [`History`]
//! stack after it has been applied. [`undo`] and [`redo`] walk the stack.
//!
//! Recorded today:
//! - Transform gizmo drags (one entry per drag).
//! - Field edits made in the Details panel (one entry per drag or committed value).
//! - Deleting entities.

use std::any::TypeId;

use bevy::{
    ecs::{entity_disabling::Disabled, world::DeferredWorld},
    gizmos::transform_gizmo::{TransformGizmoState, TransformGizmoSystems},
    inspector::details_panel::FieldEdit,
    platform::collections::HashMap,
    prelude::*,
    reflect::{PartialReflect, TypeRegistration},
};

/// An edit that can be undone and redone.
pub trait EditorCommand: Send + Sync + 'static {
    /// A short description shown in the Edit menu and the status bar, like "Move Cube".
    fn label(&self) -> String;
    /// Reverts the edit.
    fn undo(&mut self, world: &mut World);
    /// Re-applies the edit after it was undone.
    fn redo(&mut self, world: &mut World);
}

/// The undo and redo stacks.
#[derive(Resource, Default)]
pub struct History {
    undo: Vec<Box<dyn EditorCommand>>,
    redo: Vec<Box<dyn EditorCommand>>,
    version: u64,
}

impl History {
    /// The most commands kept on the undo stack.
    pub const LIMIT: usize = 256;

    /// Records an edit that has already been applied. Clears the redo stack.
    pub fn push(&mut self, command: impl EditorCommand) {
        self.undo.push(Box::new(command));
        if self.undo.len() > Self::LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.version += 1;
    }

    /// A counter that changes whenever an edit is recorded, undone or redone. Used to tell
    /// whether the level has unsaved changes.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The label of the command [`undo`] would revert.
    pub fn undo_label(&self) -> Option<String> {
        self.undo.last().map(|command| command.label())
    }

    /// The label of the command [`redo`] would re-apply.
    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(|command| command.label())
    }

    /// Forgets all recorded edits.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.version += 1;
    }
}

/// Reverts the most recent edit.
pub fn undo(world: &mut World) {
    let Some(mut command) = world.resource_mut::<History>().undo.pop() else {
        return;
    };
    command.undo(world);
    info!("Undo: {}", command.label());
    let mut history = world.resource_mut::<History>();
    history.redo.push(command);
    history.version += 1;
}

/// Re-applies the most recently undone edit.
pub fn redo(world: &mut World) {
    let Some(mut command) = world.resource_mut::<History>().redo.pop() else {
        return;
    };
    command.redo(world);
    info!("Redo: {}", command.label());
    let mut history = world.resource_mut::<History>();
    history.undo.push(command);
    history.version += 1;
}

/// Replaces a whole component value. Used for gizmo drags and Details panel edits.
pub struct SetComponent {
    /// A description of the edit.
    pub label: String,
    /// The edited entity.
    pub entity: Entity,
    /// The component's type.
    pub type_id: TypeId,
    /// The value before the edit.
    pub before: Box<dyn PartialReflect>,
    /// The value after the edit.
    pub after: Box<dyn PartialReflect>,
}

impl SetComponent {
    fn write(&self, world: &mut World, value: &dyn PartialReflect) {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let Some(reflect_component) = registry.get_type_data::<ReflectComponent>(self.type_id)
        else {
            return;
        };
        let Ok(entity) = world.get_entity_mut(self.entity) else {
            return;
        };
        reflect_component.apply(entity, value);
    }
}

impl EditorCommand for SetComponent {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn undo(&mut self, world: &mut World) {
        self.write(world, &*self.before);
    }

    fn redo(&mut self, world: &mut World) {
        self.write(world, &*self.after);
    }
}

/// Deletes entities. They are disabled rather than despawned so that undo can bring them back.
pub struct DeleteEntities {
    /// The deleted root entities.
    pub entities: Vec<Entity>,
}

impl DeleteEntities {
    /// Deletes `entities` and returns the command that undoes it.
    pub fn apply(world: &mut World, entities: Vec<Entity>) -> Self {
        let mut command = Self { entities };
        command.redo(world);
        command
    }

    fn set_disabled(&self, world: &mut World, disabled: bool) {
        let mut stack = self.entities.clone();
        while let Some(entity) = stack.pop() {
            let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                continue;
            };
            if let Some(children) = entity_mut.get::<Children>() {
                stack.extend(children.iter());
            }
            if disabled {
                entity_mut.insert(Disabled);
            } else {
                entity_mut.remove::<Disabled>();
            }
        }
    }
}

impl EditorCommand for DeleteEntities {
    fn label(&self) -> String {
        match self.entities.len() {
            1 => "Delete Actor".into(),
            n => format!("Delete {n} Actors"),
        }
    }

    fn undo(&mut self, world: &mut World) {
        self.set_disabled(world, false);
    }

    fn redo(&mut self, world: &mut World) {
        self.set_disabled(world, true);
    }
}

/// Clones the current value of a component through reflection.
pub fn snapshot_component(
    world: &World,
    entity: Entity,
    type_id: TypeId,
) -> Option<Box<dyn PartialReflect>> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let reflect_component = registry.get_type_data::<ReflectComponent>(type_id)?;
    let value = reflect_component.reflect(world.get_entity(entity).ok()?)?;
    Some(match value.reflect_clone() {
        Ok(clone) => clone.into_partial_reflect(),
        Err(_) => value.to_dynamic().ok()?,
    })
}

fn entity_label(world: &World, entity: Entity) -> String {
    world
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string())
        .unwrap_or_else(|| entity.to_string())
}

/// Records one undo step per transform gizmo drag.
fn record_gizmo_drags(world: &mut World, mut was_active: Local<Option<(Entity, Transform)>>) {
    let state = world.resource::<TransformGizmoState>();
    if state.active {
        if was_active.is_none()
            && let Some(entity) = state.entity
        {
            *was_active = Some((entity, state.start_transform));
        }
        return;
    }
    let Some((entity, before)) = was_active.take() else {
        return;
    };
    let Some(after) = world.get::<Transform>(entity).copied() else {
        return;
    };
    if after == before {
        return;
    }
    let label = format!("Transform {}", entity_label(world, entity));
    world.resource_mut::<History>().push(SetComponent {
        label,
        entity,
        type_id: TypeId::of::<Transform>(),
        before: Box::new(before),
        after: Box::new(after),
    });
}

/// Details panel edits waiting to be recorded, keyed by entity and component type path.
#[derive(Resource, Default)]
struct PendingFieldEdits(HashMap<(Entity, String), Box<dyn PartialReflect>>);

/// Snapshots a component before the Details panel writes into it. The inspector applies the
/// edit through commands, so the component still holds its old value while observers run.
fn capture_field_edit(edit: On<FieldEdit>, mut world: DeferredWorld) {
    let key = (edit.entity, edit.component.clone());
    if world.resource::<PendingFieldEdits>().0.contains_key(&key) {
        return;
    }
    let type_id = {
        let registry = world.resource::<AppTypeRegistry>().read();
        registry
            .get_with_type_path(&edit.component)
            .map(TypeRegistration::type_id)
    };
    let Some(before) = type_id.and_then(|id| snapshot_component(&world, edit.entity, id)) else {
        return;
    };
    world
        .resource_mut::<PendingFieldEdits>()
        .0
        .insert(key, before);
}

/// Turns finished Details panel edits into undo steps. An edit is finished once the mouse is
/// released, so dragging a number field records a single step.
fn record_field_edits(world: &mut World) {
    if world
        .resource::<ButtonInput<MouseButton>>()
        .pressed(MouseButton::Left)
        || world.resource::<PendingFieldEdits>().0.is_empty()
    {
        return;
    }
    let pending = core::mem::take(&mut world.resource_mut::<PendingFieldEdits>().0);
    for ((entity, component), before) in pending {
        let type_id = {
            let registry = world.resource::<AppTypeRegistry>().read();
            registry
                .get_with_type_path(&component)
                .map(TypeRegistration::type_id)
        };
        let Some(type_id) = type_id else {
            continue;
        };
        let Some(after) = snapshot_component(world, entity, type_id) else {
            continue;
        };
        if before.reflect_partial_eq(&*after) == Some(true) {
            continue;
        }
        let short = component
            .rsplit("::")
            .next()
            .unwrap_or(&component)
            .to_string();
        let label = format!("Edit {} on {}", short, entity_label(world, entity));
        world.resource_mut::<History>().push(SetComponent {
            label,
            entity,
            type_id,
            before,
            after,
        });
    }
}

/// Registers the undo history and the systems that record edits into it.
pub struct HistoryPlugin;

impl Plugin for HistoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<History>()
            .init_resource::<PendingFieldEdits>()
            .add_observer(capture_field_edit)
            .add_systems(PostUpdate, record_gizmo_drags.after(TransformGizmoSystems))
            .add_systems(Last, record_field_edits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Transform>();
        world.insert_resource(registry);
        world.init_resource::<History>();
        world
    }

    #[test]
    fn set_component_undo_redo() {
        let mut world = world();
        let before = Transform::from_xyz(1.0, 2.0, 3.0);
        let after = Transform::from_xyz(4.0, 5.0, 6.0);
        let entity = world.spawn(after).id();
        world.resource_mut::<History>().push(SetComponent {
            label: "Move".into(),
            entity,
            type_id: TypeId::of::<Transform>(),
            before: Box::new(before),
            after: Box::new(after),
        });

        undo(&mut world);
        assert_eq!(world.get::<Transform>(entity), Some(&before));
        assert_eq!(
            world.resource::<History>().redo_label().as_deref(),
            Some("Move")
        );

        redo(&mut world);
        assert_eq!(world.get::<Transform>(entity), Some(&after));
        assert!(world.resource::<History>().redo_label().is_none());
    }

    #[test]
    fn delete_disables_hierarchy_and_undo_restores_it() {
        let mut world = world();
        let parent = world.spawn_empty().id();
        let child = world.spawn(ChildOf(parent)).id();

        let command = DeleteEntities::apply(&mut world, vec![parent]);
        world.resource_mut::<History>().push(command);
        assert!(world.entity(parent).contains::<Disabled>());
        assert!(world.entity(child).contains::<Disabled>());

        undo(&mut world);
        assert!(!world.entity(parent).contains::<Disabled>());
        assert!(!world.entity(child).contains::<Disabled>());
    }

    #[test]
    fn push_clears_redo() {
        let mut world = world();
        let entity = world.spawn(Transform::default()).id();
        let command = || SetComponent {
            label: "Move".into(),
            entity,
            type_id: TypeId::of::<Transform>(),
            before: Box::new(Transform::default()),
            after: Box::new(Transform::default()),
        };
        world.resource_mut::<History>().push(command());
        undo(&mut world);
        world.resource_mut::<History>().push(command());
        assert!(world.resource::<History>().redo_label().is_none());
    }
}
