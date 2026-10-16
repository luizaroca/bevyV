//! Screen-space reflections: temporal accumulation (denoising, step 1 of 2).
//!
//! Blends this frame's raw trace into a running average that follows each surface with its
//! motion vectors. Values are averaged premultiplied by their confidence, so a ray that missed
//! (confidence 0) only lowers the confidence, not the color.
//!
//! The average restarts wherever the reprojected history belongs to a different surface (its
//! stored distance doesn't match). Reflections move differently from the surface they're on, so
//! history is clamped to the range of this frame's nearby raw values, and smooth surfaces keep
//! only a few frames: their rays barely scatter, so they need little averaging and would
//! otherwise smear when the camera moves. Rough surfaces keep up to `max_history_frames`.

import bevy_render::view::View;

// Keep in sync with `trace.wesl` and `SsrSettingsUniform`.
struct SsrSettings {
    max_roughness: f32,
    thickness: f32,
    max_distance: f32,
    steps: u32,
    refine_steps: u32,
    edge_fade: f32,
    frame: u32,
    denoise: u32,
    max_history_frames: u32,
    glass: u32,
}

@group(0) @binding(0) var depth_texture: texture_depth_2d;
@group(0) @binding(1) var normal_texture: texture_2d<f32>;
@group(0) @binding(2) var motion_vectors: texture_2d<f32>;
@group(0) @binding(3) var raw: texture_2d<f32>;
@group(0) @binding(4) var history: texture_2d<f32>;
@group(0) @binding(5) var history_meta: texture_2d<f32>;
@group(0) @binding(6) var accumulated: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var accumulated_meta: texture_storage_2d<rg32float, write>;
@group(0) @binding(8) var<uniform> view: View;
@group(0) @binding(9) var<uniform> settings: SsrSettings;

/// Bevy's lighting never goes below this perceptual roughness either.
const MIN_PERCEPTUAL_ROUGHNESS: f32 = 0.089;

/// Distance in front of the camera of the surface at a pixel, or 0 for sky.
fn linear_depth(pixel: vec2<i32>) -> f32 {
    let depth = textureLoad(depth_texture, pixel, 0);
    if (depth == 0.0) {
        return 0.0;
    }
    let t = view.view_from_clip * vec4(0.0, 0.0, depth, 1.0);
    return -t.z / t.w;
}

/// The raw trace at a viewport pixel, premultiplied by its confidence.
fn load_raw(local: vec2<i32>) -> vec4<f32> {
    let value = textureLoad(raw, local, 0);
    return vec4(value.rgb * value.a, value.a);
}

/// Mean and standard deviation of the raw trace in the 3x3 block around a viewport pixel.
fn neighborhood(local: vec2<i32>) -> array<vec4<f32>, 2> {
    let max_local = vec2<i32>(view.viewport.zw) - 1;
    var sum = vec4(0.0);
    var sum_squares = vec4(0.0);
    for (var y = -1; y <= 1; y += 1) {
        for (var x = -1; x <= 1; x += 1) {
            let value = load_raw(clamp(local + vec2(x, y), vec2(0), max_local));
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
    let roughness = max(textureLoad(normal_texture, pixel, 0).a, MIN_PERCEPTUAL_ROUGHNESS);
    if (depth == 0.0 || roughness >= settings.max_roughness) {
        textureStore(accumulated, pixel, vec4(0.0));
        textureStore(accumulated_meta, pixel, vec4(depth, 0.0, 0.0, 0.0));
        return;
    }
    let current = load_raw(local);

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

    // A near-mirror keeps about 5 frames; from roughness 0.3 up, the full history.
    let max_frames = mix(
        1.0,
        f32(max(settings.max_history_frames, 1u)),
        saturate(roughness / 0.3),
    );
    let frames = min(history_frames + 1.0, max(max_frames, 1.0));
    textureStore(accumulated, pixel, mix(history_value, current, 1.0 / frames));
    textureStore(accumulated_meta, pixel, vec4(depth, frames, 0.0, 0.0));
}
