//! Screen-space global illumination (SSGI).
//!
//! Work in progress, built in phases (see `notes/ssgi-plan.md` in the project files). This first
//! phase lays the groundwork: every frame the lit HDR image is copied into a history texture, so
//! the next frame can gather bounce light from it. Debug views show that history and how well it
//! lines up with the current frame after reprojecting it with motion vectors. SSGI does not change
//! lighting yet.

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
    renderer::{RenderContext, RenderDevice, ViewQuery},
    sync_component::SyncComponent,
    texture::{CachedTexture, TextureCache},
    view::{ExtractedView, Msaa, ViewTarget},
    GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy_shader::Shader;
use bevy_utils::{default, once};
use tracing::error;

/// Format of the radiance history. Lit HDR color needs more range than 8 bits.
const HISTORY_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Adds screen-space global illumination. Part of [`ExtendPlugins`](crate::ExtendPlugins).
#[derive(Default)]
pub struct ScreenSpaceGlobalIlluminationPlugin;

/// Add this to a 3D camera to enable screen-space global illumination (SSGI).
///
/// SSGI gathers diffuse bounce light from what is on screen, using the previous frame's lit
/// image. It needs the depth, normal and motion vector prepasses (added automatically), an
/// untonemapped main texture ([`TonemappingPass`], also added) and `Msaa::Off`.
///
/// For now it only captures that history and offers [debug views](SsgiDebugView); it does not
/// change lighting yet.
#[derive(Component, Reflect, Clone, Default, PartialEq, Debug)]
#[reflect(Component, Default, Clone, PartialEq)]
#[require(DepthPrepass, NormalPrepass, MotionVectorPrepass, TonemappingPass)]
pub struct ScreenSpaceGlobalIllumination {
    /// Replaces the image with one of SSGI's inputs, for checking them by eye.
    pub debug_view: SsgiDebugView,
}

/// What [`ScreenSpaceGlobalIllumination::debug_view`] shows.
#[derive(Reflect, Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
#[reflect(Default, Clone, PartialEq, Hash)]
pub enum SsgiDebugView {
    /// The normal image.
    #[default]
    Off,
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
    pub const ALL: [Self; 4] = [
        Self::Off,
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
            Self::History => Some("DEBUG_HISTORY"),
            Self::Reprojected => Some("DEBUG_REPROJECTED"),
            Self::ReprojectionError => Some("DEBUG_REPROJECTION_ERROR"),
        }
    }
}

impl Plugin for ScreenSpaceGlobalIlluminationPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "ssgi.wesl");

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
                    prepare_ssgi_history_textures.in_set(RenderSystems::PrepareResources),
                ),
            )
            .add_systems(
                Core3d,
                // After lighting and transparents, before TAA and other post-processing.
                screen_space_global_illumination
                    .after(Core3dSystems::MainPass)
                    .before(Core3dSystems::EarlyPostProcess),
            );
    }
}

impl SyncComponent<RenderApp> for ScreenSpaceGlobalIllumination {
    type Target = (Self, SsgiHistoryTextures, SsgiPipelineIds);
}

impl ExtractComponent<RenderApp> for ScreenSpaceGlobalIllumination {
    type QueryData = (Read<Self>, Read<Msaa>);
    type QueryFilter = With<Camera3d>;
    type Out = Self;

    fn extract_component((ssgi, msaa): QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        if *msaa != Msaa::Off {
            once!(error!(
                "Screen-space global illumination needs Msaa::Off, but the camera uses Msaa::{:?}",
                *msaa
            ));
            return None;
        }
        Some(ssgi.clone())
    }
}

/// The radiance history: last frame's image is read while this frame's is written. The two
/// textures swap every frame.
#[derive(Component)]
pub struct SsgiHistoryTextures {
    write: CachedTexture,
    read: CachedTexture,
}

fn prepare_ssgi_history_textures(
    mut commands: Commands,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    frame_count: Res<FrameCount>,
    cameras: Query<(Entity, &ExtractedCamera), With<ScreenSpaceGlobalIllumination>>,
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
        descriptor.label = Some("ssgi_history_1_texture");
        let history_1 = texture_cache.get(&render_device, descriptor.clone());
        descriptor.label = Some("ssgi_history_2_texture");
        let history_2 = texture_cache.get(&render_device, descriptor);

        let (write, read) = if frame_count.0.is_multiple_of(2) {
            (history_1, history_2)
        } else {
            (history_2, history_1)
        };
        commands
            .entity(entity)
            .insert(SsgiHistoryTextures { write, read });
    }
}

/// Shared bind group layout, sampler and shader for SSGI's passes.
#[derive(Resource)]
pub struct SsgiPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    fragment_shader: Handle<Shader>,
}

fn init_ssgi_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
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
            ),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        label: Some("ssgi_linear_sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(SsgiPipeline {
        layout,
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

/// Copies the lit image into the history and, when a debug view is on, draws it over the image.
fn screen_space_global_illumination(
    view: ViewQuery<(
        &ViewTarget,
        &ViewPrepassTextures,
        &SsgiHistoryTextures,
        &SsgiPipelineIds,
    )>,
    ssgi_pipeline: Res<SsgiPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (view_target, prepass_textures, history, pipeline_ids) = view.into_inner();
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
                &history.read.default_view,
                motion_vectors,
                &ssgi_pipeline.sampler,
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
                view: &history.write.default_view,
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
