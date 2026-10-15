# Bevy Extend

This is a fork of [Bevy](https://github.com/bevyengine/bevy) that adds a friendlier UI, an
editor, and extra 3D rendering features, one feature at a time.

## Layout

Everything this fork adds lives in its own crates, so upstream files stay untouched and pulling
in Bevy updates rarely conflicts:

- `crates/bevy_extend`: the fork's plugins. Add `bevy_extend::ExtendPlugins` to an app to use them.
- `crates/bevy_extend_sandbox`: a small 3D scene for trying features as they land.
  Run it with `cargo run -p bevy_extend_sandbox`.
  Extra test scenes live in `crates/bevy_extend_sandbox/src/bin`, for example the soft shadows
  test: `cargo run -p bevy_extend_sandbox --bin soft_shadows --release`.
  The SSGI test (a Cornell box): `cargo run -p bevy_extend_sandbox --bin ssgi --release`.

When a feature does need to change an upstream file, keep the change small and mention it in the PR.

## Staying up to date with Bevy

```sh
git remote add upstream https://github.com/bevyengine/bevy.git   # once
git fetch upstream
git merge upstream/main
```

## Releases

Fork releases are git tags (`vX.Y.Z`); crate versions keep following Bevy's.

- `v0.2.1`: the level editor (`crates/bevy_editor`, `crates/bevy_inspector`) plus contact-hardening
  soft shadows for point, spot and directional lights. Try both with
  `cargo run -p bevy_editor --release -- assets/levels/soft_shadows_all.level.ron`.
- `v0.3.0`: screen-space global illumination (`crates/bevy_extend/src/ssgi`): light bouncing off
  nearby surfaces, with ambient occlusion, denoising and an optional half-resolution trace. The
  editor gains World Settings (toolbar button) with SSAO and SSGI toggles and SSGI's settings,
  saved in the level. Try it with
  `cargo run -p bevy_editor --release -- assets/levels/ssgi.level.ron`.
