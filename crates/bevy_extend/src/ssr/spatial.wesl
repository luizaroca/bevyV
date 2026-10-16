//! Screen-space reflections: edge-aware blur (denoising, step 2 of 2).
//!
//! Averages each pixel with neighbors on the same surface (close to its plane, facing the same
//! way, similar roughness). Distance to the plane rather than to the camera, so floors seen at a
//! grazing angle, whose distance changes quickly from pixel to pixel, still get blurred. Rougher surfaces blur over a wider area, and near-mirrors aren't
//! blurred at all, so sharp reflections stay sharp. Pixels with little history yet (just came
//! into view) are blurred wider. Bright samples count for less (weighted by 1 / (1 + brightness),
//! Karis 2014), so a lone ray that found the sun doesn't spread into a blotch. Writes what the
//! PBR shaders read: the light in rgb, no longer premultiplied, and the confidence in a.

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
@group(0) @binding(2) var accumulated: texture_2d<f32>;
@group(0) @binding(3) var accumulated_meta: texture_2d<f32>;
@group(0) @binding(4) var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var<uniform> view: View;
@group(0) @binding(6) var<uniform> settings: SsrSettings;

/// Bevy's lighting never goes below this perceptual roughness either.
const MIN_PERCEPTUAL_ROUGHNESS: f32 = 0.089;

fn linear_depth(pixel: vec2<i32>) -> f32 {
    let depth = textureLoad(depth_texture, pixel, 0);
    if (depth == 0.0) {
        return 0.0;
    }
    let t = view.view_from_clip * vec4(0.0, 0.0, depth, 1.0);
    return -t.z / t.w;
}

/// World-space position of the surface at a pixel.
fn world_position(pixel: vec2<i32>) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + 0.5 - view.viewport.xy) / view.viewport.zw;
    let clip = vec4(uv.x * 2.0 - 1.0, 1.0 - 2.0 * uv.y, textureLoad(depth_texture, pixel, 0), 1.0);
    let world = view.world_from_clip * clip;
    return world.xyz / world.w;
}

/// How much a sample counts, by its brightness (premultiplied rgb is fine: confidence is at most
/// 1, so it only makes uncertain samples count a little more).
fn brightness_weight(value: vec4<f32>) -> f32 {
    return 1.0 / (1.0 + dot(value.rgb, vec3(0.2126, 0.7152, 0.0722)));
}

/// Undoes the premultiplication by confidence.
fn unpremultiply(value: vec4<f32>) -> vec4<f32> {
    if (value.a <= 1e-4) {
        return vec4(0.0);
    }
    return vec4(value.rgb / value.a, value.a);
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
    let normal_and_roughness = textureLoad(normal_texture, pixel, 0);
    let roughness = max(normal_and_roughness.a, MIN_PERCEPTUAL_ROUGHNESS);
    // Blur radius in pixels (as a Gaussian's sigma): none for near-mirrors, wider when rougher.
    var sigma = saturate((roughness - 0.1) / 0.25) * 2.5;
    let frames = textureLoad(accumulated_meta, pixel, 0).g;
    if (frames < 4.0) {
        sigma *= 2.0;
    }
    if (depth == 0.0 || settings.denoise == 0u || sigma < 0.25) {
        textureStore(output, pixel, unpremultiply(center));
        return;
    }
    let normal = normalize(normal_and_roughness.xyz * 2.0 - 1.0);
    let position = world_position(pixel);
    // Taps 2 sigma out at most, five per side.
    let spacing = max(i32(round(sigma * 0.8)), 1);

    let center_weight = brightness_weight(center);
    var sum = center * center_weight;
    var weight_sum = center_weight;
    for (var y = -2; y <= 2; y += 1) {
        for (var x = -2; x <= 2; x += 1) {
            if (x == 0 && y == 0) {
                continue;
            }
            let offset = vec2(x, y) * spacing;
            let neighbor = clamp(pixel + offset, viewport_min, viewport_max);
            if (textureLoad(depth_texture, neighbor, 0) == 0.0) {
                continue;
            }
            let neighbor_normal_and_roughness = textureLoad(normal_texture, neighbor, 0);
            let neighbor_normal = normalize(neighbor_normal_and_roughness.xyz * 2.0 - 1.0);
            let plane_distance = abs(dot(normal, world_position(neighbor) - position));
            let depth_weight = exp(-plane_distance / (0.01 * depth));
            let normal_weight = pow(saturate(dot(normal, neighbor_normal)), 32.0);
            let roughness_weight = exp(-abs(neighbor_normal_and_roughness.a - normal_and_roughness.a) / 0.05);
            let distance_weight = exp(-f32(dot(offset, offset)) / (2.0 * sigma * sigma));
            let value = textureLoad(accumulated, neighbor, 0);
            let weight = depth_weight * normal_weight * roughness_weight * distance_weight
                * brightness_weight(value);
            sum += weight * value;
            weight_sum += weight;
        }
    }
    textureStore(output, pixel, unpremultiply(sum / weight_sum));
}
