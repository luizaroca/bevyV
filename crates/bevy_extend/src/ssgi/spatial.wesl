//! Screen-space global illumination: edge-aware blur (denoising, step 2 of 2).
//!
//! Averages each pixel with neighbors on the same surface: similar distance and facing the same
//! way. Pixels with little history yet (just came into view) are blurred over a wider area.
//! The result is what the PBR shaders read.

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
@group(0) @binding(1) var normal_texture: texture_2d<f32>;
@group(0) @binding(2) var accumulated: texture_2d<f32>;
@group(0) @binding(3) var accumulated_meta: texture_2d<f32>;
@group(0) @binding(4) var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var<uniform> view: View;
@group(0) @binding(6) var<uniform> settings: SsgiSettings;

fn linear_depth(pixel: vec2<i32>) -> f32 {
    let depth = textureLoad(depth_texture, pixel, 0);
    if (depth == 0.0) {
        return 0.0;
    }
    let t = view.view_from_clip * vec4(0.0, 0.0, depth, 1.0);
    return -t.z / t.w;
}

fn load_normal(pixel: vec2<i32>) -> vec3<f32> {
    return normalize(textureLoad(normal_texture, pixel, 0).xyz * 2.0 - 1.0);
}

@compute
@workgroup_size(8, 8, 1)
fn spatial(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let local = vec2<i32>(global_id.xy);
    if (any(vec2<f32>(local) >= view.viewport.zw)) {
        return;
    }
    let viewport_min = vec2<i32>(view.viewport.xy);
    let viewport_max = viewport_min + vec2<i32>(view.viewport.zw) - 1;
    let pixel = local + viewport_min;

    let center = textureLoad(accumulated, pixel, 0);
    let depth = linear_depth(pixel);
    if (depth == 0.0 || settings.denoise == 0u) {
        textureStore(output, pixel, center);
        return;
    }
    let normal = load_normal(pixel);
    let frames = textureLoad(accumulated_meta, pixel, 0).g;
    let spacing = select(1, 2, frames < 4.0);

    var sum = center;
    var weight_sum = 1.0;
    for (var y = -2; y <= 2; y += 1) {
        for (var x = -2; x <= 2; x += 1) {
            if (x == 0 && y == 0) {
                continue;
            }
            let neighbor = clamp(pixel + vec2(x, y) * spacing, viewport_min, viewport_max);
            let neighbor_depth = linear_depth(neighbor);
            if (neighbor_depth == 0.0) {
                continue;
            }
            let depth_weight = exp(-abs(neighbor_depth - depth) / (0.02 * depth));
            let normal_weight = pow(saturate(dot(normal, load_normal(neighbor))), 16.0);
            let distance_weight = exp(-f32(x * x + y * y) / 4.5);
            let weight = depth_weight * normal_weight * distance_weight;
            sum += weight * textureLoad(accumulated, neighbor, 0);
            weight_sum += weight;
        }
    }
    textureStore(output, pixel, sum / weight_sum);
}
