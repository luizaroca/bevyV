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

When a feature does need to change an upstream file, keep the change small and mention it in the PR.

## Staying up to date with Bevy

```sh
git remote add upstream https://github.com/bevyengine/bevy.git   # once
git fetch upstream
git merge upstream/main
```
