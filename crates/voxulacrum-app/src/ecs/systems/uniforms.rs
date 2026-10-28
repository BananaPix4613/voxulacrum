//! Writes this frame's uniform buffers, and uploads the main side's versioned
//! render data.

use bevy_ecs::prelude::*;
use crate::ecs::resources::{
    GlobalUniformBuffer, LoadedPalette, ReflectionUniformBuffer, ShadowUniformBuffer,
    VisibilityMask, VisibilityMaskTexels,
};
use crate::meshing::cap::CapMesh;
use crate::meshing::overlay::{DebugLines, SkeletonPreview};
use crate::meshing::player::PlayerMesh;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::cloud_textures::CloudTextures;
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::skeleton_preview_pass::SkeletonPreviewPass;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::water_pass::WaterPass;
use crate::simulation::manager::{FrameState, SimulationManager};
use crate::ui::panels::UiState;

/// Copy the main side's render data into the passes, each only when its version
/// moved: the debug lines, the skeleton preview, the player mesh, the
/// cross-section cap, the visibility mask and the cloud texels. Runs in
/// UniformWrite after the systems that build that data.
pub fn render_upload_system(
    ctx: Res<RenderContext>,
    debug_lines: Res<DebugLines>,
    preview: Res<SkeletonPreview>,
    player_mesh: Res<PlayerMesh>,
    cap_mesh: Res<CapMesh>,
    mask_texels: Res<VisibilityMaskTexels>,
    sim: Res<SimulationManager>,
    mut debug_line_pass: ResMut<DebugLinePass>,
    mut preview_pass: ResMut<SkeletonPreviewPass>,
    mut player_pass: ResMut<PlayerPass>,
    mut cap_pass: ResMut<CapPass>,
    mut mask: ResMut<VisibilityMask>,
    mut cloud_textures: ResMut<CloudTextures>,
) {
    debug_line_pass.upload(
        &ctx.device,
        &debug_lines.boundaries,
        &debug_lines.highlight,
        &debug_lines.selection,
        &debug_lines.overlay,
    );
    preview_pass.upload(&ctx.device, &preview.0);
    player_pass.upload(&ctx.queue, &player_mesh.0);
    cap_pass.upload(&ctx.device, &cap_mesh.mesh);
    mask.upload(&ctx.queue, &mask_texels.0);
    cloud_textures.upload(
        &ctx.queue,
        &sim.cloud_shadow.detail_texels,
        &sim.cloud_shadow.envelope_texels,
    );
}

pub fn write_uniforms_system(
    ctx: Res<RenderContext>,
    surface: Res<SurfaceState>,
    frame: Res<FrameState>,
    ui: Res<UiState>,
    sim: Res<SimulationManager>,
    global_buf: Res<GlobalUniformBuffer>,
    shadow_buf: Res<ShadowUniformBuffer>,
    reflection_buf: Res<ReflectionUniformBuffer>,
    post_process: Res<PostProcessPass>,
    outline: Res<OutlinePass>,
    palette_pass: Res<PalettePass>,
    upscale: Res<UpscalePass>,
    water_pass: Res<WaterPass>,
    loaded_palette: Res<LoadedPalette>,
) {
    let reflect_plane = water_pass.avg_surface_y;
    crate::rendering::uniform_writer::write_all_uniforms(
        &ctx,
        &frame,
        &ui.params,
        &sim.cloud_shadow,
        &global_buf.0,
        &shadow_buf.0,
        &reflection_buf.0,
        reflect_plane,
        &post_process,
        &outline,
        &palette_pass,
        &upscale,
        &water_pass,
        &loaded_palette.0,
        surface.surface_config.width,
        surface.surface_config.height,
    );
}
