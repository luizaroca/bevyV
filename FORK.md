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
  The SSR test (a polished floor of six roughnesses): `cargo run -p bevy_extend_sandbox --bin ssr --release`.

Upstream files changed for screen-space reflections (`crates/bevy_extend/src/ssr`): the normal
prepass is `Rgba16Float` instead of `Rgb10a2Unorm` (`bevy_core_pipeline/src/prepass/mod.rs`), and
`StandardMaterial` writes its perceptual roughness into its alpha (`bevy_pbr/src/render/pbr_prepass.wesl`
and `bevy_pbr/src/deferred/functions.wesl`). Reflections reach lighting through a `ScreenSpaceSpecular` hook in
`bevy_pbr` (`screen_space_indirect_light.rs`, view binding 40, used in `pbr_functions.wesl`), next to
SSGI's `ScreenSpaceIndirectLight` hook.

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
- `v0.4.0`: screen-space reflections (`crates/bevy_extend/src/ssr`) that work with forward and
  deferred rendering, reflect the sky and see-through objects (glass), and are denoised so they
  work without TAA. World Settings gains Reflections and Transparent Reflections toggles and
  the reflection settings, saved in the level; editor shapes whose colour alpha is below 1 are
  see-through. Try it with `cargo run -p bevy_editor --release -- assets/levels/ssr.level.ron`.
