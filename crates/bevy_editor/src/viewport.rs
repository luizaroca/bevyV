//! The level viewport: the editor camera, Unreal-style navigation, and the grid.
//!
//! The 3D camera renders into the rectangle of the viewport panel, and a second camera draws the
//! editor UI over the whole window.
//!
//! Navigation, as in Unreal's level viewport:
//! - Right mouse + mouse: look around. While held, W/A/S/D move, Q/E move down/up, and the mouse
//!   wheel changes the camera speed.
//! - Left mouse drag on empty space: move forward/back and turn.
//! - Middle mouse drag: pan.
//! - Alt + left mouse: orbit. Alt + right mouse: dolly. Alt + middle mouse: pan.
//! - Mouse wheel: dolly forward/back.
//! - F: frame the selection.

use bevy::{
    camera::{primitives::Aabb, visibility::RenderLayers, Viewport},
    dev_tools::infinite_grid::{InfiniteGrid, InfiniteGridSettings},
    gizmos::transform_gizmo::{TransformGizmoCamera, TransformGizmoState},
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    inspector::InspectorSelection,
    math::ops,
    prelude::*,
    ui::{ComputedNode, UiGlobalTransform},
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{selection::SelectionSystems, EditorEntity};

/// Marks the UI node whose rectangle the level viewport renders into.
#[derive(Component, Default, Clone, Copy)]
pub struct EditorViewportArea;

/// The camera that renders the level.
#[derive(Component)]
pub struct EditorCamera {
    /// Rotation around the world up axis, in radians.
    pub yaw: f32,
    /// Rotation up and down, in radians.
    pub pitch: f32,
    /// Flying speed in units per second, changed with the mouse wheel while flying.
    pub speed: f32,
    /// The point Alt + left mouse orbits around.
    pub pivot: Vec3,
    /// Where the camera is gliding to after an F (frame selection) press.
    pub glide_to: Option<Vec3>,
}

/// The camera that draws the editor UI.
#[derive(Component)]
pub struct EditorUiCamera;

/// The level viewport's rectangle in logical window pixels, updated from the layout.
#[derive(Resource, Default)]
pub struct ViewportRect(pub Option<Rect>);

/// Set for one frame after a mouse button that navigated the camera is released, so that the
/// release does not also count as a click that changes the selection.
#[derive(Resource, Default)]
pub struct SuppressClick(pub bool);

/// The kind of mouse navigation in progress.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Drag {
    Fly,
    Walk,
    Pan,
    Orbit,
    Dolly,
}

/// Navigation state that lasts across frames.
#[derive(Resource, Default)]
struct Navigation {
    drag: Option<(Drag, MouseButton)>,
    moved: f32,
    grabbed: Option<(CursorGrabMode, bool)>,
}

/// The editor UI renders on its own render layer so it never shows level meshes.
const UI_LAYER: usize = 31;

const LOOK_SENSITIVITY: f32 = 0.004;
const MIN_SPEED: f32 = 0.5;
const MAX_SPEED: f32 = 200.0;
/// How far the mouse may move, in pixels, before a press counts as a drag rather than a click.
const DRAG_THRESHOLD: f32 = 4.0;

fn spawn_cameras(mut commands: Commands) {
    let position = Vec3::new(-6.0, 4.5, 9.0);
    let transform = Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y);
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    commands.spawn((
        Name::new("Editor Camera"),
        EditorEntity,
        EditorCamera {
            yaw,
            pitch,
            speed: 8.0,
            pivot: Vec3::ZERO,
            glide_to: None,
        },
        Camera3d::default(),
        Camera {
            order: 0,
            ..default()
        },
        transform,
        TransformGizmoCamera,
        InfiniteGridSettings::default(),
    ));
    commands.spawn((
        Name::new("Editor UI Camera"),
        EditorEntity,
        EditorUiCamera,
        Camera2d,
        Camera {
            // Above the level camera (0) and the transform gizmo's overlay camera (1).
            order: 10,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        IsDefaultUiCamera,
        RenderLayers::layer(UI_LAYER),
    ));
    commands.spawn((Name::new("Editor Grid"), EditorEntity, InfiniteGrid));
}

