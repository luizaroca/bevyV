//! The glass layer: where transparent and transmissive surfaces are, so reflections can show them.
//!
//! Transparent objects are drawn after the opaque ones and don't write depth, so the depth
//! buffer SSR traces against doesn't know where they are. This pass draws the depth of every
//! visible see-through `StandardMaterial` mesh (alpha blended, or with specular transmission)
//! into a separate depth texture, after the prepass. Glass hidden behind opaque surfaces is left
//! out, so the texture holds the nearest glass in front of the opaque scene at each pixel, and
//! nothing where there is none.
//!
//! The trace treats that texture as a second layer: a ray that hits it takes its light from the
//! history copy made after transparents were drawn, so the glass shows up in reflections where it
//! really is.
//!
//! Only plain meshes are drawn: skinned, morphed and compressed-position meshes are skipped.

use bevy_asset::{AssetId, Assets, Handle};
use bevy_camera::visibility::ViewVisibility;
use bevy_ecs::{
    resource::Resource,
    system::{Query, Res, ResMut},
};
use bevy_material::AlphaMode;
use bevy_math::Mat4;
use bevy_mesh::{
    BaseMeshPipelineKey, Mesh, Mesh3d, MeshAttributeCompressionFlags, MeshVertexBufferLayoutRef,
};
use bevy_pbr::{MeshMaterial3d, StandardMaterial};
use bevy_platform::collections::HashMap;
use bevy_render::{
    camera::ExtractedCamera,
    mesh::{allocator::MeshAllocator, RenderMesh, RenderMeshBufferInfo},
    render_asset::RenderAssets,
    render_resource::{
        binding_types::{texture_depth_2d, uniform_buffer},
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
        CachedRenderPipelineId, CompareFunction, DepthBiasState, DepthStencilState,
        DynamicUniformBuffer, FragmentState, LoadOp, Operations, PipelineCache, PrimitiveState,
        PrimitiveTopology, RenderPassDepthStencilAttachment, RenderPassDescriptor,
        RenderPipelineDescriptor, ShaderStages, ShaderType, StencilState, StoreOp, TextureFormat,
        VertexState,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
    view::{ViewUniform, ViewUniformOffset, ViewUniforms},
    Extract,
};
use bevy_shader::Shader;
use bevy_transform::components::GlobalTransform;
use bevy_utils::default;
use tracing::warn;

use bevy_core_pipeline::prepass::ViewPrepassTextures;

use super::{ExtendScreenSpaceReflections, SsrTextures};

/// Format of the glass layer's depth.
pub(super) const GLASS_DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

/// One see-through mesh to draw into the glass layer.
struct GlassInstance {
    mesh: AssetId<Mesh>,
    world_from_local: Mat4,
    /// Its transform's offset in [`GlassPipeline::draws`], once prepared.
    draw_offset: u32,
    /// The pipeline for its vertex layout, once prepared.
    pipeline: Option<CachedRenderPipelineId>,
}

/// Every visible see-through mesh this frame.
#[derive(Resource, Default)]
pub struct ExtractedGlass(Vec<GlassInstance>);

/// Whether a material lets you see through it, so it isn't in the depth buffer.
fn is_glass(material: &StandardMaterial) -> bool {
    matches!(
        material.alpha_mode,
        AlphaMode::Blend | AlphaMode::Premultiplied
    ) || material.specular_transmission > 0.0
}

pub(super) fn extract_glass(
    mut glass: ResMut<ExtractedGlass>,
    materials: Extract<Res<Assets<StandardMaterial>>>,
    meshes: Extract<
        Query<(
            &Mesh3d,
            &MeshMaterial3d<StandardMaterial>,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
) {
    glass.0.clear();
    for (mesh, material, transform, visibility) in &meshes {
        if !visibility.get() || !materials.get(&material.0).is_some_and(is_glass) {
            continue;
        }
        glass.0.push(GlassInstance {
            mesh: mesh.id(),
            world_from_local: transform.to_matrix(),
            draw_offset: 0,
            pipeline: None,
        });
    }
}

/// A glass mesh's transform, as the shader reads it.
#[derive(ShaderType, Clone, Default)]
struct GlassDraw {
    world_from_local: Mat4,
}

/// The glass layer's bind group layout, shader, per-mesh transforms and pipelines (one per mesh
/// vertex layout and topology).
#[derive(Resource)]
pub struct GlassPipeline {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    draws: DynamicUniformBuffer<GlassDraw>,
    pipelines: HashMap<(MeshVertexBufferLayoutRef, PrimitiveTopology), CachedRenderPipelineId>,
}

impl GlassPipeline {
    pub(super) fn new(shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "ssr_glass_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer::<ViewUniform>(true),
                    uniform_buffer::<GlassDraw>(true),
                    // The prepass depth, to leave out glass behind opaque surfaces
                    texture_depth_2d(),
                ),
            ),
        );
        Self {
            layout,
            shader,
            draws: DynamicUniformBuffer::default(),
            pipelines: HashMap::default(),
        }
    }

    fn pipeline_for(
        &mut self,
        pipeline_cache: &PipelineCache,
        mesh: &RenderMesh,
    ) -> Option<CachedRenderPipelineId> {
        if mesh.key_bits.intersects(BaseMeshPipelineKey::MORPH_TARGETS)
            || mesh
                .layout
                .0
                .get_attribute_compression()
                .contains(MeshAttributeCompressionFlags::COMPRESS_POSITION)
        {
            return None;
        }
        let topology = mesh.primitive_topology();
        if let Some(id) = self.pipelines.get(&(mesh.layout.clone(), topology)) {
            return Some(*id);
        }
        let vertex_layout = mesh
            .layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])
            .ok()?;
        let id = pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("ssr_glass_depth_pipeline".into()),
            layout: vec![self.layout.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                entry_point: Some("vertex".into()),
                buffers: vec![vertex_layout],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                entry_point: Some("fragment".into()),
                targets: vec![],
                ..default()
            }),
            primitive: PrimitiveState {
                topology,
                // Glass is often seen from inside; the depth test keeps the nearest side anyway.
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: GLASS_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                // Reverse Z: nearer is greater.
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }),
            ..default()
        });
        self.pipelines.insert((mesh.layout.clone(), topology), id);
        Some(id)
    }
}

