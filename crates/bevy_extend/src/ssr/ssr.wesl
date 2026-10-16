//! Screen-space reflections: storing the radiance history, and the debug views.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

@group(0) @binding(0) var lit_image: texture_2d<f32>;
@group(0) @binding(1) var reflections: texture_2d<f32>;
@group(0) @binding(2) var normal_texture: texture_2d<f32>;

/// Copies this frame's lit image into the history texture.
@fragment
fn copy_history(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4(textureLoad(lit_image, vec2<i32>(in.position.xy), 0).rgb, 1.0);
}

/// Shows one of SSR's buffers instead of the image.
@fragment
fn debug_view(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let reflection = textureLoad(reflections, pixel, 0);
    var color = vec3(0.0);
    @if(DEBUG_REFLECTIONS)
    color = reflection.rgb * reflection.a;
    @if(DEBUG_CONFIDENCE)
    color = vec3(reflection.a);
    @if(DEBUG_ROUGHNESS)
    color = vec3(textureLoad(normal_texture, pixel, 0).a);
    return vec4(color, 1.0);
}
