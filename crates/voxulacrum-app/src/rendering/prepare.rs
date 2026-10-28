//! Render-world systems that prepare a frame before it draws: the surface
//! resize request, shader reloads, the per-chunk upload drain, and the uniform
//! buffers.

use bevy_ecs::prelude::*;
use crate::rendering::canopy_pass::CanopyPass;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::rendering::chunk_meshes::ChunkMeshes;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::frame_state::FrameState;
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::pipelines::{PipelineRegistry, PipelineResources};
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::render_world::{RenderFeedback, RenderParams};
use crate::rendering::resources::{
    GlobalUniformBuffer, ReflectionUniformBuffer, RenderRequests, ShadowUniformBuffer,
};
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::uploads::{ChunkUpload, ChunkUploads};
use crate::rendering::water_pass::WaterPass;

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

/// Rebuild the pipelines of the shaders `shader_watch_system` queued. Runs in
/// the render world, before the frame draws; its log lines reach the panel
/// through `RenderFeedback`.
pub fn shader_reload_system(
    mut requests: ResMut<RenderRequests>,
    ctx: Res<RenderContext>,
    mut pipeline_registry: ResMut<PipelineRegistry>,
    pipeline_resources: Res<PipelineResources>,
    mut post_process: ResMut<PostProcessPass>,
    mut outline_pass: ResMut<OutlinePass>,
    mut palette_pass: ResMut<PalettePass>,
    mut cap_pass: ResMut<CapPass>,
    mut feedback: ResMut<RenderFeedback>,
) {
    let changed = std::mem::take(&mut requests.shader_changes);
    for path in changed {
        let filename = match path.file_name().and_then(|f| f.to_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };

        if filename == "post_process.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match post_process.try_reload_shader(&ctx, &source) {
                        Ok(()) => {
                            log::info!("Reloaded post_process.wgsl");
                            feedback.push_shader_log("post_process.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("post_process.wgsl compile error: {}", e);
                            feedback.push_shader_log(format!("post_process.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read post_process.wgsl: {}", e);
                    feedback.push_shader_log(format!("post_process.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        if filename == "outline.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match outline_pass.try_reload_shader(&ctx, &source) {
                        Ok(()) => {
                            log::info!("Reloaded outline.wgsl");
                            feedback.push_shader_log("outline.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("outline.wgsl compile error: {}", e);
                            feedback.push_shader_log(format!("outline.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read outline.wgsl: {}", e);
                    feedback.push_shader_log(format!("outline.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        if filename == "palette.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match palette_pass.try_reload_shader(&ctx, &source) {
                        Ok(()) => {
                            log::info!("Reloaded palette.wgsl");
                            feedback.push_shader_log("palette.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("palette.wgsl compile error: {}", e);
                            feedback.push_shader_log(format!("palette.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read palette.wgsl: {}", e);
                    feedback.push_shader_log(format!("palette.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        if filename == "cap.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match cap_pass.try_reload_shader(
                        &ctx,
                        &pipeline_resources.global_bind_group_layout,
                        &source,
                    ) {
                        Ok(()) => {
                            log::info!("Reloaded cap.wgsl");
                            feedback.push_shader_log("cap.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("cap.wgsl compile error: {}", e);
                            feedback.push_shader_log(format!("cap.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read cap.wgsl: {}", e);
                    feedback.push_shader_log(format!("cap.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        // All other shaders go through the pipeline registry
        if let Some(result) = pipeline_registry.try_reload(
            &ctx,
            &pipeline_resources,
            &path,
        ) {
            if result.success {
                log::info!("Reloaded {}", result.filename);
            } else {
                log::warn!("{}: {}", result.filename, result.message);
            }
            feedback.push_shader_log(
                format!("{}: {}", result.filename, result.message),
                !result.success,
            );
        }
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
        &render_params.settings,
        &render_params.scene,
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
