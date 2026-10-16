//! Screen-space reflections, the raw trace.
//!
//! For every pixel, picks a reflection direction around the mirror direction (a GGX sample, so
//! rougher surfaces scatter more widely), marches it across the screen against the depth buffer
//! and, where it hits a surface, takes the light that surface sent toward the camera last frame
//! (the reprojected history). A ray that leaves without hitting anything but points at the sky
//! takes the sky from the history, which is exact for something infinitely far away.
//!
//! Glass and other see-through surfaces aren't in the depth buffer. They get a depth layer of
//! their own (see `glass.rs`): a ray that hits it takes its light from a history copy that has
//! glass in it, while rays that hit opaque surfaces use one without, so glass in front of a
//! surface doesn't show up at the surface's position. A ray that passes through glass sees what's
//! behind it untinted.
//!
//! Roughness comes from the normal prepass's alpha, which this fork's `StandardMaterial` prepass
//! fills in (other materials write 1.0, which is too rough to reflect).
//!
//! Output: rgb is the light arriving along the reflected ray, before the surface's own specular
//! color is applied. a is how much to trust it (0 = missed, fall back to the environment map),
//! faded near screen edges and toward the roughness limit. The output is one noisy sample per
//! pixel, which `temporal.wesl` and `spatial.wesl` then denoise.

import bevy_render::{
    view::View,
    maths::PI,
};

// Keep in sync with `temporal.wesl`, `spatial.wesl` and `SsrSettingsUniform`.
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
@group(0) @binding(3) var history: texture_2d<f32>;
@group(0) @binding(4) var linear_sampler: sampler;
@group(0) @binding(5) var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(6) var<uniform> view: View;
@group(0) @binding(7) var<uniform> settings: SsrSettings;
@group(0) @binding(8) var glass_depth_texture: texture_depth_2d;
@group(0) @binding(9) var glass_history: texture_2d<f32>;

/// Bevy's lighting never goes below this perceptual roughness either.
const MIN_PERCEPTUAL_ROUGHNESS: f32 = 0.089;

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

/// Viewport UV of a view-space point (`w` = 1) or direction (`w` = 0, its vanishing point), and
/// the clip-space w, which is the distance in front of the camera.
fn project(position: vec4<f32>) -> vec3<f32> {
    let clip = view.clip_from_view * position;
    let ndc = clip.xy / clip.w;
    return vec3(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, clip.w);
}

/// Distance in front of the camera of a depth buffer value, or 0 for nothing there.
fn depth_to_linear(depth: f32) -> f32 {
    if (depth == 0.0) {
        return 0.0;
    }
    let t = view.view_from_clip * vec4(0.0, 0.0, depth, 1.0);
    return -t.z / t.w;
}

/// Distance in front of the camera of the opaque surface at a pixel, or 0 for sky.
fn linear_depth(pixel: vec2<i32>) -> f32 {
    return depth_to_linear(textureLoad(depth_texture, pixel, 0));
}

/// Distance in front of the camera of the glass at a pixel, or 0 where there is none.
fn glass_linear_depth(pixel: vec2<i32>) -> f32 {
    if (settings.glass == 0u) {
        return 0.0;
    }
    return depth_to_linear(textureLoad(glass_depth_texture, pixel, 0));
}

fn view_from_world_3x3() -> mat3x3<f32> {
    return mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    );
}

