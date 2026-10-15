//! Screen-space global illumination (SSGI).
//!
//! Work in progress, built in phases (see `notes/ssgi-plan.md` in the project files).
//!
//! Each frame:
//! 1. After the prepass, `trace.wesl` gathers bounce light and ambient occlusion for every pixel
//!    (or one pixel in four, at half resolution) from last frame's lit image (the history), using
//!    a visibility bitmask slice march. The result is noisy.
//! 2. `temporal.wesl` averages it over frames, following motion vectors, and `spatial.wesl`
//!    blurs it along surfaces without crossing edges.
//! 3. The main pass (forward or deferred) adds that bounce light to each surface's indirect
//!    light and uses the occlusion like SSAO, through the `ScreenSpaceIndirectLight` hook this fork
//!    adds to `bevy_pbr`.
//! 4. After the main pass, the lit image is copied into the history for the next frame, so light
//!    keeps bouncing over frames, and a debug view can replace the image.

use bevy_app::{App, Plugin};
use bevy_asset::{embedded_asset, load_embedded_asset, AssetServer, Handle};
use bevy_camera::{Camera3d, TonemappingPass};
use bevy_core_pipeline::{
    prepass::{DepthPrepass, MotionVectorPrepass, NormalPrepass, ViewPrepassTextures},
    schedule::{Core3d, Core3dSystems},
    FullscreenShader,
};
use bevy_diagnostic::FrameCount;
use bevy_ecs::{
    prelude::{Component, Entity, ReflectComponent},
    query::{QueryItem, With},
    resource::Resource,
    schedule::IntoScheduleConfigs,
    system::{lifetimeless::Read, Commands, Query, Res, ResMut},
};
use bevy_image::ToExtents;
use bevy_math::UVec2;
use bevy_pbr::{ScreenSpaceIndirectLight, ScreenSpaceIndirectLightTexture};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::{
    camera::ExtractedCamera,
    diagnostic::RecordDiagnostics,
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    render_resource::{
        binding_types::{sampler, texture_2d},
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
        CachedRenderPipelineId, ColorTargetState, ColorWrites, FilterMode, FragmentState,
        Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
        RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
        SpecializedRenderPipeline, SpecializedRenderPipelines, TextureDescriptor, TextureDimension,
        TextureFormat, TextureSampleType, TextureUsages, TextureView,
    },
    render_resource::{
        binding_types::{texture_depth_2d, texture_storage_2d, uniform_buffer},
        CachedComputePipelineId, ComputePassDescriptor, ComputePipelineDescriptor, ShaderType,
        StorageTextureAccess, UniformBuffer,
    },
    renderer::RenderQueue,
    renderer::{RenderContext, RenderDevice, ViewQuery},
    sync_component::SyncComponent,
    texture::{CachedTexture, TextureCache},
    view::{ExtractedView, Msaa, ViewTarget},
    view::{ViewUniform, ViewUniformOffset, ViewUniforms},
    GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy_shader::Shader;
use bevy_utils::{default, once};
use tracing::error;

/// Format of the radiance history. Lit HDR color needs more range than 8 bits.
const HISTORY_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Format of the trace, accumulation and final textures: bounce light in rgb, ambient
/// occlusion in a.
const GI_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Format of the traced pixels' distances (half resolution upsampling).
const RAW_DEPTH_FORMAT: TextureFormat = TextureFormat::R32Float;

/// Format of the accumulation's bookkeeping: distance and number of frames averaged.
const META_FORMAT: TextureFormat = TextureFormat::Rg32Float;

/// Adds screen-space global illumination. Part of [`ExtendPlugins`](crate::ExtendPlugins).
#[derive(Default)]
pub struct ScreenSpaceGlobalIlluminationPlugin;

/// Add this to a 3D camera to enable screen-space global illumination (SSGI).
///
/// SSGI gathers diffuse bounce light from what is on screen, using the previous frame's lit
/// image. It needs the depth, normal and motion vector prepasses (added automatically), an
/// untonemapped main texture ([`TonemappingPass`], also added) and `Msaa::Off`.
///
/// SSGI denoises its own output, so it works without TAA, but TAA still smooths what is left.
#[derive(Component, Reflect, Clone, PartialEq, Debug)]
#[reflect(Component, Default, Clone, PartialEq)]
#[require(DepthPrepass, NormalPrepass, MotionVectorPrepass, TonemappingPass)]
pub struct ScreenSpaceGlobalIllumination {
    /// Multiplies the bounce light. 1.0 is physically based; 0.0 leaves only the occlusion.
    pub intensity: f32,
    /// How far away surfaces can send bounce light to a pixel, in meters (world units).
    ///
    /// Larger values reach further but get noisier and miss thin details.
    pub radius: f32,
    /// How thick surfaces are assumed to be, in meters. The depth buffer only shows the front of
    /// things, so light can pass behind anything further back than this.
    pub thickness: f32,
    /// Directions searched around each pixel per frame. More is smoother and slower.
    pub slice_count: u32,
    /// Depth samples taken along each side of a direction. More finds thinner objects.
    pub samples_per_slice_side: u32,
    /// Trace one pixel in four each frame (a different one every frame) and fill in the rest from
    /// neighbors at a similar distance. About four times cheaper, slightly softer.
    pub half_resolution: bool,
    /// Average over frames and blur along surfaces. Turn off to see the raw trace.
    pub denoise: bool,
    /// The most frames averaged together. More is smoother; fewer reacts faster when lighting
    /// changes.
    pub max_history_frames: u32,
    /// Replaces the image with one of SSGI's buffers, for checking them by eye.
    pub debug_view: SsgiDebugView,
}

impl Default for ScreenSpaceGlobalIllumination {
    fn default() -> Self {
        Self {
            intensity: 1.0,
            radius: 2.0,
            thickness: 0.5,
            slice_count: 4,
            samples_per_slice_side: 8,
            half_resolution: false,
            denoise: true,
            max_history_frames: 16,
            debug_view: SsgiDebugView::Off,
        }
    }
}

/// What [`ScreenSpaceGlobalIllumination::debug_view`] shows.
#[derive(Reflect, Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
#[reflect(Default, Clone, PartialEq, Hash)]
pub enum SsgiDebugView {
    /// The normal image.
    #[default]
    Off,
    /// The bounce light reaching each pixel, before it is multiplied by the surface color. Shows
    /// the denoised result, or the raw trace when `denoise` is off.
    BounceLight,
    /// The ambient occlusion from the same trace: white is open, black is enclosed.
    AmbientOcclusion,
    /// The previous frame's lit image, as stored for SSGI.
    History,
    /// The previous frame moved onto this frame with motion vectors. Should match the normal
    /// image, apart from areas that just came into view (magenta).
    Reprojected,
    /// How far the reprojected history is from this frame, amplified. Should be close to black
    /// while moving, apart from edges and newly visible areas.
    ReprojectionError,
}

impl SsgiDebugView {
    /// Every view, in the order a "next view" key should cycle through them.
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::BounceLight,
        Self::AmbientOcclusion,
        Self::History,
        Self::Reprojected,
        Self::ReprojectionError,
    ];

    /// The next view in [`Self::ALL`], wrapping around.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|view| *view == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    fn shader_def(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::BounceLight => Some("DEBUG_BOUNCE_LIGHT"),
            Self::AmbientOcclusion => Some("DEBUG_AMBIENT_OCCLUSION"),
            Self::History => Some("DEBUG_HISTORY"),
            Self::Reprojected => Some("DEBUG_REPROJECTED"),
            Self::ReprojectionError => Some("DEBUG_REPROJECTION_ERROR"),
        }
    }
}

