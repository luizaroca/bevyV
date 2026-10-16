//! Screen-space reflections (SSR) that work with forward and deferred rendering.
//!
//! Work in progress, built in phases (see `notes/ssr-plan.md` in the project files). Bevy's own
//! `bevy_pbr::ScreenSpaceReflections` needs the deferred renderer, misses the sky and transparent
//! objects, and is noisy without TAA. This one traces against last frame's finished image, like
//! SSGI, so it doesn't depend on how the frame is lit.
//!
//! Each frame:
//! 1. After the prepass, `trace.wesl` traces one reflection ray per pixel through the depth
//!    buffer and takes the light it hits from last frame's lit image (the history). Roughness
//!    comes from the normal prepass, which this fork extends to carry it.
//! 2. The main pass (forward or deferred) uses the reflections as each opaque surface's specular
//!    indirect light, through the `ScreenSpaceSpecular` hook this fork adds to `bevy_pbr`. Where
//!    rays miss, it blends back to the environment map and ambient light.
//! 3. After the main pass, the lit image is copied into the history for the next frame, so
//!    reflections show up in other reflections, and a debug view can replace the image.
//!
//! The reflections are not denoised yet (phase 4).

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
use bevy_pbr::{ScreenSpaceSpecular, ScreenSpaceSpecularTexture};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::{
    camera::ExtractedCamera,
    diagnostic::RecordDiagnostics,
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    render_resource::{
        binding_types::{
            sampler, texture_2d, texture_depth_2d, texture_storage_2d, uniform_buffer,
        },
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
        CachedComputePipelineId, CachedRenderPipelineId, ColorTargetState, ColorWrites,
        ComputePassDescriptor, ComputePipelineDescriptor, FilterMode, FragmentState, Operations,
        PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor,
        Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType,
        SpecializedRenderPipeline, SpecializedRenderPipelines, StorageTextureAccess,
        TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
        TextureView, UniformBuffer,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
    sync_component::SyncComponent,
    texture::{CachedTexture, TextureCache},
    view::{ExtractedView, Msaa, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy_shader::Shader;
use bevy_utils::{default, once};
use tracing::error;

/// Format of the radiance history. Lit HDR color needs more range than 8 bits.
const HISTORY_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Format of the trace output: reflected light in rgb, how much to trust it in a.
const REFLECTIONS_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Adds Bevy Extend's screen-space reflections. Part of [`ExtendPlugins`](crate::ExtendPlugins).
#[derive(Default)]
pub struct ExtendScreenSpaceReflectionsPlugin;

/// Add this to a 3D camera to enable Bevy Extend's screen-space reflections.
///
/// Unlike Bevy's `ScreenSpaceReflections`, it works with forward rendering as well as deferred,
/// and reflects everything in last frame's image, including the sky and transparent objects. It
/// needs the depth, normal and motion vector prepasses (added automatically), an untonemapped
/// main texture ([`TonemappingPass`], also added) and `Msaa::Off`.
///
/// Only `StandardMaterial` surfaces reflect, because they store their roughness in the normal
/// prepass; other materials count as fully rough.
///
/// Work in progress: the reflections are one noisy ray per pixel and not denoised yet, so rough
/// surfaces look grainy without TAA.
#[derive(Component, Reflect, Clone, PartialEq, Debug)]
#[reflect(Component, Default, Clone, PartialEq)]
#[require(DepthPrepass, NormalPrepass, MotionVectorPrepass, TonemappingPass)]
pub struct ExtendScreenSpaceReflections {
    /// Surfaces at least this rough (perceptual roughness) get no screen-space reflections, only
    /// the environment map. Reflections fade out over the last fifth of the range.
    pub max_roughness: f32,
    /// How thick surfaces are assumed to be, in meters. The depth buffer only shows the front of
    /// things, so a ray passing further behind a surface than this goes on past it.
    pub thickness: f32,
    /// How far a reflection ray travels, in meters.
    pub max_distance: f32,
    /// Depth buffer samples along each ray to find a hit. More finds thinner objects.
    pub steps: u32,
    /// Extra samples to pin down exactly where a ray hits, once it has.
    pub refine_steps: u32,
    /// Fraction of the screen over which reflections fade out near its edges.
    pub edge_fade: f32,
    /// Replaces the image with one of SSR's buffers, for checking them by eye.
    pub debug_view: SsrDebugView,
}

impl Default for ExtendScreenSpaceReflections {
    fn default() -> Self {
        Self {
            max_roughness: 0.6,
            thickness: 0.3,
            max_distance: 30.0,
            steps: 48,
            refine_steps: 6,
            edge_fade: 0.08,
            debug_view: SsrDebugView::Off,
        }
    }
}

/// What [`ExtendScreenSpaceReflections::debug_view`] shows.
#[derive(Reflect, Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
#[reflect(Default, Clone, PartialEq, Hash)]
pub enum SsrDebugView {
    /// The normal image.
    #[default]
    Off,
    /// The light each pixel's reflection ray found, before the surface's specular color is
    /// applied, dimmed by how much it is trusted. Black where rays missed.
    Reflections,
    /// How much each pixel's reflection is trusted: white is a solid hit, black falls back to the
    /// environment map.
    Confidence,
    /// The perceptual roughness stored in the normal prepass: black is a mirror, white is fully
    /// rough (and what other materials than `StandardMaterial` show).
    Roughness,
}

impl SsrDebugView {
    /// Every view, in the order a "next view" key should cycle through them.
    pub const ALL: [Self; 4] = [
        Self::Off,
        Self::Reflections,
        Self::Confidence,
        Self::Roughness,
    ];

    /// The next view in [`Self::ALL`], wrapping around.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|view| *view == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    fn shader_def(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Reflections => Some("DEBUG_REFLECTIONS"),
            Self::Confidence => Some("DEBUG_CONFIDENCE"),
            Self::Roughness => Some("DEBUG_ROUGHNESS"),
        }
    }
}

impl Plugin for ExtendScreenSpaceReflectionsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "ssr.wesl");
        embedded_asset!(app, "trace.wesl");

        app.add_plugins(ExtractComponentPlugin::<ExtendScreenSpaceReflections>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<SpecializedRenderPipelines<SsrPipeline>>()
            .add_systems(RenderStartup, init_ssr_pipeline)
            .add_systems(
                Render,
                (
                    prepare_ssr_pipelines.in_set(RenderSystems::Prepare),
                    (prepare_ssr_textures, prepare_ssr_settings)
                        .in_set(RenderSystems::PrepareResources),
                ),
            )
            .add_systems(
                Core3d,
                (
                    // Needs this frame's depth, normals and motion vectors, and must finish
                    // before the main pass so lighting can use it (from phase 3).
                    ssr_trace
                        .after(Core3dSystems::Prepass)
                        .before(Core3dSystems::MainPass),
                    // After lighting and transparents, before TAA and other post-processing.
                    ssr_store_history_and_debug
                        .after(Core3dSystems::MainPass)
                        .before(Core3dSystems::EarlyPostProcess),
                ),
            );
    }
}

