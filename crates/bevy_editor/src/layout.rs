//! The editor window layout, following Unreal's default level editor layout:
//!
//! ```text
//! +--------------------------------------------------------------+
//! | File  Edit  Play  Help                          (menu bar)   |
//! | [Select][Move][Rotate][Scale] [Local] [Snap]  [Play][Stop]   |
//! +-----------------------------------------+--------------------+
//! |                                         | Outliner           |
//! |               Viewport                  +--------------------+
//! |                                         | Details            |
//! +-----------------------------------------+--------------------+
//! | status bar                                                   |
//! +--------------------------------------------------------------+
//! ```

use bevy::{
    app::AppExit,
    feathers::{
        containers::{flex_spacer, pane_header_divider},
        controls::{
            ButtonVariant, FeathersMenu, FeathersMenuButton, FeathersMenuDivider, FeathersMenuItem,
            FeathersMenuPopup, FeathersPane, FeathersSplitPane, FeathersSplitPaneHandle,
            FeathersToolButton,
        },
        display::{caption, label, label_dim},
        theme::ThemeBackgroundColor,
        tokens,
    },
    gizmos::transform_gizmo::{TransformGizmoMode, TransformGizmoSettings, TransformGizmoSpace},
    inspector::{details_panel::details_panel, entity_tree::entity_tree_panel},
    prelude::*,
    ui::InteractionDisabled,
    ui_widgets::{split_pane_self_update, Activate, ControlOrientation},
};

use crate::{
    history::{self, History},
    level,
    play::{self, EditorMode},
    selection::GizmoVisible,
    shortcuts,
    viewport::{self, EditorCamera, EditorViewportArea},
    EditorEntity,
};

/// A toolbar button that switches the gizmo mode. `None` is Unreal's Select mode.
#[derive(Component, Default, Clone, Copy)]
struct ModeButton(Option<TransformGizmoMode>);

/// The toolbar button that toggles local-space gizmo editing.
#[derive(Component, Default, Clone, Copy)]
struct LocalSpaceButton;

/// The toolbar button that toggles grid snapping.
#[derive(Component, Default, Clone, Copy)]
struct SnapButton;

/// The toolbar Play button.
#[derive(Component, Default, Clone, Copy)]
struct PlayButton;

/// The toolbar Stop button.
#[derive(Component, Default, Clone, Copy)]
struct StopButton;

/// The text in the status bar.
#[derive(Component, Default, Clone, Copy)]
struct StatusText;

/// The overlay text in the corner of the viewport.
#[derive(Component, Default, Clone, Copy)]
struct ViewportInfoText;

/// Translation, rotation (degrees) and scale increments used when snapping is on.
const SNAP: (f32, f32, f32) = (0.5, 15.0, 0.25);