impl Plugin for ScreenSpaceGlobalIlluminationPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "ssgi.wesl");
        embedded_asset!(app, "trace.wesl");
        embedded_asset!(app, "temporal.wesl");
        embedded_asset!(app, "spatial.wesl");

        app.add_plugins(ExtractComponentPlugin::<ScreenSpaceGlobalIllumination>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<SpecializedRenderPipelines<SsgiPipeline>>()
            .add_systems(RenderStartup, init_ssgi_pipeline)
            .add_systems(
                Render,
                (
                    prepare_ssgi_pipelines.in_set(RenderSystems::Prepare),
                    (prepare_ssgi_textures, prepare_ssgi_settings)
                        .in_set(RenderSystems::PrepareResources),
                ),
            )
            .add_systems(
                Core3d,
                (
                    // Needs this frame's depth, normals and motion vectors, and must finish
                    // before the main pass so lighting can use it (from phase 3).
                    ssgi_trace
                        .after(Core3dSystems::Prepass)
                        .before(Core3dSystems::MainPass),
                    // After lighting and transparents, before TAA and other post-processing.
                    ssgi_store_history_and_debug
                        .after(Core3dSystems::MainPass)
                        .before(Core3dSystems::EarlyPostProcess),
                ),
            );
    }
}

impl SyncComponent<RenderApp> for ScreenSpaceGlobalIllumination {
    type Target = (
        Self,
        ScreenSpaceIndirectLight,
        ScreenSpaceIndirectLightTexture,
        SsgiTextures,
        SsgiPipelineIds,
        SsgiSettingsBuffer,
    );
}