/// Fits the level camera to the viewport panel.
fn fit_camera_to_viewport(
    area: Query<(&ComputedNode, &UiGlobalTransform), With<EditorViewportArea>>,
    mut cameras: Query<&mut Camera, With<EditorCamera>>,
    mut rect: ResMut<ViewportRect>,
) {
    let Ok((node, transform)) = area.single() else {
        return;
    };
    let size = node.size();
    if size.x < 1.0 || size.y < 1.0 {
        return;
    }
    let min = transform.translation - size * 0.5;
    let scale = node.inverse_scale_factor();
    let logical = Rect::from_corners(min * scale, (min + size) * scale);
    if rect.0 != Some(logical) {
        rect.0 = Some(logical);
    }
    let viewport = Viewport {
        physical_position: min.max(Vec2::ZERO).as_uvec2(),
        physical_size: size.as_uvec2(),
        ..default()
    };
    for mut camera in &mut cameras {
        let unchanged = camera.viewport.as_ref().is_some_and(|current| {
            current.physical_position == viewport.physical_position
                && current.physical_size == viewport.physical_size
        });
        if !unchanged {
            camera.viewport = Some(viewport.clone());
        }
    }
}

fn cursor_in_viewport(window: &Window, rect: &ViewportRect) -> bool {
    match (window.cursor_position(), rect.0) {
        (Some(cursor), Some(rect)) => rect.contains(cursor),
        _ => false,
    }
}

/// Unreal-style viewport navigation.
fn navigate(
    mut cameras: Query<(&mut EditorCamera, &mut Transform)>,
    mut window: Single<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    rect: Res<ViewportRect>,
    gizmo: Res<TransformGizmoState>,
    time: Res<Time>,
    mut nav: ResMut<Navigation>,
    mut suppress: ResMut<SuppressClick>,
) {
    suppress.0 = false;
    let Ok((mut camera, mut transform)) = cameras.single_mut() else {
        return;
    };
    let (window, cursor_options) = &mut *window;
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let over_viewport = cursor_in_viewport(window, &rect);

    // Start a drag.
    if nav.drag.is_none() && over_viewport {
        let started = if mouse.just_pressed(MouseButton::Right) {
            Some((
                if alt { Drag::Dolly } else { Drag::Fly },
                MouseButton::Right,
            ))
        } else if mouse.just_pressed(MouseButton::Middle) {
            Some((Drag::Pan, MouseButton::Middle))
        } else if mouse.just_pressed(MouseButton::Left)
            && gizmo.hovered_axis.is_none()
            && !gizmo.active
        {
            Some((
                if alt { Drag::Orbit } else { Drag::Walk },
                MouseButton::Left,
            ))
        } else {
            None
        };
        if started.is_some() {
            nav.drag = started;
            nav.moved = 0.0;
        }
    }

    // End a drag.
    if let Some((_, button)) = nav.drag
        && !mouse.pressed(button)
    {
        if nav.moved > DRAG_THRESHOLD {
            suppress.0 = true;
        }
        nav.drag = None;
        if let Some((mode, visible)) = nav.grabbed.take() {
            cursor_options.grab_mode = mode;
            cursor_options.visible = visible;
        }
    }

    let delta = motion.delta;
    let wheel = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    let dt = time.delta_secs();

    if let Some((drag, _)) = nav.drag {
        nav.moved += delta.length();
        // Left-drag navigation only starts once the mouse has moved, so plain clicks still select.
        let navigating = drag != Drag::Walk || nav.moved > DRAG_THRESHOLD;
        if navigating && nav.grabbed.is_none() && matches!(drag, Drag::Fly | Drag::Orbit) {
            nav.grabbed = Some((cursor_options.grab_mode, cursor_options.visible));
            cursor_options.grab_mode = CursorGrabMode::Locked;
            cursor_options.visible = false;
        }
        if navigating {
            camera.glide_to = None;
        }
        match drag {
            Drag::Fly => {
                camera.yaw -= delta.x * LOOK_SENSITIVITY;
                camera.pitch = (camera.pitch - delta.y * LOOK_SENSITIVITY).clamp(-1.54, 1.54);
                if wheel != 0.0 {
                    camera.speed =
                        (camera.speed * ops::powf(1.2, wheel)).clamp(MIN_SPEED, MAX_SPEED);
                }
                let mut input = Vec3::ZERO;
                if keys.pressed(KeyCode::KeyW) {
                    input.z -= 1.0;
                }
                if keys.pressed(KeyCode::KeyS) {
                    input.z += 1.0;
                }
                if keys.pressed(KeyCode::KeyA) {
                    input.x -= 1.0;
                }
                if keys.pressed(KeyCode::KeyD) {
                    input.x += 1.0;
                }
                if keys.pressed(KeyCode::KeyE) {
                    input.y += 1.0;
                }
                if keys.pressed(KeyCode::KeyQ) {
                    input.y -= 1.0;
                }
                let boost = if keys.pressed(KeyCode::ShiftLeft) {
                    3.0
                } else {
                    1.0
                };
                let rotation = Quat::from_euler(EulerRot::YXZ, camera.yaw, camera.pitch, 0.0);
                let movement = rotation * Vec3::new(input.x, 0.0, input.z) + Vec3::Y * input.y;
                transform.translation += movement.normalize_or_zero() * camera.speed * boost * dt;
            }
            Drag::Walk if navigating => {
                camera.yaw -= delta.x * LOOK_SENSITIVITY;
                let forward = Quat::from_rotation_y(camera.yaw) * Vec3::NEG_Z;
                transform.translation -= forward * delta.y * camera.speed * 0.01;
            }
            Drag::Walk => {}
            Drag::Pan => {
                let right = transform.right();
                let up = transform.up();
                let scale = camera.speed * 0.004;
                transform.translation += (-right * delta.x + up * delta.y) * scale;
            }
            Drag::Orbit => {
                let offset = transform.translation - camera.pivot;
                let distance = offset.length().max(0.1);
                camera.yaw -= delta.x * LOOK_SENSITIVITY;
                camera.pitch = (camera.pitch - delta.y * LOOK_SENSITIVITY).clamp(-1.54, 1.54);
                let rotation = Quat::from_euler(EulerRot::YXZ, camera.yaw, camera.pitch, 0.0);
                transform.translation = camera.pivot + rotation * Vec3::Z * distance;
            }
            Drag::Dolly => {
                let forward = transform.forward();
                // Maya's direction: dragging right or down dollies in.
                transform.translation += forward * (delta.x + delta.y) * camera.speed * 0.01;
            }
        }
    } else if over_viewport && wheel != 0.0 {
        let forward = transform.forward();
        transform.translation += forward * wheel * camera.speed * 0.25;
        camera.glide_to = None;
    }

    // Glide towards a framed selection.
    if let Some(target) = camera.glide_to {
        let t = 1.0 - ops::exp(-12.0 * dt);
        transform.translation = transform.translation.lerp(target, t);
        if transform.translation.distance(target) < 0.01 {
            transform.translation = target;
            camera.glide_to = None;
        }
    }

    transform.rotation = Quat::from_euler(EulerRot::YXZ, camera.yaw, camera.pitch, 0.0);

    // Keep the orbit pivot in front of the camera when it is not set by framing.
    if nav.drag != Some((Drag::Orbit, MouseButton::Left)) && camera.glide_to.is_none() {
        let distance = camera
            .pivot
            .distance(transform.translation)
            .clamp(1.0, 100.0);
        if nav.drag.is_some() {
            camera.pivot = transform.translation + transform.forward() * distance;
        }
    }
}