impl SyncComponent<RenderApp> for ExtendScreenSpaceReflections {
    type Target = (
        Self,
        ScreenSpaceSpecular,
        ScreenSpaceSpecularTexture,
        SsrTextures,
        SsrPipelineIds,
        SsrSettingsBuffer,
    );
}

impl ExtractComponent<RenderApp> for ExtendScreenSpaceReflections {
    type QueryData = (Read<Self>, Read<Msaa>);
    type QueryFilter = With<Camera3d>;
    /// The marker tells `bevy_pbr` to build lighting pipelines that read SSR's output.
    type Out = (Self, ScreenSpaceSpecular);

    fn extract_component((ssr, msaa): QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        if *msaa != Msaa::Off {
            once!(error!(
                "Screen-space reflections need Msaa::Off, but the camera uses Msaa::{:?}",
                *msaa
            ));
            return None;
        }
        Some((ssr.clone(), ScreenSpaceSpecular))
    }
}

/// A view's SSR textures. The radiance history is a pair: last frame's image is read while this
/// frame's is written, and they swap every frame.
#[derive(Component)]
pub struct SsrTextures {
    history_write: CachedTexture,
    history_read: CachedTexture,
    /// Trace output, viewport-relative.
    reflections: CachedTexture,
}

fn prepare_ssr_textures(
    mut commands: Commands,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    frame_count: Res<FrameCount>,
    cameras: Query<(Entity, &ExtractedCamera), With<ExtendScreenSpaceReflections>>,
) {
    for (entity, camera) in &cameras {
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
        descriptor.label = Some("ssr_history_1_texture");
        let history_1 = texture_cache.get(&render_device, descriptor.clone());
        descriptor.label = Some("ssr_history_2_texture");
        let history_2 = texture_cache.get(&render_device, descriptor.clone());
        let reflections = texture_cache.get(
            &render_device,
            TextureDescriptor {
                label: Some("ssr_reflections_texture"),
                format: REFLECTIONS_FORMAT,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                ..descriptor
            },
        );

        let (history_write, history_read) = if frame_count.0.is_multiple_of(2) {
            (history_1, history_2)
        } else {
            (history_2, history_1)
        };
        commands.entity(entity).insert((
            ScreenSpaceSpecularTexture(reflections.default_view.clone()),
            SsrTextures {
                history_write,
                history_read,
                reflections,
            },
        ));
    }
}