impl ExtractComponent<RenderApp> for ScreenSpaceGlobalIllumination {
    type QueryData = (Read<Self>, Read<Msaa>);
    type QueryFilter = With<Camera3d>;
    /// The marker tells `bevy_pbr` to build lighting pipelines that read SSGI's output.
    type Out = (Self, ScreenSpaceIndirectLight);

    fn extract_component((ssgi, msaa): QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        if *msaa != Msaa::Off {
            once!(error!(
                "Screen-space global illumination needs Msaa::Off, but the camera uses Msaa::{:?}",
                *msaa
            ));
            return None;
        }
        Some((ssgi.clone(), ScreenSpaceIndirectLight))
    }
}

/// A view's SSGI textures. The radiance history is a pair: last frame's image is read while this
/// frame's is written, and they swap every frame.
#[derive(Component)]
pub struct SsgiTextures {
    history_write: CachedTexture,
    history_read: CachedTexture,
    /// Raw trace output, viewport-relative, half size at half resolution.
    raw: CachedTexture,
    /// Distance of each traced pixel, same layout as `raw`.
    raw_depth: CachedTexture,
    /// Running average and its bookkeeping, swapped every frame like the history.
    accumulated_write: CachedTexture,
    accumulated_read: CachedTexture,
    meta_write: CachedTexture,
    meta_read: CachedTexture,
    /// Final denoised output that lighting reads.
    gi: CachedTexture,
}

fn prepare_ssgi_textures(
    mut commands: Commands,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    frame_count: Res<FrameCount>,
    cameras: Query<(Entity, &ExtractedCamera, &ScreenSpaceGlobalIllumination)>,
) {
    for (entity, camera, ssgi) in &cameras {
        let Some(size) = camera.physical_target_size else {
            continue;
        };
        let mut descriptor = TextureDescriptor {
            label: None,
            size: size.to_extents(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: HISTORY_FORMAT,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        };
        descriptor.label = Some("ssgi_history_1_texture");
        let history_1 = texture_cache.get(&render_device, descriptor.clone());
        descriptor.label = Some("ssgi_history_2_texture");
        let history_2 = texture_cache.get(&render_device, descriptor.clone());

        let mut storage = |label: &'static str, format: TextureFormat, size: UVec2| {
            texture_cache.get(
                &render_device,
                TextureDescriptor {
                    label: Some(label),
                    size: size.to_extents(),
                    format,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                    ..descriptor.clone()
                },
            )
        };
        let raw_size = if ssgi.half_resolution {
            (size + UVec2::ONE) / 2
        } else {
            size
        };
        let raw = storage("ssgi_raw_texture", GI_FORMAT, raw_size);
        let raw_depth = storage("ssgi_raw_depth_texture", RAW_DEPTH_FORMAT, raw_size);
        let accumulated_1 = storage("ssgi_accumulated_1_texture", GI_FORMAT, size);
        let accumulated_2 = storage("ssgi_accumulated_2_texture", GI_FORMAT, size);
        let meta_1 = storage("ssgi_meta_1_texture", META_FORMAT, size);
        let meta_2 = storage("ssgi_meta_2_texture", META_FORMAT, size);
        let gi = storage("ssgi_gi_texture", GI_FORMAT, size);

        let even = frame_count.0.is_multiple_of(2);
        let (history_write, history_read) = if even {
            (history_1, history_2)
        } else {
            (history_2, history_1)
        };
        let (accumulated_write, accumulated_read, meta_write, meta_read) = if even {
            (accumulated_1, accumulated_2, meta_1, meta_2)
        } else {
            (accumulated_2, accumulated_1, meta_2, meta_1)
        };
        commands.entity(entity).insert((
            ScreenSpaceIndirectLightTexture(gi.default_view.clone()),
            SsgiTextures {
                history_write,
                history_read,
                raw,
                raw_depth,
                accumulated_write,
                accumulated_read,
                meta_write,
                meta_read,
                gi,
            },
        ));
    }
}