/// Frames the selection (the F key in Unreal).
pub fn frame_selection(world: &mut World) {
    let Some(selected) = world.resource::<InspectorSelection>().0 else {
        return;
    };
    let Some(center) = world
        .get::<GlobalTransform>(selected)
        .map(GlobalTransform::translation)
    else {
        return;
    };

    // Grow a bounding sphere over the selection and its descendants.
    let mut radius: f32 = 0.5;
    let mut stack = vec![selected];
    while let Some(entity) = stack.pop() {
        if let (Some(aabb), Some(transform)) = (
            world.get::<Aabb>(entity),
            world.get::<GlobalTransform>(entity),
        ) {
            let world_center = transform.transform_point(Vec3::from(aabb.center));
            let scale = transform.scale().abs().max_element();
            let extent = Vec3::from(aabb.half_extents).length() * scale;
            radius = radius.max(world_center.distance(center) + extent);
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }

    let mut cameras = world.query::<(&mut EditorCamera, &Transform)>();
    let Ok((mut camera, transform)) = cameras.single_mut(world) else {
        return;
    };
    let distance = radius * 2.5 + 0.5;
    camera.pivot = center;
    camera.glide_to = Some(center - transform.forward() * distance);
}

/// Registers the viewport camera and navigation.
pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewportRect>()
            .init_resource::<SuppressClick>()
            .init_resource::<Navigation>()
            .add_systems(Startup, spawn_cameras)
            .add_systems(Update, navigate.before(SelectionSystems))
            .add_systems(
                PostUpdate,
                fit_camera_to_viewport.after(bevy::ui::UiSystems::Layout),
            );
    }
}
