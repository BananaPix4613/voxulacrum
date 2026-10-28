//! Writes this frame's uniform buffers, uploads the main side's versioned and
//! per-chunk render data, and applies its render requests.

use bevy_ecs::prelude::*;
use crate::ecs::resources::{
    GlobalUniformBuffer, LoadedPalette, ReflectionUniformBuffer, RenderRequests,
    ShadowUniformBuffer, VisibilityMask, VisibilityMaskTexels,
};
use crate::meshing::cap::CapMesh;
use crate::meshing::coordinator::ChunkMeshes;
use crate::meshing::uploads::{ChunkUpload, ChunkUploads};
use crate::rendering::canopy_pass::CanopyPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::scatter_pass::ScatterPass;
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

/// Drain `ChunkUploads` into the terrain meshes and the per-chunk passes. Runs
/// in UniformWrite before everything that reads them: the stats, the uniforms
/// (the water plane) and the render graph.
pub fn chunk_upload_system(
    ctx: Res<RenderContext>,
    mut uploads: ResMut<ChunkUploads>,
    mut meshes: ResMut<ChunkMeshes>,
    mut water_pass: ResMut<WaterPass>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut capsules: ResMut<CapsulePass>,
    mut canopy: ResMut<CanopyPass>,
) {
    let device = &ctx.device;
    let uploads = &mut *uploads;
    for (pos, upload) in uploads.terrain.drain() {
        match upload {
            ChunkUpload::Set { version, data } => meshes.set(pos, version, data.as_ref(), device),
            ChunkUpload::Remove => meshes.remove(pos),
        }
    }
    for (pos, upload) in uploads.water.drain() {
        match upload {
            ChunkUpload::Set { version, data } => water_pass.set(pos, version, data.as_ref(), device),
            ChunkUpload::Remove => water_pass.remove(pos),
        }
    }
    for (pos, upload) in uploads.detail_paint.drain() {
        match upload {
            ChunkUpload::Set { version, data } => detail_paint.set(pos, version, data.as_ref(), device),
            ChunkUpload::Remove => detail_paint.remove(pos),
        }
    }
    for (pos, upload) in uploads.scatter.drain() {
        match upload {
            ChunkUpload::Set { version, data } => scatter.set(pos, version, data.as_deref(), device),
            ChunkUpload::Remove => scatter.remove(pos),
        }
    }
    for (pos, upload) in uploads.capsules.drain() {
        match upload {
            ChunkUpload::Set { version, data } => capsules.set(pos, version, data.as_deref(), device),
            ChunkUpload::Remove => capsules.remove(pos),
        }
    }
    for (pos, upload) in uploads.lobes.drain() {
        match upload {
            ChunkUpload::Set { version, data } => canopy.set(pos, version, data.as_deref(), device),
            ChunkUpload::Remove => canopy.remove(pos),
        }
    }
}

/// Resize the surface to the window's latest size, when the window reported one
/// since the last frame. The render targets follow on their own: the Render
/// stage recreates them when the frame's dims change.
pub fn surface_resize_system(
    ctx: Res<RenderContext>,
    mut requests: ResMut<RenderRequests>,
    mut surface: ResMut<SurfaceState>,
) {
    if let Some((width, height)) = requests.surface_size.take() {
        surface.resize(&ctx.device, width, height);
    }
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
