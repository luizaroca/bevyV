//! Screen-space global illumination, the raw trace.
//!
//! Screen Space Indirect Lighting with Visibility Bitmask (Therrien, Levesque and Gilet 2023,
//! https://arxiv.org/abs/2301.11376), the same paper Bevy's SSAO (`bevy_pbr/src/ssao/ssao.wesl`)
//! is built on. Each pixel marches a few slices through the depth buffer. Every slice keeps a
//! 32-bit mask of which directions of the hemisphere are blocked. When a depth sample blocks
//! directions that were still open, the light that surface sent out last frame (the reprojected
//! history) is added for the share of the hemisphere it newly covers.
//!
//! Output: rgb is the diffuse bounce light arriving at the pixel (multiply by albedo to light a
//! surface), a is the fraction of the hemisphere left open (ambient occlusion).
//! Everything is in the same units as the lit image, which already includes exposure.
//!
//! At half resolution each thread traces one pixel of a 2x2 block, a different one every frame,
//! and also stores that pixel's distance so `temporal.wesl` can upsample without bleeding across
//! edges. The output is raw and noisy; `temporal.wesl` and `spatial.wesl` clean it up.

import bevy_render::{
    view::View,
    maths::{PI, HALF_PI, fast_acos},
};

// Keep in sync with `temporal.wesl`, `spatial.wesl` and `SsgiSettingsUniform`.
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
@group(0) @binding(2) var motion_vectors: texture_2d<f32>;
@group(0) @binding(3) var history: texture_2d<f32>;
@group(0) @binding(4) var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var output_depth: texture_storage_2d<r32float, write>;
@group(0) @binding(6) var<uniform> view: View;
@group(0) @binding(7) var<uniform> settings: SsgiSettings;

/// Sectors per slice in the visibility bitmask.
const SECTOR_COUNT: u32 = 32u;

/// Viewport UV (0..1 inside the camera's viewport) of a pixel's center.
fn pixel_to_uv(pixel: vec2<i32>) -> vec2<f32> {
    return (vec2<f32>(pixel) + 0.5 - view.viewport.xy) / view.viewport.zw;
}

fn uv_to_pixel(uv: vec2<f32>) -> vec2<i32> {
    return vec2<i32>(floor(uv * view.viewport.zw + view.viewport.xy));
}

fn reconstruct_view_space_position(depth: f32, uv: vec2<f32>) -> vec3<f32> {
    let clip_xy = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - 2.0 * uv.y);
    let t = view.view_from_clip * vec4<f32>(clip_xy, depth, 1.0);
    return t.xyz / t.w;
}

fn load_normal_view_space(pixel: vec2<i32>) -> vec3<f32> {
    let world_normal = textureLoad(normal_texture, pixel, 0).xyz * 2.0 - 1.0;
    let view_from_world = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    );
    return normalize(view_from_world * world_normal);
}

/// The light a pixel's surface sent toward the camera last frame. Black if it wasn't on screen.
fn load_previous_radiance(pixel: vec2<i32>, uv: vec2<f32>) -> vec3<f32> {
    let previous_uv = uv - textureLoad(motion_vectors, pixel, 0).rg;
    if (any(previous_uv != saturate(previous_uv))) {
        return vec3(0.0);
    }
    return textureLoad(history, uv_to_pixel(previous_uv), 0).rgb;
}

