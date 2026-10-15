//! Screen-space global illumination: temporal accumulation (denoising, step 1 of 2).
//!
//! Blends this frame's raw trace (upsampled with depth awareness when it ran at half
//! resolution) into a running average that follows the scene with motion vectors. The average
//! restarts wherever the reprojected history belongs to a different surface (its stored distance
//! doesn't match), and keeps at most `max_history_frames` frames so lighting changes still show.
//! History is also clamped to the range of this frame's nearby raw values, so light left behind
//! by moving objects fades within a few frames instead of trailing them.

import bevy_render::view::View;

// Keep in sync with `trace.wesl` and `SsgiSettingsUniform`.
struct SsgiSettings {
    intensity: f32,
    radius: f32,
    thickness: f32,
    slice_count: u32,
    samples_per_slice_side: u32,
    frame: u32,
    half_resolution: u32,
    denoise: u32,
    max_history_frames: u32,
}

@group(0) @binding(0) var depth_texture: texture_depth_2d;
@group(0) @binding(1) var motion_vectors: texture_2d<f32>;
@group(0) @binding(2) var raw: texture_2d<f32>;
@group(0) @binding(3) var raw_depth: texture_2d<f32>;
@group(0) @binding(4) var history: texture_2d<f32>;
@group(0) @binding(5) var history_meta: texture_2d<f32>;
@group(0) @binding(6) var accumulated: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var accumulated_meta: texture_storage_2d<rg32float, write>;
@group(0) @binding(8) var<uniform> view: View;
@group(0) @binding(9) var<uniform> settings: SsgiSettings;

/// Distance from the camera plane to the surface at a pixel, or 0 for sky.
fn linear_depth(pixel: vec2<i32>) -> f32 {
    let depth = textureLoad(depth_texture, pixel, 0);
    if (depth == 0.0) {
        return 0.0;
    }
    let t = view.view_from_clip * vec4(0.0, 0.0, depth, 1.0);
    return -t.z / t.w;
}

/// This frame's raw trace at a viewport pixel. At half resolution, blends the nearby traced
/// pixels, favoring ones at a similar distance so light doesn't leak across edges.
fn current_frame(local: vec2<i32>, depth: f32) -> vec4<f32> {
    if (settings.half_resolution == 0u) {
        return textureLoad(raw, local, 0);
    }
    var offsets = array(vec2(0, 0), vec2(1, 1), vec2(1, 0), vec2(0, 1));
    let offset = offsets[settings.frame % 4u];
    let max_texel = (vec2<i32>(view.viewport.zw) + 1) / 2 - 1;
    let base = (local - offset) >> vec2(1u);

    var sum = vec4(0.0);
    var weight_sum = 0.0;
    for (var y = 0; y <= 1; y += 1) {
        for (var x = 0; x <= 1; x += 1) {
            let texel = clamp(base + vec2(x, y), vec2(0), max_texel);
            let texel_depth = textureLoad(raw_depth, texel, 0).r;
            if (texel_depth == 0.0) {
                continue;
            }
            let distance = abs(vec2<f32>(2 * texel + offset - local));
            let bilinear = max(1.0 - distance.x * 0.5, 0.0) * max(1.0 - distance.y * 0.5, 0.0);
            let similar_depth = saturate(1.0 - abs(texel_depth - depth) / (0.05 * depth));
            let weight = (bilinear + 1e-3) * similar_depth;
            sum += weight * textureLoad(raw, texel, 0);
            weight_sum += weight;
        }
    }
    if (weight_sum < 1e-4) {
        return textureLoad(raw, clamp(local >> vec2(1u), vec2(0), max_texel), 0);
    }
    return sum / weight_sum;
}

/// Mean and standard deviation of the raw trace in the 3x3 block around a viewport pixel (in
/// trace texels, so twice as wide at half resolution).
fn neighborhood(local: vec2<i32>) -> array<vec4<f32>, 2> {
    var center = local;
    var max_texel = vec2<i32>(view.viewport.zw) - 1;
    if (settings.half_resolution != 0u) {
        center = local >> vec2(1u);
        max_texel = (vec2<i32>(view.viewport.zw) + 1) / 2 - 1;
    }
    var sum = vec4(0.0);
    var sum_squares = vec4(0.0);
    for (var y = -1; y <= 1; y += 1) {
        for (var x = -1; x <= 1; x += 1) {
            let value = textureLoad(raw, clamp(center + vec2(x, y), vec2(0), max_texel), 0);
            sum += value;
            sum_squares += value * value;
        }
    }
    let mean = sum / 9.0;
    let deviation = sqrt(max(sum_squares / 9.0 - mean * mean, vec4(0.0)));
    return array(mean, deviation);
}

@compute
@workgroup_size(8, 8, 1)
fn temporal(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let local = vec2<i32>(global_id.xy);
    if (any(vec2<f32>(local) >= view.viewport.zw)) {
        return;
    }
    let pixel = local + vec2<i32>(view.viewport.xy);

    let depth = linear_depth(pixel);
    if (depth == 0.0) {
        textureStore(accumulated, pixel, vec4(0.0, 0.0, 0.0, 1.0));
        textureStore(accumulated_meta, pixel, vec4(0.0));
        return;
    }
    let current = current_frame(local, depth);

    var history_value = vec4(0.0);
    var history_frames = 0.0;
    if (settings.denoise != 0u) {
        let uv = (vec2<f32>(local) + 0.5) / view.viewport.zw;
        let previous_uv = uv - textureLoad(motion_vectors, pixel, 0).rg;
        if (all(previous_uv == saturate(previous_uv))) {
            let previous_pixel = min(
                vec2<i32>(previous_uv * view.viewport.zw),
                vec2<i32>(view.viewport.zw) - 1,
            ) + vec2<i32>(view.viewport.xy);
            let previous_meta = textureLoad(history_meta, previous_pixel, 0).rg;
            // Same surface as last frame? Its distance can only have changed a little.
            if (previous_meta.y > 0.0 && abs(previous_meta.x - depth) < 0.03 * depth) {
                // The raw trace is noisy, so the allowed range is wide; it still catches light
                // that clearly no longer belongs here.
                let stats = neighborhood(local);
                let spread = 2.0 * stats[1] + vec4(0.02);
                history_value = clamp(
                    textureLoad(history, previous_pixel, 0),
                    stats[0] - spread,
                    stats[0] + spread,
                );
                history_frames = previous_meta.y;
            }
        }
    }

    let frames = min(history_frames + 1.0, f32(max(settings.max_history_frames, 1u)));
    textureStore(accumulated, pixel, mix(history_value, current, 1.0 / frames));
    textureStore(accumulated_meta, pixel, vec4(depth, frames, 0.0, 0.0));
}
