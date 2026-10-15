//! Screen-space global illumination, phase 1: radiance history and its debug views.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

@group(0) @binding(0) var lit_image: texture_2d<f32>;
@group(0) @binding(1) var history: texture_2d<f32>;
@group(0) @binding(2) var motion_vectors: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;

/// Copies this frame's lit image into the history texture.
@fragment
fn copy_history(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4(textureLoad(lit_image, vec2<i32>(in.position.xy), 0).rgb, 1.0);
}

/// Where this pixel was on screen last frame, in UV space.
fn history_uv(uv: vec2<f32>, pixel: vec2<i32>) -> vec2<f32> {
    return uv - textureLoad(motion_vectors, pixel, 0).rg;
}

/// Shows one of SSGI's inputs instead of the image.
@fragment
fn debug_view(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let current = textureLoad(lit_image, pixel, 0).rgb;
    let previous_uv = history_uv(in.uv, pixel);
    let reprojected = textureSampleLevel(history, linear_sampler, previous_uv, 0.0).rgb;
    // Pixels that came into view this frame have nothing to reproject.
    let off_screen = any(previous_uv != saturate(previous_uv));

    var color = textureSampleLevel(history, linear_sampler, in.uv, 0.0).rgb;
    @if(DEBUG_REPROJECTED)
    color = select(reprojected, vec3(1.0, 0.0, 1.0), off_screen);
    // Error relative to brightness, so dark and bright areas read the same.
    @if(DEBUG_REPROJECTION_ERROR)
    color = select(
        4.0 * abs(reprojected - current) / (max(current, reprojected) + vec3(0.05)),
        vec3(1.0, 0.0, 1.0),
        off_screen,
    );
    return vec4(color, 1.0);
}
