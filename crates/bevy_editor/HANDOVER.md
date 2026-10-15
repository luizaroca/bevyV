# Editor handover

Updated at the end of every work session. Newest session first.

## Ground rules

- All work stays in the fork `luizaroca/bevyV`. Never push, open PRs, file issues or comment
  on `bevyengine/bevy`; `upstream` is fetch-only (its push URL is disabled locally).
- Unreal Engine is the benchmark (the owner has 15+ years of UE experience). Viewport Alt
  navigation follows Maya.
- Prefer new code in `crates/bevy_editor`. Keep changes to Bevy's own crates small and listed
  below, so pulling in upstream stays easy.

## How to run and test

- Double-click `F:\BevyV\Run Bevy Editor.bat` (local only, excluded from git), or
  `cargo run -p bevy_editor`.
- Unattended checks: environment variables read by `src/main.rs`.
  - `BEVY_EDITOR_SCREENSHOT=<png>` saves a screenshot after 4 s and exits.
  - `BEVY_EDITOR_SELECT=<name>` selects that entity first.
  - `BEVY_EDITOR_ROUNDTRIP=<file>` saves the level and reopens it first.
- `cargo test -p bevy_editor -p bevy_inspector -p bevy_gizmos` and
  `cargo clippy -p bevy_editor --all-targets` before committing.

## Changes to Bevy's own crates

- `bevy_inspector`: `InspectorRootFilter` (which roots the entity tree lists) and
  `InspectorComponentOrder` (Details panel order and hiding).
- `bevy_gizmos_render`: the transform gizmo's overlay camera no longer clears the colour buffer
  (it erased the scene behind the gizmo).

## Known Bevy issues worked around

- Loading a world file inserts `ChildOf` before remapping its entity, so parents miss their
  children. `level::load_level_from` re-inserts `ChildOf` after loading.
- `Camera::viewport_to_world` / `world_to_viewport` already handle viewport offsets; do not
  subtract the viewport origin (that broke gizmo picking once).

---

## Session 2026-10-15

**Branch:** `editor/soft-shadows` (off `editor/foundation`, which now has main's soft shadows merged in).

**Done**
- Turned on `experimental_pbr_pcss` for the editor, so lights' soft shadow settings work.
- `cargo run -p bevy_editor -- <level file>` opens that level at startup.
- Test level `assets/levels/soft_shadows.level.ron`: box, cylinder, sphere, a point light (on),
  and a spot light and sun (hidden; set Visibility to switch).
- Levels no longer save the sun's computed `Cascades`.

---

## Session 2026-10-13

**Branch:** `editor/foundation` (latest commit pushed to the fork).

**Done**
- Cloned the fork to `F:\BevyV`, added `upstream` (fetch-only).
- Assessment of Bevy's editor story against Unreal.
- New crate `bevy_editor`:
  - Unreal layout: menu bar, toolbar, viewport, Outliner, Details, status bar.
  - Viewport navigation: RMB fly + WASD/QE, LMB walk, MMB pan, Alt orbit/dolly/pan (Maya
    direction for dolly), wheel, F to frame.
  - Click selection (viewport and Outliner), orange selection bounds, gizmo with Q/W/E/R/Space,
    local/world, snapping.
  - Undo/redo for gizmo drags, Details edits and Delete.
  - Play/Stop (Alt+P, Esc) restoring the level.
  - New/Open/Save/Save As `.level.ron` levels with native dialogs; `*` in the title for unsaved
    changes.
  - `Primitive` and `PrimitiveMaterial` saveable shapes.
  - Details panel in Unreal's order with computed components hidden.
- The owner tested by hand: gizmo and controls work after the picking fix.

**Not done / known gaps**
- Imported glTF models are not saved in levels.
- Closing the window does not prompt to save.
- Outliner lists entities in reverse order after reopening a level.

**Next (agreed)**
1. Multi-select (Ctrl/Shift-click, box select) and Duplicate (Ctrl+D, Alt+drag).
2. Place Actors panel (cube, sphere, lights, camera; drag into the viewport).
3. Then: Content Browser and glTF import, dockable tabs.