/// PCG hash (Jarzynski and Olano 2020): turns an integer into well-mixed random bits.
fn pcg(input: u32) -> u32 {
    let state = input * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

/// Three independent random numbers in 0..1 for this pixel and frame.
fn random3(pixel: vec2<i32>) -> vec3<f32> {
    let seed = pcg(u32(pixel.x) + pcg(u32(pixel.y) + pcg(settings.frame)));
    let a = pcg(seed);
    let b = pcg(a);
    let c = pcg(b);
    return vec3(f32(a >> 8u), f32(b >> 8u), f32(c >> 8u)) / 16777216.0;
}

/// Samples a microfacet normal from the GGX distribution of visible normals (Heitz 2018,
/// "Sampling the GGX Distribution of Visible Normals"), in tangent space where the surface normal
/// is +z. `alpha` is the GGX roughness (perceptual roughness squared).
fn sample_ggx_vndf(view_tangent: vec3<f32>, alpha: f32, u: vec2<f32>) -> vec3<f32> {
    let stretched = normalize(vec3(alpha * view_tangent.xy, view_tangent.z));
    let length_squared = dot(stretched.xy, stretched.xy);
    var t1 = vec3(1.0, 0.0, 0.0);
    if (length_squared > 0.0) {
        t1 = vec3(-stretched.y, stretched.x, 0.0) * inverseSqrt(length_squared);
    }
    let t2 = cross(stretched, t1);
    let r = sqrt(u.x);
    let phi = 2.0 * PI * u.y;
    let p1 = r * cos(phi);
    let s = 0.5 * (1.0 + stretched.z);
    let p2 = (1.0 - s) * sqrt(1.0 - p1 * p1) + s * r * sin(phi);
    let half_stretched = p1 * t1 + p2 * t2 + sqrt(max(0.0, 1.0 - p1 * p1 - p2 * p2)) * stretched;
    return normalize(vec3(alpha * half_stretched.xy, max(0.0, half_stretched.z)));
}

/// Builds two axes perpendicular to `n` (Duff et al. 2017).
fn orthonormal_basis(n: vec3<f32>) -> mat3x3<f32> {
    let sign = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    return mat3x3(
        vec3(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x),
        vec3(b, sign + n.y * n.y * a, -n.y),
        n,
    );
}

/// 1 inside the viewport, fading to 0 within `edge_fade` (a fraction of the viewport) of its
/// border, so reflections don't stop with a hard line where the screen ends.
fn edge_fade(uv: vec2<f32>) -> f32 {
    if (any(uv != saturate(uv))) {
        return 0.0;
    }
    if (settings.edge_fade <= 0.0) {
        return 1.0;
    }
    let distance_to_edge = min(uv, 1.0 - uv);
    let fade = saturate(distance_to_edge / settings.edge_fade);
    return fade.x * fade.y;
}

/// Last frame's light at a point on screen this frame, following its motion vector, from the
/// history with glass in it or the one without. The alpha is 0 if it was off screen last frame.
fn previous_radiance(uv: vec2<f32>, with_glass: bool) -> vec4<f32> {
    let pixel = uv_to_pixel(uv);
    let previous_uv = uv - textureLoad(motion_vectors, pixel, 0).rg;
    if (any(previous_uv != saturate(previous_uv))) {
        return vec4(0.0);
    }
    var radiance: vec3<f32>;
    if (with_glass) {
        radiance = textureSampleLevel(glass_history, linear_sampler, previous_uv, 0.0).rgb;
    } else {
        radiance = textureSampleLevel(history, linear_sampler, previous_uv, 0.0).rgb;
    }
    return vec4(radiance, 1.0);
}

@compute
@workgroup_size(8, 8, 1)
fn trace(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let local_pixel = vec2<i32>(global_id.xy);
    let viewport_size = vec2<i32>(view.viewport.zw);
    if (any(local_pixel >= viewport_size)) {
        return;
    }
    let pixel = local_pixel + vec2<i32>(view.viewport.xy);

    let depth = textureLoad(depth_texture, pixel, 0);
    let normal_and_roughness = textureLoad(normal_texture, pixel, 0);
    let perceptual_roughness = max(normal_and_roughness.a, MIN_PERCEPTUAL_ROUGHNESS);
    if (depth == 0.0 || perceptual_roughness >= settings.max_roughness) {
        // Sky, or too rough: lighting keeps using the environment map.
        textureStore(output, local_pixel, vec4(0.0));
        return;
    }
    // Fade out over the last fifth of the roughness range instead of stopping at a hard line.
    let roughness_fade = 1.0 - smoothstep(0.8 * settings.max_roughness, settings.max_roughness, perceptual_roughness);

    let uv = pixel_to_uv(pixel);
    let position = reconstruct_view_space_position(depth, uv);
    let normal = normalize(view_from_world_3x3() * (normal_and_roughness.xyz * 2.0 - 1.0));
    let view_vec = normalize(-position);
    let noise = random3(pixel);

    // Pick a reflection direction: mirror the view direction around a GGX microfacet normal.
    let tangent_to_view = orthonormal_basis(normal);
    let view_tangent = transpose(tangent_to_view) * view_vec;
    let alpha = perceptual_roughness * perceptual_roughness;
    let microfacet = tangent_to_view * sample_ggx_vndf(view_tangent, alpha, noise.xy);
    let direction = reflect(-view_vec, microfacet);
    if (dot(direction, normal) <= 0.0) {
        // Reflected into the surface; rare except at grazing angles.
        textureStore(output, local_pixel, vec4(0.0));
        return;
    }

    // Shorten the ray so it stays in front of the camera's near plane, where it can be projected.
    let near = view.clip_from_view[3][2];
    var ray_length = settings.max_distance;
    if (direction.z > 0.0) {
        ray_length = min(ray_length, 0.99 * (-position.z - near) / direction.z);
    }
    let start = project(vec4(position, 1.0));
    let end = project(vec4(position + direction * ray_length, 1.0));

    // March in screen space. Distance is interpolated as 1/w, which is linear on screen.
    let start_inverse_w = 1.0 / start.z;
    let end_inverse_w = 1.0 / end.z;
    let steps = f32(max(settings.steps, 1u));
    // Skip at least a pixel and a half so the ray doesn't hit the surface it starts on.
    let screen_length = length((end.xy - start.xy) * view.viewport.zw);
    let min_t = min(1.5 / max(screen_length, 1e-4), 1.0);

    var previous_t = min_t;
    var hit_t = -1.0;
    // Whether the hit is on the glass layer rather than an opaque surface.
    var hit_glass = false;
    // When the ray passes behind something (further than its assumed thickness), the depth of
    // that occluder; negative while the ray is in the open.
    var occluder_depth = -1.0;
    // Where an occluded ray reached a surface further back, taken as the hit when nothing better
    // turns up.
    var hidden_hit_t = -1.0;
    for (var i = 0.0; i < steps; i += 1.0) {
        let t = min_t + (1.0 - min_t) * (i + noise.z) / steps;
        let sample_uv = mix(start.xy, end.xy, t);
        if (any(sample_uv != saturate(sample_uv))) {
            break;
        }
        let ray_depth = 1.0 / mix(start_inverse_w, end_inverse_w, t);
        let sample_pixel = uv_to_pixel(sample_uv);
        // Glass is in front of the opaque surface at the same pixel, so it's checked first.
        let glass_depth = glass_linear_depth(sample_pixel);
        if (glass_depth > 0.0 && ray_depth > glass_depth && ray_depth - glass_depth < settings.thickness) {
            hit_t = t;
            hit_glass = true;
            break;
        }
        let scene_depth = linear_depth(sample_pixel);
        if (scene_depth > 0.0 && ray_depth > scene_depth) {
            // Behind the depth buffer's surface, but by less than its assumed thickness.
            if (ray_depth - scene_depth < settings.thickness) {
                hit_t = t;
                break;
            }
            // Well behind it: the ray went behind an object, and what it hits there is hidden.
            // Once it comes out behind a surface further back (usually whatever stands behind
            // the object), that surface is the best guess. Without this, these rays would fall
            // back to the environment map and outline objects with a ghost of the sky.
            if (occluder_depth < 0.0) {
                occluder_depth = scene_depth;
            } else if (hidden_hit_t < 0.0 && scene_depth > occluder_depth + settings.thickness) {
                hidden_hit_t = t;
            }
        } else if (scene_depth > 0.0) {
            // Back in front of the depth buffer, in the open again.
            occluder_depth = -1.0;
            hidden_hit_t = -1.0;
        }
        previous_t = t;
    }

    var result = vec4(0.0);
    if (hit_t < 0.0 && hidden_hit_t >= 0.0) {
        let hit_uv = mix(start.xy, end.xy, hidden_hit_t);
        let hit_normal = view_from_world_3x3() * (textureLoad(normal_texture, uv_to_pixel(hit_uv), 0).xyz * 2.0 - 1.0);
        if (dot(hit_normal, direction) < 0.0) {
            let radiance = previous_radiance(hit_uv, false);
            result = vec4(radiance.rgb, radiance.a * edge_fade(hit_uv));
        }
    } else if (hit_t >= 0.0) {
        // Narrow down where the ray crosses the surface with a binary search.
        var low = previous_t;
        var high = hit_t;
        for (var i = 0u; i < settings.refine_steps; i += 1u) {
            let middle = 0.5 * (low + high);
            let sample_uv = mix(start.xy, end.xy, middle);
            let ray_depth = 1.0 / mix(start_inverse_w, end_inverse_w, middle);
            var scene_depth: f32;
            if (hit_glass) {
                scene_depth = glass_linear_depth(uv_to_pixel(sample_uv));
            } else {
                scene_depth = linear_depth(uv_to_pixel(sample_uv));
            }
            if (scene_depth > 0.0 && ray_depth > scene_depth) {
                high = middle;
            } else {
                low = middle;
            }
        }
        let hit_uv = mix(start.xy, end.xy, high);
        let hit_pixel = uv_to_pixel(hit_uv);

        // Only the front of a surface sends light our way; hitting a back face means the ray went
        // behind something the depth buffer can't see past. The glass layer has no normals (the
        // normal there is the opaque surface's behind it), and glass is seen from both sides.
        let hit_normal = view_from_world_3x3() * (textureLoad(normal_texture, hit_pixel, 0).xyz * 2.0 - 1.0);
        if (hit_glass || dot(hit_normal, direction) < 0.0) {
            let radiance = previous_radiance(hit_uv, hit_glass);
            result = vec4(radiance.rgb, radiance.a * edge_fade(hit_uv));
        }
    } else if (direction.z < 0.0) {
        // No hit. If the ray heads into the sky, its vanishing point on screen shows the sky it
        // would see (the sky is infinitely far away, so where the ray starts doesn't matter).
        let sky_uv = project(vec4(direction, 0.0)).xy;
        if (all(sky_uv == saturate(sky_uv)) && linear_depth(uv_to_pixel(sky_uv)) == 0.0) {
            let radiance = previous_radiance(sky_uv, false);
            result = vec4(radiance.rgb, radiance.a * edge_fade(sky_uv));
        }
    }

    result.a *= roughness_fade;
    textureStore(output, local_pixel, result);
}
