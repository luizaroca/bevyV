//! A hook for screen-space indirect (bounce) light computed outside `bevy_pbr`.
//!
//! Added by the Bevy Extend fork so its screen-space global illumination
//! (`bevy_extend::ssgi`) can feed the PBR shaders without `bevy_pbr` knowing how the light is
//! computed. A view opts in by carrying [`ScreenSpaceIndirectLight`] in the render world from
//! extraction on, and supplies the texture as [`ScreenSpaceIndirectLightTexture`] before bind
//! groups are prepared.

use bevy_ecs::component::Component;
use bevy_render::render_resource::TextureView;

/// Render-world marker: this view's PBR shaders read a [`ScreenSpaceIndirectLightTexture`].
///
/// It must be present when pipelines are specialized (insert it during extraction), so the mesh
/// view layout includes the texture binding.
#[derive(Component, Clone, Copy, Default, Debug)]
pub struct ScreenSpaceIndirectLight;

/// The screen-space indirect light for a view, as a non-filterable float texture the size of the
/// render target, read at each fragment's target pixel position:
///
/// - rgb: diffuse light arriving at the surface, in the units of the view's lit output (exposure
///   already applied). The shaders multiply it by the surface's diffuse color.
/// - a: ambient visibility (1 open, 0 fully occluded), used like SSAO for the other indirect light.
///
/// If a view has [`ScreenSpaceIndirectLight`] but no texture yet, a black fallback is bound.
#[derive(Component, Clone)]
pub struct ScreenSpaceIndirectLightTexture(pub TextureView);
