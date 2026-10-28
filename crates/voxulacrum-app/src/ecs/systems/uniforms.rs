//! Render-world systems that prepare a frame before it draws: the per-chunk
//! upload drain, the surface resize request, and the uniform buffers.

use bevy_ecs::prelude::*;
use crate::ecs::resources::{
    GlobalUniformBuffer, ReflectionUniformBuffer, RenderRequests, ShadowUniformBuffer,
};
use crate::meshing::coordinator::ChunkMeshes;
use crate::meshing::uploads::{ChunkUpload, ChunkUploads};
use crate::rendering::canopy_pass::CanopyPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::render_world::RenderParams;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::water_pass::WaterPass;
use crate::simulation::manager::FrameState;

/// Drain the render world's `ChunkUploads` into the terrain meshes and the
/// per-chunk passes. Runs before everything in the render schedule that reads
/// them: the stats, the uniforms (the water plane) and the draw.
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
/// since the last frame. The render targets follow on their own:
/// `render_present_system` recreates them when the frame's dims change.
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
    render_params: Res<RenderParams>,
    global_buf: Res<GlobalUniformBuffer>,
    shadow_buf: Res<ShadowUniformBuffer>,
    reflection_buf: Res<ReflectionUniformBuffer>,
    post_process: Res<PostProcessPass>,
    outline: Res<OutlinePass>,
    palette_pass: Res<PalettePass>,
    upscale: Res<UpscalePass>,
    water_pass: Res<WaterPass>,
) {
    let reflect_plane = water_pass.avg_surface_y;
    crate::rendering::uniform_writer::write_all_uniforms(
        &ctx,
        &frame,
        &render_params.params,
        &render_params.cloud,
        &global_buf.0,
        &shadow_buf.0,
        &reflection_buf.0,
        reflect_plane,
        &post_process,
        &outline,
        &palette_pass,
        &upscale,
        &water_pass,
        &render_params.palette,
        surface.surface_config.width,
        surface.surface_config.height,
    );
}