/// Settings as the trace shader reads them.
#[derive(ShaderType, Clone, Default)]
struct SsgiSettingsUniform {
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

/// A view's settings uniform for the trace.
#[derive(Component, Default)]
pub struct SsgiSettingsBuffer(UniformBuffer<SsgiSettingsUniform>);

fn prepare_ssgi_settings(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    frame_count: Res<FrameCount>,
    mut views: Query<(
        Entity,
        &ScreenSpaceGlobalIllumination,
        Option<&mut SsgiSettingsBuffer>,
    )>,
) {
    for (entity, ssgi, buffer) in &mut views {
        let uniform = SsgiSettingsUniform {
            intensity: ssgi.intensity.max(0.0),
            radius: ssgi.radius.max(0.01),
            thickness: ssgi.thickness.max(0.0),
            slice_count: ssgi.slice_count.max(1),
            samples_per_slice_side: ssgi.samples_per_slice_side.max(1),
            frame: frame_count.0,
            half_resolution: ssgi.half_resolution.into(),
            denoise: ssgi.denoise.into(),
            max_history_frames: ssgi.max_history_frames.max(1),
        };
        match buffer {
            Some(mut buffer) => {
                buffer.0.set(uniform);
                buffer.0.write_buffer(&render_device, &render_queue);
            }
            None => {
                let mut buffer = UniformBuffer::from(uniform);
                buffer.set_label(Some("ssgi_settings"));
                buffer.write_buffer(&render_device, &render_queue);
                commands.entity(entity).insert(SsgiSettingsBuffer(buffer));
            }
        }
    }
}

/// Bind group layouts, samplers, shaders and the trace pipeline for SSGI's passes.
#[derive(Resource)]
pub struct SsgiPipeline {
    layout: BindGroupLayoutDescriptor,
    trace_layout: BindGroupLayoutDescriptor,
    trace_pipeline: CachedComputePipelineId,
    temporal_layout: BindGroupLayoutDescriptor,
    temporal_pipeline: CachedComputePipelineId,
    spatial_layout: BindGroupLayoutDescriptor,
    spatial_pipeline: CachedComputePipelineId,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    fragment_shader: Handle<Shader>,
}

fn init_ssgi_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    fullscreen_shader: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "ssgi_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                // This frame's lit image
                texture_2d(TextureSampleType::Float { filterable: true }),
                // Last frame's lit image (history)
                texture_2d(TextureSampleType::Float { filterable: true }),
                // Motion vectors
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                // Trace output
                texture_2d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    );
    let trace_layout = BindGroupLayoutDescriptor::new(
        "ssgi_trace_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_depth_2d(),
                // Normals
                texture_2d(TextureSampleType::Float { filterable: false }),
                // Motion vectors
                texture_2d(TextureSampleType::Float { filterable: false }),
                // History
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_storage_2d(GI_FORMAT, StorageTextureAccess::WriteOnly),
                texture_storage_2d(RAW_DEPTH_FORMAT, StorageTextureAccess::WriteOnly),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<SsgiSettingsUniform>(false),
            ),
        ),
    );
    let unfilterable = || texture_2d(TextureSampleType::Float { filterable: false });
    let temporal_layout = BindGroupLayoutDescriptor::new(
        "ssgi_temporal_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_depth_2d(),
                // Motion vectors, raw trace, raw depth, accumulated and meta from last frame
                unfilterable(),
                unfilterable(),
                unfilterable(),
                unfilterable(),
                unfilterable(),
                texture_storage_2d(GI_FORMAT, StorageTextureAccess::WriteOnly),
                texture_storage_2d(META_FORMAT, StorageTextureAccess::WriteOnly),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<SsgiSettingsUniform>(false),
            ),
        ),
    );
    let spatial_layout = BindGroupLayoutDescriptor::new(
        "ssgi_spatial_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_depth_2d(),
                // Normals, accumulated and meta from this frame
                unfilterable(),
                unfilterable(),
                unfilterable(),
                texture_storage_2d(GI_FORMAT, StorageTextureAccess::WriteOnly),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<SsgiSettingsUniform>(false),
            ),
        ),
    );
    let compute_pipeline =
        |label: &'static str, layout: &BindGroupLayoutDescriptor, shader, entry: &'static str| {
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some(label.into()),
                layout: vec![layout.clone()],
                shader,
                entry_point: Some(entry.into()),
                ..default()
            })
        };
    let temporal_pipeline = compute_pipeline(
        "ssgi_temporal_pipeline",
        &temporal_layout,
        load_embedded_asset!(asset_server.as_ref(), "temporal.wesl"),
        "temporal",
    );
    let spatial_pipeline = compute_pipeline(
        "ssgi_spatial_pipeline",
        &spatial_layout,
        load_embedded_asset!(asset_server.as_ref(), "spatial.wesl"),
        "spatial",
    );
    let trace_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("ssgi_trace_pipeline".into()),
        layout: vec![trace_layout.clone()],
        shader: load_embedded_asset!(asset_server.as_ref(), "trace.wesl"),
        entry_point: Some("trace".into()),
        ..default()
    });
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        label: Some("ssgi_linear_sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(SsgiPipeline {
        layout,
        trace_layout,
        trace_pipeline,
        temporal_layout,
        temporal_pipeline,
        spatial_layout,
        spatial_pipeline,
        sampler,
        fullscreen_shader: fullscreen_shader.clone(),
        fragment_shader: load_embedded_asset!(asset_server.as_ref(), "ssgi.wesl"),
    });
}

