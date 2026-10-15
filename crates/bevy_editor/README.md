# Bevy Editor

A scene editor for Bevy, modelled on the Unreal Engine level editor.

```bash
cargo run -p bevy_editor
```

## What works

- Unreal-style layout: menu bar, toolbar, level viewport, Outliner, Details panel, status bar.
  The right-hand panels can be resized with the split handles.
- Viewport navigation matching Unreal's defaults (see below).
- Click to select in the viewport or the Outliner. The selection is outlined in orange.
- Transform gizmo with Move/Rotate/Scale, world/local space and grid snapping.
- Details panel editing of any reflected component.
- Undo/redo for gizmo drags, Details panel edits and deletes.
- Play/Stop: gameplay systems gated with `in_play_mode` run while playing, and the level is
  restored on Stop.
- New / Open / Save / Save As levels (`.level.ron` files) through native file dialogs, with
  the level name and an unsaved-changes `*` in the window title.
- Basic shapes (`Primitive`: cube, sphere, cylinder, cone, plane, capsule) with a
  `PrimitiveMaterial`; edit their size and colour in the Details panel and the mesh updates.
- Details panel in Unreal's order (Name, Transform, shape, material, lights, then gameplay
  components), hiding engine-computed components.

## Controls

| Input | Action |
| --- | --- |
| RMB + mouse | Look around |
| RMB + W/A/S/D, Q/E | Fly; Q/E move down/up; Shift is faster |
| RMB + wheel | Change camera speed |
| LMB drag on empty space | Move forward/back and turn |
| MMB drag | Pan |
| Alt + LMB / RMB / MMB | Orbit / dolly (Maya direction: right or down moves in) / pan |
| Wheel | Dolly |
| F | Frame selection |
| Q / W / E / R, Space | Select / Move / Rotate / Scale, cycle |
| Ctrl + \` | Toggle world/local space |
| Delete | Delete selection |
| Ctrl + Z, Ctrl + Y | Undo, redo |
| Ctrl + N, Ctrl + O | New level, open level |
| Ctrl + S, Ctrl + Shift + S | Save, save as |
| Alt + P, Esc | Play/Stop, stop |

## Using it with your own level

Add `EditorPlugin` after `DefaultPlugins`, and mark the root entities of your level with
`LevelEntity`. Gate gameplay systems with `.run_if(in_play_mode)`. Use `Primitive` and
`PrimitiveMaterial` instead of meshes and materials created in code, which cannot be saved.

## Not done yet

Multi-selection, a Content Browser, spawning actors from a palette,
duplicate (Ctrl+D), dockable/rearrangeable tabs, and saving imported models (glTF) in levels, and a prompt for unsaved changes when closing the
window.
