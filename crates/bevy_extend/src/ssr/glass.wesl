//! Screen-space reflections: draws the depth of see-through meshes into the glass layer (see
//! `glass.rs`).

import bevy_render::view::View;

// Keep in sync with `GlassDraw` in `glass.rs`.
struct GlassDraw {
    world_from_local: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> draw: GlassDraw;
@group(0) @binding(2) var opaque_depth: texture_depth_2d;

@vertex
fn vertex(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return view.clip_from_world * draw.world_from_local * vec4(position, 1.0);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) {
    // Reverse Z: behind an opaque surface means a smaller depth. That glass can't be seen, and
    // keeping it would let rays reflect it through the surface in front.
    if (position.z < textureLoad(opaque_depth, vec2<i32>(position.xy), 0)) {
        discard;
    }
}