/// Settings as the trace shader reads them.
#[derive(ShaderType, Clone, Default)]
struct SsrSettingsUniform {
    max_roughness: f32,
    thickness: f32,
    max_distance: f32,
    steps: u32,
    refine_steps: u32,
    edge_fade: f32,
    frame: u32,
}

/// A view's settings uniform for the trace.
#[derive(Component, Default)]
pub struct SsrSettingsBuffer(UniformBuffer<SsrSettingsUniform>);

fn prepare_ssr_settings(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    frame_count: Res<FrameCount>,
    mut views: Query<(
        Entity,
        &ExtendScreenSpaceReflections,
        Option<&mut SsrSettingsBuffer>,
    )>,
) {
    for (entity, ssr, buffer) in &mut views {
        let uniform = SsrSettingsUniform {
            max_roughness: ssr.max_roughness.clamp(0.0, 1.0),
            thickness: ssr.thickness.max(0.001),
            max_distance: ssr.max_distance.max(0.01),
            steps: ssr.steps.max(1),
            refine_steps: ssr.refine_steps,
            edge_fade: ssr.edge_fade.clamp(0.0, 0.5),
            frame: frame_count.0,
        };
        match buffer {
            Some(mut buffer) => {
                buffer.0.set(uniform);
                buffer.0.write_buffer(&render_device, &render_queue);
            }
            None => {
                let mut buffer = UniformBuffer::from(uniform);
                buffer.set_label(Some("ssr_settings"));
                buffer.write_buffer(&render_device, &render_queue);
                commands.entity(entity).insert(SsrSettingsBuffer(buffer));
            }
        }
    }
}

/// Bind group layouts, sampler, shaders and the trace pipeline for SSR's passes.
#[derive(Resource)]
pub struct SsrPipeline {
    layout: BindGroupLayoutDescriptor,
    trace_layout: BindGroupLayoutDescriptor,
    trace_pipeline: CachedComputePipelineId,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    fragment_shader: Handle<Shader>,
}

fn init_ssr_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    fullscreen_shader: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
) {
    let unfilterable = || texture_2d(TextureSampleType::Float { filterable: false });
    let layout = BindGroupLayoutDescriptor::new(
        "ssr_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                // This frame's lit image, the reflections and the normals (for roughness)
                unfilterable(),
                unfilterable(),
                unfilterable(),
            ),
        ),
    );
    let trace_layout = BindGroupLayoutDescriptor::new(
        "ssr_trace_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_depth_2d(),
                // Normals and roughness
                unfilterable(),
                // Motion vectors
                unfilterable(),
                // History
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_storage_2d(REFLECTIONS_FORMAT, StorageTextureAccess::WriteOnly),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<SsrSettingsUniform>(false),
            ),
        ),
    );
    let trace_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("ssr_trace_pipeline".into()),
        layout: vec![trace_layout.clone()],
        shader: load_embedded_asset!(asset_server.as_ref(), "trace.wesl"),
        entry_point: Some("trace".into()),
        ..default()
    });
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        label: Some("ssr_linear_sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(SsrPipeline {
        layout,
        trace_layout,
        trace_pipeline,
        sampler,
        fullscreen_shader: fullscreen_shader.clone(),
        fragment_shader: load_embedded_asset!(asset_server.as_ref(), "ssr.wesl"),
    });
}

/// Which SSR pass a pipeline is for, and the format it writes.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
pub struct SsrPipelineKey {
    /// `Off` builds the pass that copies the image into the history.
    debug_view: SsrDebugView,
    target_format: TextureFormat,
}