fn menu_bar() -> impl Scene {
    bsn! {
        Node {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(2),
            padding: px(4),
        }
        ThemeBackgroundColor(tokens::WINDOW_BG)
        Children [
            @FeathersMenu
            Children [
                @FeathersMenuButton { @caption: bsn! { @caption("File") } }
                --
                @FeathersMenuPopup
                Children [
                    @FeathersMenuItem { @caption: bsn! { @caption("New Level    Ctrl+N") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(level::new_level))
                    --
                    @FeathersMenuItem { @caption: bsn! { @caption("Open Level...    Ctrl+O") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(level::open_level))
                    --
                    @FeathersMenuDivider
                    --
                    @FeathersMenuItem { @caption: bsn! { @caption("Save Level    Ctrl+S") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(level::save_level))
                    --
                    @FeathersMenuItem { @caption: bsn! { @caption("Save Level As...    Ctrl+Shift+S") } }
                    on(|_: On<Activate>, mut commands: Commands| {
                        commands.queue(level::save_level_as);
                    })
                    --
                    @FeathersMenuDivider
                    --
                    @FeathersMenuItem { @caption: bsn! { @caption("Exit") } }
                    on(|_: On<Activate>, mut exit: MessageWriter<AppExit>| {
                        exit.write(AppExit::Success);
                    })
                ]
            ]
            --
            @FeathersMenu
            Children [
                @FeathersMenuButton { @caption: bsn! { @caption("Edit") } }
                --
                @FeathersMenuPopup
                Children [
                    @FeathersMenuItem { @caption: bsn! { @caption("Undo    Ctrl+Z") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(history::undo))
                    --
                    @FeathersMenuItem { @caption: bsn! { @caption("Redo    Ctrl+Y") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(history::redo))
                    --
                    @FeathersMenuDivider
                    --
                    @FeathersMenuItem { @caption: bsn! { @caption("Delete    Del") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(shortcuts::delete_selection))
                    --
                    @FeathersMenuItem { @caption: bsn! { @caption("Frame Selection    F") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(viewport::frame_selection))
                ]
            ]
            --
            @FeathersMenu
            Children [
                @FeathersMenuButton { @caption: bsn! { @caption("Play") } }
                --
                @FeathersMenuPopup
                Children [
                    @FeathersMenuItem { @caption: bsn! { @caption("Play / Stop    Alt+P") } }
                    on(|_: On<Activate>, mut commands: Commands| commands.queue(play::toggle_play))
                ]
            ]
            --
            @FeathersMenu
            Children [
                @FeathersMenuButton { @caption: bsn! { @caption("Help") } }
                --
                @FeathersMenuPopup
                Children [
                    @FeathersMenuItem { @caption: bsn! { @caption("Keyboard Shortcuts") } }
                    on(|_: On<Activate>| {
                        info!("{}", SHORTCUT_HELP);
                    })
                ]
            ]
        ]
    }
}

const SHORTCUT_HELP: &str = "Viewport: RMB+mouse look, RMB+WASD/QE fly, RMB+wheel speed, \
    LMB drag walk, MMB pan, Alt+LMB orbit, Alt+RMB dolly, wheel dolly, F frame. \
    Tools: Q select, W move, E rotate, R scale, Space cycle, Ctrl+` local/world. \
    Edit: Ctrl+Z undo, Ctrl+Y redo, Del delete.     File: Ctrl+N new, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as. Play: Alt+P play/stop, Esc stop.";

fn toolbar() -> impl Scene {
    bsn! {
        Node {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(4),
            padding: px(4),
        }
        ThemeBackgroundColor(tokens::WINDOW_BG)
        Children [
            @FeathersToolButton { @caption: bsn! { @caption("Select") } }
            ModeButton({None})
            on(|_: On<Activate>, mut commands: Commands| commands.queue(shortcuts::set_select_mode))
            --
            @FeathersToolButton { @caption: bsn! { @caption("Move") } }
            ModeButton({Some(TransformGizmoMode::Translate)})
            on(|_: On<Activate>, mut commands: Commands| {
                commands.queue(|world: &mut World| {
                    shortcuts::set_gizmo_mode(world, TransformGizmoMode::Translate);
                });
            })
            --
            @FeathersToolButton { @caption: bsn! { @caption("Rotate") } }
            ModeButton({Some(TransformGizmoMode::Rotate)})
            on(|_: On<Activate>, mut commands: Commands| {
                commands.queue(|world: &mut World| {
                    shortcuts::set_gizmo_mode(world, TransformGizmoMode::Rotate);
                });
            })
            --
            @FeathersToolButton { @caption: bsn! { @caption("Scale") } }
            ModeButton({Some(TransformGizmoMode::Scale)})
            on(|_: On<Activate>, mut commands: Commands| {
                commands.queue(|world: &mut World| {
                    shortcuts::set_gizmo_mode(world, TransformGizmoMode::Scale);
                });
            })
            --
            @pane_header_divider()
            --
            @FeathersToolButton { @caption: bsn! { @caption("Local Space") } }
            LocalSpaceButton
            on(|_: On<Activate>, mut commands: Commands| commands.queue(shortcuts::toggle_gizmo_space))
            --
            @FeathersToolButton { @caption: bsn! { @caption("Snap") } }
            SnapButton
            on(|_: On<Activate>, mut settings: ResMut<TransformGizmoSettings>| {
                let on = settings.snap_translate.is_none();
                settings.snap_translate = on.then_some(SNAP.0);
                settings.snap_rotate = on.then_some(SNAP.1.to_radians());
                settings.snap_scale = on.then_some(SNAP.2);
            })
            --
            @flex_spacer()
            --
            @FeathersToolButton {
                @caption: bsn! { @caption("Play") },
                @variant: ButtonVariant::Primary,
            }
            PlayButton
            on(|_: On<Activate>, mut commands: Commands| commands.queue(play::toggle_play))
            --
            @FeathersToolButton { @caption: bsn! { @caption("Stop") } }
            StopButton
            InteractionDisabled
            on(|_: On<Activate>, mut commands: Commands| commands.queue(play::toggle_play))
            --
            @flex_spacer()
        ]
    }
}

fn editor_window() -> impl Scene {
    bsn! {
        Name::new("Editor UI")
        EditorEntity
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
        }
        Pickable::IGNORE
        Children [
            @menu_bar()
            --
            @toolbar()
            --
            @FeathersSplitPane
            Node {
                flex_grow: 1.0,
                min_height: px(0),
            }
            Pickable::IGNORE
            on(split_pane_self_update)
            Children [
                @FeathersPane { @size: 3.0, @min_size: 200.0 }
                Pickable::IGNORE
                Children [
                    EditorViewportArea
                    Node {
                        flex_grow: 1.0,
                        padding: px(8),
                    }
                    Pickable::IGNORE
                    Children [
                        @label_dim("Perspective  |  Lit")
                        ViewportInfoText
                        Pickable::IGNORE
                    ]
                ]
                --
                @FeathersSplitPaneHandle
                --
                @FeathersPane { @size: 1.0, @min_size: 280.0 }
                ThemeBackgroundColor(tokens::WINDOW_BG)
                Children [
                    @FeathersSplitPane { @orientation: ControlOrientation::Vertical }
                    Node {
                        flex_grow: 1.0,
                        min_height: px(0),
                        padding: px(4),
                    }
                    on(split_pane_self_update)
                    Children [
                        @FeathersPane { @size: 1.0, @min_size: 120.0 }
                        Children [
                            @entity_tree_panel()
                            Node {
                                width: Val::Auto,
                                flex_grow: 1.0,
                                min_height: px(0),
                            }
                        ]
                        --
                        @FeathersSplitPaneHandle
                        --
                        @FeathersPane { @size: 2.0, @min_size: 160.0 }
                        Children [
                            @details_panel()
                            Node {
                                width: Val::Auto,
                                flex_grow: 1.0,
                                min_height: px(0),
                            }
                        ]
                    ]
                ]
            ]
            --
            Node {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                padding: px(4),
            }
            ThemeBackgroundColor(tokens::WINDOW_BG)
            Children [
                @label("Ready")
                StatusText
            ]
        ]
    }
}

/// Highlights the active tool and toggles, and enables Play or Stop.
fn update_toolbar(
    settings: Res<TransformGizmoSettings>,
    visible: Res<GizmoVisible>,
    mode: Res<State<EditorMode>>,
    mut mode_buttons: Query<(&ModeButton, &mut ButtonVariant)>,
    mut local: Query<&mut ButtonVariant, (With<LocalSpaceButton>, Without<ModeButton>)>,
    mut snap: Query<
        &mut ButtonVariant,
        (
            With<SnapButton>,
            Without<ModeButton>,
            Without<LocalSpaceButton>,
        ),
    >,
    play: Query<Entity, With<PlayButton>>,
    stop: Query<Entity, With<StopButton>>,
    mut commands: Commands,
) {
    for (button, mut variant) in &mut mode_buttons {
        let active = match button.0 {
            None => !visible.0,
            Some(mode) => visible.0 && settings.mode == mode,
        };
        variant.set_if_neq(ButtonVariant::selected(active));
    }
    for mut variant in &mut local {
        variant.set_if_neq(ButtonVariant::selected(
            settings.space == TransformGizmoSpace::Local,
        ));
    }
    for mut variant in &mut snap {
        variant.set_if_neq(ButtonVariant::selected(settings.snap_translate.is_some()));
    }
    if mode.is_changed() {
        let playing = *mode.get() == EditorMode::Playing;
        for entity in &play {
            if playing {
                commands.entity(entity).insert(InteractionDisabled);
            } else {
                commands.entity(entity).remove::<InteractionDisabled>();
            }
        }
        for entity in &stop {
            if playing {
                commands.entity(entity).remove::<InteractionDisabled>();
            } else {
                commands.entity(entity).insert(InteractionDisabled);
            }
        }
    }
}

/// Shows the camera speed and the next undo step in the status bar.
fn update_status(
    history: Res<History>,
    mode: Res<State<EditorMode>>,
    cameras: Query<&EditorCamera>,
    mut status: Query<&mut Text, With<StatusText>>,
    mut info: Query<&mut Text, (With<ViewportInfoText>, Without<StatusText>)>,
) {
    let speed = cameras
        .single()
        .map(|camera| camera.speed)
        .unwrap_or_default();
    let undo = history
        .undo_label()
        .map(|label| format!("Undo: {label}"))
        .unwrap_or_else(|| "Nothing to undo".into());
    let mode = match mode.get() {
        EditorMode::Editing => "Editing",
        EditorMode::Playing => "PLAYING (Esc to stop)",
    };
    let text = format!("{mode}    |    {undo}    |    Help > Keyboard Shortcuts");
    for mut status in &mut status {
        if status.0 != text {
            status.0.clone_from(&text);
        }
    }
    let text = format!("Perspective  |  Lit  |  Camera speed {speed:.1}");
    for mut info in &mut info {
        if info.0 != text {
            info.0.clone_from(&text);
        }
    }
}

/// Builds the editor window.
pub struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, editor_window.spawn())
            .add_systems(Update, (update_toolbar, update_status));
    }
}
