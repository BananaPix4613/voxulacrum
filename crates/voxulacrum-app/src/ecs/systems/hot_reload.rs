//! Reloads from disk and from panel requests: shaders, world graphs, the
//! selected palette, and parameter change detection.

use bevy_ecs::prelude::*;
use crate::diagnostics::{FrameTimings, SUB_PARAMS};
use crate::ecs::resources::LoadedPalette;
use crate::graph_reload::GraphWatcherRes;
use crate::palette;
use crate::params::ParamChangeKind;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::pipelines::{PipelineRegistry, PipelineResources};
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_context::RenderContext;
use crate::shader_reload::ShaderWatcher;
use crate::ui::panels::UiState;

pub fn shader_hot_reload_system(
    shader_watcher: Res<ShaderWatcher>,
    ctx: Res<RenderContext>,
    mut pipeline_registry: ResMut<PipelineRegistry>,
    pipeline_resources: Res<PipelineResources>,
    mut post_process: ResMut<PostProcessPass>,
    mut outline_pass: ResMut<OutlinePass>,
    mut palette_pass: ResMut<PalettePass>,
    mut cap_pass: ResMut<CapPass>,
    mut ui: ResMut<UiState>,
) {
    let changed = shader_watcher.poll_changes();
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
                            ui.push_shader_log("post_process.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("post_process.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("post_process.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read post_process.wgsl: {}", e);
                    ui.push_shader_log(format!("post_process.wgsl read error: {}", e), true);
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
                            ui.push_shader_log("outline.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("outline.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("outline.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read outline.wgsl: {}", e);
                    ui.push_shader_log(format!("outline.wgsl read error: {}", e), true);
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
                            ui.push_shader_log("palette.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("palette.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("palette.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read palette.wgsl: {}", e);
                    ui.push_shader_log(format!("palette.wgsl read error: {}", e), true);
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
                            ui.push_shader_log("cap.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("cap.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("cap.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read cap.wgsl: {}", e);
                    ui.push_shader_log(format!("cap.wgsl read error: {}", e), true);
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
            ui.push_shader_log(
                format!("{}: {}", result.filename, result.message),
                !result.success,
            );
        }
    }
}

/// Watch graph files and announce what changed.
///
/// It writes an event and stops there. Before the bus it pushed onto a queue in
/// `UiState` that the regeneration coordinator drained - two systems reaching
/// into a third's state to talk to each other. Now the watcher does not know
/// regeneration exists.
pub fn graph_hot_reload_system(
    graph_watcher: Res<GraphWatcherRes>,
    mut events: EventWriter<crate::world::events::WorldEvent>
) {
    let changed = graph_watcher.0.poll_changes();
    if changed.is_empty() {
        return;
    }
    let manifest = crate::world::world_generator::world_manifest_path();
    for path in changed {
        match crate::world::world_generator::slot_for_graph_file(&manifest, &path) {
            Some(slot) => {
                events.write(crate::world::events::WorldEvent::GraphChanged { slot });
                log::info!("graph changed on disk: {} ({slot:?})", path.display());
            }
            // A `.json` in the graphs directory that the manifest does not
            // reference cannot affect generation.
            None => log::debug!("ignoring unreferenced graph file {}", path.display()),
        }
    }
}

pub fn param_change_detection_system(
    mut ui: ResMut<UiState>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_PARAMS);
    let kind = ui.change_detector.detect(&ui.params);
    if kind == ParamChangeKind::MeshInvalidating {
        ui.mesh_params_pending = true;
    }
    let params_clone = ui.params.clone();
    ui.change_detector.snapshot(&params_clone);
}

pub fn palette_load_system(
    mut ui: ResMut<UiState>,
    mut loaded: ResMut<LoadedPalette>,
) {
    if !ui.palette_load_requested {
        return;
    }
    ui.palette_load_requested = false;
    let name = &ui.params.palette.selected_palette;
    if !name.is_empty() {
        let path = crate::paths::asset_root().join("palettes").join(format!("{}.json", name));
        match palette::load_palette(&path) {
            Ok(pal) => {
                log::info!("Loaded palette: {}", pal.name);
                ui.loaded_palette_preview = pal.colors_srgb.clone();
                loaded.0 = Some(pal);
            }
            Err(e) => {
                log::warn!("Failed to load palette {}: {}", name, e);
                loaded.0 = None;
                ui.loaded_palette_preview.clear();
            }
        }
    } else {
        loaded.0 = None;
        ui.loaded_palette_preview.clear();
    }
}