impl SpecializedRenderPipeline for SsrPipeline {
    type Key = SsrPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let (label, entry_point) = match key.debug_view.shader_def() {
            None => ("ssr_copy_history_pipeline", "copy_history"),
            Some(_) => ("ssr_debug_pipeline", "debug_view"),
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

/// The SSR pipelines a view uses this frame.
#[derive(Component)]
pub struct SsrPipelineIds {
    copy_history: CachedRenderPipelineId,
    debug_view: Option<CachedRenderPipelineId>,
}

fn prepare_ssr_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<SsrPipeline>>,
    pipeline: Res<SsrPipeline>,
    views: Query<(Entity, &ExtractedView, &ExtendScreenSpaceReflections)>,
) {
    for (entity, view, ssr) in &views {
        let copy_history = pipelines.specialize(
            &pipeline_cache,
            &pipeline,
            SsrPipelineKey {
                debug_view: SsrDebugView::Off,
                target_format: HISTORY_FORMAT,
            },
        );
        let debug_view = (ssr.debug_view != SsrDebugView::Off).then(|| {
            pipelines.specialize(
                &pipeline_cache,
                &pipeline,
                SsrPipelineKey {
                    debug_view: ssr.debug_view,
                    target_format: view.target_format,
                },
            )
        });
        commands.entity(entity).insert(SsrPipelineIds {
            copy_history,
            debug_view,
        });
    }
}

/// Traces one reflection ray per pixel into the reflections texture.
fn ssr_trace(
    view: ViewQuery<(
        &ExtractedCamera,
        &ViewPrepassTextures,
        &SsrTextures,
        &SsrSettingsBuffer,
        &ViewUniformOffset,
    )>,
    ssr_pipeline: Res<SsrPipeline>,
    pipeline_cache: Res<PipelineCache>,
    view_uniforms: Res<ViewUniforms>,
    mut ctx: RenderContext,
) {
    let (camera, prepass_textures, textures, settings, view_uniform_offset) = view.into_inner();
    let (
        Some(viewport_size),
        Some(depth),
        Some(normals),
        Some(motion_vectors),
        Some(view_binding),
        Some(settings_binding),
        Some(trace_pipeline),
    ) = (
        camera.physical_viewport_size,
        prepass_textures.depth_only_view(),
        prepass_textures.normal_view(),
        prepass_textures.motion_vectors_view(),
        view_uniforms.uniforms.binding(),
        settings.0.binding(),
        pipeline_cache.get_compute_pipeline(ssr_pipeline.trace_pipeline),
    )
    else {
        return;
    };

    let bind_group = ctx.render_device().create_bind_group(
        Some("ssr_trace_bind_group"),
        &pipeline_cache.get_bind_group_layout(&ssr_pipeline.trace_layout),
        &BindGroupEntries::sequential((
            depth,
            normals,
            motion_vectors,
            &textures.history_read.default_view,
            &ssr_pipeline.sampler,
            &textures.reflections.default_view,
            view_binding,
            settings_binding,
        )),
    );

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let time_span = diagnostics.time_span(ctx.command_encoder(), "ssr");
    {
        let mut pass = ctx
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("ssr_trace"),
                timestamp_writes: None,
            });
        pass.set_pipeline(trace_pipeline);
        pass.set_bind_group(0, &bind_group, &[view_uniform_offset.offset]);
        pass.dispatch_workgroups(viewport_size.x.div_ceil(8), viewport_size.y.div_ceil(8), 1);
    }
    time_span.end(ctx.command_encoder());
}

/// Copies the lit image into the history and, when a debug view is on, draws it over the image.
fn ssr_store_history_and_debug(
    view: ViewQuery<(
        &ViewTarget,
        &ViewPrepassTextures,
        &SsrTextures,
        &SsrPipelineIds,
    )>,
    ssr_pipeline: Res<SsrPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (view_target, prepass_textures, textures, pipeline_ids) = view.into_inner();
    let Some(normals) = prepass_textures.normal_view() else {
        return;
    };
    let Some(copy_pipeline) = pipeline_cache.get_render_pipeline(pipeline_ids.copy_history) else {
        return;
    };
    let layout = pipeline_cache.get_bind_group_layout(&ssr_pipeline.layout);
    let bind_group_for = |ctx: &RenderContext, image: &TextureView| {
        ctx.render_device().create_bind_group(
            Some("ssr_bind_group"),
            &layout,
            &BindGroupEntries::sequential((image, &textures.reflections.default_view, normals)),
        )
    };

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();

    // Store the lit image before any debug view replaces it.
    let bind_group = bind_group_for(&ctx, view_target.main_texture_view());
    {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("ssr_copy_history"),
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
        let span = diagnostics.pass_span(&mut pass, "ssr_copy_history");
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
        label: Some("ssr_debug_view"),
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
    let span = diagnostics.pass_span(&mut pass, "ssr_debug_view");
    pass.set_render_pipeline(debug_pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
    span.end(&mut pass);
}