pub(super) fn prepare_glass(
    mut glass: ResMut<ExtractedGlass>,
    mut glass_pipeline: ResMut<GlassPipeline>,
    pipeline_cache: Res<PipelineCache>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    glass_pipeline.draws.clear();
    for instance in &mut glass.0 {
        instance.pipeline = render_meshes
            .get(instance.mesh)
            .and_then(|mesh| glass_pipeline.pipeline_for(&pipeline_cache, mesh));
        instance.draw_offset = glass_pipeline.draws.push(&GlassDraw {
            world_from_local: instance.world_from_local,
        });
    }
    glass_pipeline
        .draws
        .write_buffer(&render_device, &render_queue);
}

/// Draws the glass layer's depth. Runs every frame while glass reflections are on, so the layer
/// is cleared even with no glass in view.
pub(super) fn ssr_glass_depth(
    view: ViewQuery<(
        &ExtendScreenSpaceReflections,
        &ExtractedCamera,
        &ViewPrepassTextures,
        &SsrTextures,
        &ViewUniformOffset,
    )>,
    glass: Res<ExtractedGlass>,
    glass_pipeline: Res<GlassPipeline>,
    pipeline_cache: Res<PipelineCache>,
    view_uniforms: Res<ViewUniforms>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    mesh_allocator: Res<MeshAllocator>,
    mut ctx: RenderContext,
) {
    let (ssr, camera, prepass_textures, textures, view_uniform_offset) = view.into_inner();
    if !ssr.glass {
        // The trace ignores the glass layer when glass is off.
        return;
    }
    let (Some(opaque_depth), Some(view_binding), Some(draws_binding)) = (
        prepass_textures.depth_only_view(),
        view_uniforms.uniforms.binding(),
        glass_pipeline.draws.binding(),
    ) else {
        return;
    };
    let bind_group = ctx.render_device().create_bind_group(
        Some("ssr_glass_bind_group"),
        &pipeline_cache.get_bind_group_layout(&glass_pipeline.layout),
        &BindGroupEntries::sequential((view_binding, draws_binding, opaque_depth)),
    );

    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("ssr_glass_depth"),
        color_attachments: &[],
        depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
            view: &textures.glass_depth.default_view,
            depth_ops: Some(Operations {
                load: LoadOp::Clear(0.0),
                store: StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Some(viewport) = camera.viewport.as_ref() {
        pass.set_camera_viewport(viewport);
    }

    for instance in &glass.0 {
        let (Some(pipeline), Some(mesh), Some(vertices)) = (
            instance
                .pipeline
                .and_then(|id| pipeline_cache.get_render_pipeline(id)),
            render_meshes.get(instance.mesh),
            mesh_allocator.mesh_vertex_slice(&instance.mesh),
        ) else {
            continue;
        };
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(
            0,
            &bind_group,
            &[view_uniform_offset.offset, instance.draw_offset],
        );
        pass.set_vertex_buffer(0, vertices.buffer.slice(..));
        match &mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(indices) = mesh_allocator.mesh_index_slice(&instance.mesh) else {
                    warn!("Glass mesh has no index buffer; skipping it in SSR");
                    continue;
                };
                pass.set_index_buffer(indices.buffer.slice(..), *index_format);
                pass.draw_indexed(
                    indices.range.start..(indices.range.start + *count),
                    vertices.range.start as i32,
                    0..1,
                );
            }
            RenderMeshBufferInfo::NonIndexed => {
                pass.draw(vertices.range, 0..1);
            }
        }
    }
}
