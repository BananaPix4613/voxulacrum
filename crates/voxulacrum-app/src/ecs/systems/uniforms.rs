//! Writes this frame's uniform buffers.

use bevy_ecs::prelude::*;
use crate::ecs::resources::{
    GlobalUniformBuffer, LoadedPalette, ReflectionUniformBuffer, ShadowUniformBuffer,
};
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::render_targets::RenderTargets;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::water_pass::WaterPass;
use crate::simulation::manager::{FrameState, SimulationManager};
use crate::ui::panels::UiState;

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
    render_targets: Res<RenderTargets>,
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
        &render_targets,
        &loaded_palette.0,
        surface.surface_config.width,
        surface.surface_config.height,
    );
}