/// Which SSGI pass a pipeline is for, and the format it writes.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
pub struct SsgiPipelineKey {
    /// `Off` builds the pass that copies the image into the history.
    debug_view: SsgiDebugView,
    target_format: TextureFormat,
}

impl SpecializedRenderPipeline for SsgiPipeline {
    type Key = SsgiPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let (label, entry_point) = match key.debug_view.shader_def() {
            None => ("ssgi_copy_history_pipeline", "copy_history"),
            Some(_) => ("ssgi_debug_pipeline", "debug_view"),
        };
        RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.fragment_shader.clone(),
                shader_defs: key
                    .debug_view
                    .shader_def()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                entry_point: Some(entry_point.into()),
                targets: vec![Some(ColorTargetState {
                    format: key.target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

/// The SSGI pipelines a view uses this frame.
#[derive(Component)]
pub struct SsgiPipelineIds {
    copy_history: CachedRenderPipelineId,
    debug_view: Option<CachedRenderPipelineId>,
}

fn prepare_ssgi_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<SsgiPipeline>>,
    pipeline: Res<SsgiPipeline>,
    views: Query<(Entity, &ExtractedView, &ScreenSpaceGlobalIllumination)>,
) {
    for (entity, view, ssgi) in &views {
        let copy_history = pipelines.specialize(
            &pipeline_cache,
            &pipeline,
            SsgiPipelineKey {
                debug_view: SsgiDebugView::Off,
                target_format: HISTORY_FORMAT,
            },
        );
        let debug_view = (ssgi.debug_view != SsgiDebugView::Off).then(|| {
            pipelines.specialize(
                &pipeline_cache,
                &pipeline,
                SsgiPipelineKey {
                    debug_view: ssgi.debug_view,
                    target_format: view.target_format,
                },
            )
        });
        commands.entity(entity).insert(SsgiPipelineIds {
            copy_history,
            debug_view,
        });
    }
}

/// Traces bounce light and ambient occlusion, then denoises it into the GI texture.
fn ssgi_trace(
    view: ViewQuery<(
        &ExtractedCamera,
        &ScreenSpaceGlobalIllumination,
        &ViewPrepassTextures,
        &SsgiTextures,
        &SsgiSettingsBuffer,
        &ViewUniformOffset,
    )>,
    ssgi_pipeline: Res<SsgiPipeline>,
    pipeline_cache: Res<PipelineCache>,
    view_uniforms: Res<ViewUniforms>,
    mut ctx: RenderContext,
) {
    let (camera, ssgi, prepass_textures, textures, settings, view_uniform_offset) =
        view.into_inner();
    let (
        Some(viewport_size),
        Some(depth),
        Some(normals),
        Some(motion_vectors),
        Some(view_binding),
        Some(settings_binding),
        Some(trace_pipeline),
        Some(temporal_pipeline),
        Some(spatial_pipeline),
    ) = (
        camera.physical_viewport_size,
        prepass_textures.depth_only_view(),
        prepass_textures.normal_view(),
        prepass_textures.motion_vectors_view(),
        view_uniforms.uniforms.binding(),
        settings.0.binding(),
        pipeline_cache.get_compute_pipeline(ssgi_pipeline.trace_pipeline),
        pipeline_cache.get_compute_pipeline(ssgi_pipeline.temporal_pipeline),
        pipeline_cache.get_compute_pipeline(ssgi_pipeline.spatial_pipeline),
    )
    else {
        return;
    };

    let render_device = ctx.render_device().clone();
    let trace_bind_group = render_device.create_bind_group(
        Some("ssgi_trace_bind_group"),
        &pipeline_cache.get_bind_group_layout(&ssgi_pipeline.trace_layout),
        &BindGroupEntries::sequential((
            depth,
            normals,
            motion_vectors,
            &textures.history_read.default_view,
            &textures.raw.default_view,
            &textures.raw_depth.default_view,
            view_binding.clone(),
            settings_binding.clone(),
        )),
    );
    let temporal_bind_group = render_device.create_bind_group(
        Some("ssgi_temporal_bind_group"),
        &pipeline_cache.get_bind_group_layout(&ssgi_pipeline.temporal_layout),
        &BindGroupEntries::sequential((
            depth,
            motion_vectors,
            &textures.raw.default_view,
            &textures.raw_depth.default_view,
            &textures.accumulated_read.default_view,
            &textures.meta_read.default_view,
            &textures.accumulated_write.default_view,
            &textures.meta_write.default_view,
            view_binding.clone(),
            settings_binding.clone(),
        )),
    );
    let spatial_bind_group = render_device.create_bind_group(
        Some("ssgi_spatial_bind_group"),
        &pipeline_cache.get_bind_group_layout(&ssgi_pipeline.spatial_layout),
        &BindGroupEntries::sequential((
            depth,
            normals,
            &textures.accumulated_write.default_view,
            &textures.meta_write.default_view,
            &textures.gi.default_view,
            view_binding,
            settings_binding,
        )),
    );

    let trace_size = if ssgi.half_resolution {
        (viewport_size + UVec2::ONE) / 2
    } else {
        viewport_size
    };
    let passes = [
        ("ssgi_trace", trace_pipeline, &trace_bind_group, trace_size),
        (
            "ssgi_temporal",
            temporal_pipeline,
            &temporal_bind_group,
            viewport_size,
        ),
        (
            "ssgi_spatial",
            spatial_pipeline,
            &spatial_bind_group,
            viewport_size,
        ),
    ];

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let time_span = diagnostics.time_span(ctx.command_encoder(), "ssgi");
    for (label, pipeline, bind_group, size) in passes {
        let mut pass = ctx
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[view_uniform_offset.offset]);
        pass.dispatch_workgroups(size.x.div_ceil(8), size.y.div_ceil(8), 1);
    }
    time_span.end(ctx.command_encoder());
}

/// Copies the lit image into the history and, when a debug view is on, draws it over the image.
fn ssgi_store_history_and_debug(
    view: ViewQuery<(
        &ViewTarget,
        &ViewPrepassTextures,
        &SsgiTextures,
        &SsgiPipelineIds,
    )>,
    ssgi_pipeline: Res<SsgiPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (view_target, prepass_textures, textures, pipeline_ids) = view.into_inner();
    let Some(motion_vectors) = prepass_textures.motion_vectors_view() else {
        return;
    };
    let Some(copy_pipeline) = pipeline_cache.get_render_pipeline(pipeline_ids.copy_history) else {
        return;
    };
    let layout = pipeline_cache.get_bind_group_layout(&ssgi_pipeline.layout);
    let bind_group_for = |ctx: &RenderContext, image: &TextureView| {
        ctx.render_device().create_bind_group(
            Some("ssgi_bind_group"),
            &layout,
            &BindGroupEntries::sequential((
                image,
                &textures.history_read.default_view,
                motion_vectors,
                &ssgi_pipeline.sampler,
                &textures.gi.default_view,
            )),
        )
    };

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();

    // Store the lit image before any debug view replaces it.
    let lit_image = view_target.main_texture_view();
    let bind_group = bind_group_for(&ctx, lit_image);
    {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("ssgi_copy_history"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &textures.history_write.default_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let span = diagnostics.pass_span(&mut pass, "ssgi_copy_history");
        pass.set_render_pipeline(copy_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
    }

    let Some(debug_pipeline) = pipeline_ids
        .debug_view
        .and_then(|id| pipeline_cache.get_render_pipeline(id))
    else {
        return;
    };
    let post_process = view_target.post_process_write();
    let bind_group = bind_group_for(&ctx, post_process.source);
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("ssgi_debug_view"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: post_process.destination,
            depth_slice: None,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let span = diagnostics.pass_span(&mut pass, "ssgi_debug_view");
    pass.set_render_pipeline(debug_pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
    span.end(&mut pass);
}