/// PCG hash (Jarzynski and Olano 2020): turns an integer into well-mixed random bits.
fn pcg(input: u32) -> u32 {
    let state = input * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

/// Two independent random numbers in 0..1 for this pixel and frame. Plain white noise: an
/// earlier pattern-based noise left visible diagonal and horizontal streaks.
fn random_pair(pixel: vec2<i32>) -> vec2<f32> {
    let seed = pcg(u32(pixel.x) + pcg(u32(pixel.y) + pcg(settings.frame)));
    let a = pcg(seed);
    let b = pcg(a);
    return vec2(f32(a >> 8u), f32(b >> 8u)) / 16777216.0;
}

@compute
@workgroup_size(8, 8, 1)
fn trace(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // Output texel, relative to the viewport (half size at half resolution).
    let out_texel = vec2<i32>(global_id.xy);
    let half_resolution = settings.half_resolution != 0u;
    let viewport_size = vec2<i32>(view.viewport.zw);
    var local_pixel = out_texel;
    if (half_resolution) {
        // Visit the four pixels of each 2x2 block in turn over four frames.
        var offsets = array(vec2(0, 0), vec2(1, 1), vec2(1, 0), vec2(0, 1));
        local_pixel = 2 * out_texel + offsets[settings.frame % 4u];
        if (any(2 * out_texel >= viewport_size)) {
            return;
        }
        local_pixel = min(local_pixel, viewport_size - 1);
    } else if (any(out_texel >= viewport_size)) {
        return;
    }
    let pixel = local_pixel + vec2<i32>(view.viewport.xy);

    let depth = textureLoad(depth_texture, pixel, 0);
    if (depth == 0.0) {
        // Nothing drawn here (sky): no bounce light and nothing blocked.
        textureStore(output, out_texel, vec4(0.0, 0.0, 0.0, 1.0));
        textureStore(output_depth, out_texel, vec4(0.0));
        return;
    }

    let uv = pixel_to_uv(pixel);
    let position = reconstruct_view_space_position(depth, uv);
    textureStore(output_depth, out_texel, vec4(-position.z, 0.0, 0.0, 0.0));
    let normal = load_normal_view_space(pixel);
    let view_vec = normalize(-position);

    let slice_count = f32(settings.slice_count);
    let samples_per_side = f32(settings.samples_per_slice_side);
    let noise = random_pair(pixel);
    let slice_noise = noise.x;
    let step_noise = noise.y;

    // How far `radius` meters reach on screen at this depth, in viewport UV.
    let radius_uv = 0.5 * settings.radius * vec2(view.clip_from_view[0][0], view.clip_from_view[1][1])
        / -position.z;

    var bounce_light = vec3(0.0);
    var occluded_sectors = 0u;
    for (var slice_index = 0.0; slice_index < slice_count; slice_index += 1.0) {
        let phi = (PI / slice_count) * (slice_index + slice_noise);
        let omega = vec2(cos(phi), sin(phi));

        // The slice plane and the surface normal projected onto it, as in GTAO.
        let direction = vec3(omega, 0.0);
        let orthographic_direction = direction - dot(direction, view_vec) * view_vec;
        let axis = cross(direction, view_vec);
        let projected_normal = normal - axis * dot(normal, axis);
        let projected_normal_length = max(length(projected_normal), 1e-4);
        let sign_normal = sign(dot(orthographic_direction, projected_normal));
        let cos_normal = saturate(dot(projected_normal, view_vec) / projected_normal_length);
        // Rotates horizon angles so the bitmask spans the hemisphere around the normal.
        let normal_angle = (HALF_PI - sign_normal * fast_acos(cos_normal)) / PI;

        var bitmask = 0u;
        let step_uv = vec2(omega.x, -omega.y) * radius_uv;
        for (var step = 0.0; step < samples_per_side; step += 1.0) {
            var s = (step + fract(step_noise + step * 0.6180339887)) / samples_per_side;
            s *= s; // More samples close to the pixel, where detail matters most.

            for (var side = -1.0; side <= 1.0; side += 2.0) {
                let sample_uv = uv - side * s * step_uv;
                if (any(sample_uv != saturate(sample_uv))) {
                    continue;
                }
                let sample_pixel = uv_to_pixel(sample_uv);
                let sample_depth = textureLoad(depth_texture, sample_pixel, 0);
                if (sample_depth == 0.0) {
                    continue;
                }
                let sample_position = reconstruct_view_space_position(sample_depth, pixel_to_uv(sample_pixel));
                let delta = sample_position - position;
                let delta_length = length(delta);
                if (delta_length < 1e-4) {
                    continue;
                }

                // Front and back of the sample, assuming surfaces are `thickness` thick.
                let delta_back = delta - view_vec * settings.thickness;
                var horizons = vec2(
                    fast_acos(dot(delta / delta_length, view_vec)),
                    fast_acos(dot(normalize(delta_back), view_vec)),
                );
                horizons = saturate(fma(vec2(side), -horizons / PI, vec2(normal_angle)));
                horizons = select(horizons.xy, horizons.yx, side >= 0.0);
                let start = u32(horizons.x * f32(SECTOR_COUNT));
                let count = u32(ceil((horizons.y - horizons.x) * f32(SECTOR_COUNT)));
                let sample_bits = insertBits(0u, 0xFFFFFFFFu, start, count);

                let new_bits = sample_bits & ~bitmask;
                bitmask |= sample_bits;
                if (new_bits == 0u) {
                    continue;
                }

                // Light from this sample: its share of the hemisphere, cosine weighted around
                // the normal (the pi/2 keeps a uniformly lit hemisphere at full strength), and
                // only if we see the side of the sample that faces us.
                let light_dir = delta / delta_length;
                let sample_normal = load_normal_view_space(sample_pixel);
                let faces_us = select(0.0, 1.0, dot(sample_normal, -light_dir) > 0.0);
                let share = f32(countOneBits(new_bits)) / f32(SECTOR_COUNT);
                let weight = share * saturate(dot(normal, light_dir)) * HALF_PI * faces_us;
                if (weight > 0.0) {
                    bounce_light += weight * load_previous_radiance(sample_pixel, sample_uv);
                }
            }
        }
        occluded_sectors += countOneBits(bitmask);
    }

    let visibility = 1.0 - f32(occluded_sectors) / (slice_count * f32(SECTOR_COUNT));
    textureStore(output, out_texel, vec4(settings.intensity * bounce_light / slice_count, visibility));
}
