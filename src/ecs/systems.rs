use std::path::PathBuf;

use bevy_ecs::prelude::*;
use glam::IVec3;

use crate::ecs::resources::*;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::params::ParamChangeKind;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::main_scene_pass::{CapConfig, MainScenePassNode};
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::pipelines::{PipelineRegistry, PipelineResources};
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_graph::{RenderGraph, ResourceId, ResourceMap};
use crate::rendering::render_targets::RenderTargets;
use crate::rendering::shadow_pass::ShadowPassNode;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::vegetation_pass::VegetationPass;
use crate::rendering::water_pass::WaterPass;
use crate::shader_reload::ShaderWatcher;
use crate::input::InputState;
use crate::simulation::manager::{FrameState, SimulationManager};
use crate::ui;
use crate::ui::panels::UiState;
use crate::world::regen::WorldRegenCoordinator;
use crate::{compute_render_dimensions, palette, FrameCounter};
use crate::meshing::MeshingPipeline;
use crate::world::chunk::{Chunk, CHUNK_WORLD_SIZE};
use crate::world::streaming::{CameraView, ChunkStreamingManager};
// ==========================================================================
// Input stage
// ==========================================================================

pub fn frame_counter_system(mut counter: ResMut<FrameCounter>, mut ui: ResMut<UiState>) {
    counter.tick();
    ui.fps = counter.fps;
}

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

// ==========================================================================
// Simulation stage
// ==========================================================================

pub fn simulation_tick_system(
    mut sim: ResMut<SimulationManager>,
    ui: Res<UiState>,
    render_targets: Res<RenderTargets>,
    input: Res<InputState>,
    mut commands: Commands,
) {
    let frame = sim.tick(&ui.params, &render_targets, &input);
    commands.insert_resource(frame);
}

pub fn param_change_detection_system(mut ui: ResMut<UiState>) {
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
        let path = PathBuf::from("palettes").join(format!("{}.json", name));
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

// ==========================================================================
// Streaming stage
// ==========================================================================

pub fn streaming_tick_system(
    mut streaming: ResMut<ChunkStreamingManager>,
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    sim: Res<SimulationManager>,
    frame: Res<FrameState>,
    mut vegetation: ResMut<VegetationPass>,
    mut water_pass: ResMut<WaterPass>,
    mut ui: ResMut<UiState>,
    ctx: Res<RenderContext>,
) {
    let cam_pos = sim.camera.smooth_target;
    let cam_cx = (cam_pos.x / CHUNK_WORLD_SIZE).floor() as i32;
    let cam_cz = (cam_pos.z / CHUNK_WORLD_SIZE).floor() as i32;
    let camera_view = CameraView {
        zoom: sim.camera.zoom,
        aspect: sim.camera.aspect,
        rotation: sim.camera.rotation,
        camera_chunk: IVec3::new(cam_cx, 0, cam_cz),
        camera_world_pos: cam_pos,
    };

    let tick_result = streaming.tick(
        &mut world.0,
        &mut meshing,
        &camera_view,
        frame.dt,
    );

    // Remove vegetation and water for unloaded chunks
    for pos in &tick_result.unloaded {
        vegetation.remove_chunk_vegetation(*pos);
        water_pass.remove_chunk_water(*pos);
    }

    // NOTE: We do NOT add vegetation/water here for inserted chunks.
    // Inserted chunks don't have meshes yet. Vegetation/water are added
    // in meshing_tick_system when the mesh upload completes, so they
    // appear on the same frame as the terrain.

    // Update streaming stats for UI
    ui.streaming_loaded = world.0.chunks.len() as u32;
    ui.streaming_pending = streaming.pending_gen_count() as u32;
}

// ==========================================================================
// Meshing stage
// ==========================================================================

pub fn meshing_tick_system(
    mut meshing: ResMut<MeshingCoordinator>,
    mut world: ResMut<VoxelWorld>,
    ctx: Res<RenderContext>,
    mut ui: ResMut<UiState>,
    mut vegetation: ResMut<VegetationPass>,
    mut water_pass: ResMut<WaterPass>,
) {
    // Phase 1: tick meshing with &mut world - collects meshed positions.
    let meshed = meshing.tick(&mut world.0, &ctx, &mut ui);

    // Phase 2: for each newly meshed chunk, build per-chunk vegetation
    // and water GPU buffers. world.0 is now borrowed immutably.
    if !meshed.is_empty() {
        let water_level = ui.params.water.water_level;
        let terrain_params = &ui.params.terrain_gen;
        let veg_params = &ui.params.vegetation;

        for pos in &meshed {
            vegetation.add_chunk_vegetation(
                *pos, &world.0, veg_params, &ctx.device,
            );
            water_pass.add_chunk_water(
                *pos, &world.0.generator, terrain_params, water_level, &ctx.device,
            );
        }
    }
}

// ==========================================================================
// UniformWrite stage
// ==========================================================================

pub fn write_uniforms_system(
    ctx: Res<RenderContext>,
    surface: Res<SurfaceState>,
    frame: Res<FrameState>,
    ui: Res<UiState>,
    global_buf: Res<GlobalUniformBuffer>,
    shadow_buf: Res<ShadowUniformBuffer>,
    post_process: Res<PostProcessPass>,
    outline: Res<OutlinePass>,
    palette_pass: Res<PalettePass>,
    upscale: Res<UpscalePass>,
    render_targets: Res<RenderTargets>,
    loaded_palette: Res<LoadedPalette>,
) {
    crate::rendering::uniform_writer::write_all_uniforms(
        &ctx,
        &frame,
        &ui.params,
        &global_buf.0,
        &shadow_buf.0,
        &post_process,
        &outline,
        &palette_pass,
        &upscale,
        &render_targets,
        &loaded_palette.0,
        surface.surface_config.width,
        surface.surface_config.height,
    );
}

pub fn compute_stats_system(
    world: Res<VoxelWorld>,
    sim: Res<SimulationManager>,
    frame: Res<FrameState>,
    mut ui: ResMut<UiState>,
) {
    ui.frame_time_ms = frame.dt * 1000.0;
    let mut total_tris: u64 = 0;
    let mut chunks_visible: u32 = 0;
    let mut chunks_total: u32 = 0;
    for chunk in world.0.chunks.values() {
        if chunk.mesh.is_some() {
            chunks_total += 1;
            if sim.frustum.is_chunk_visible(chunk.position) {
                chunks_visible += 1;
                total_tris += chunk.mesh.as_ref().unwrap().index_count as u64 / 3;
            }
        }
    }
    ui.total_triangles = total_tris;
    ui.chunks_visible = chunks_visible;
    ui.chunks_total = chunks_total;
}

// ==========================================================================
// Render stage — exclusive system (takes &mut World)
// ==========================================================================

pub fn render_present_system(ecs: &mut bevy_ecs::world::World) {
    // Clone is cheap — Device/Queue are Arc handles internally.
    // This avoids all borrow conflicts with resource_mut calls below.
    let ctx = ecs.resource::<RenderContext>().clone();

    // --- Render target resize check ---
    let (rw, rh, eff_scale) = {
        let surface = ecs.resource::<SurfaceState>();
        let sim = ecs.resource::<SimulationManager>();
        let ui = ecs.resource::<UiState>();
        compute_render_dimensions(
            surface.surface_config.width,
            surface.surface_config.height,
            sim.camera.zoom,
            ui.params.render_pipeline.world_pixel_density,
        )
    };

    if ecs.resource::<RenderTargets>().needs_recreate(rw, rh) {
        let new_rt = RenderTargets::new(&ctx, rw, rh, eff_scale);

        ecs.resource_mut::<OutlinePass>().rebuild_bind_group(
            &ctx,
            &new_rt.scene_view,
            &new_rt.depth_view,
            &new_rt.normal_view,
        );
        ecs.resource_mut::<PostProcessPass>().rebuild_bind_group(
            &ctx,
            &new_rt.processed_view,
            &new_rt.depth_view,
        );
        ecs.resource_mut::<PalettePass>().rebuild_bind_group(
            &ctx,
            &new_rt.scene_view,
        );
        ecs.resource_mut::<UpscalePass>().rebuild_bind_group(
            &ctx,
            &new_rt.processed_view,
        );

        *ecs.resource_mut::<RenderTargets>() = new_rt;
    }

    // --- Acquire swapchain ---
    let surface_result = ecs.resource::<SurfaceState>().surface.get_current_texture();
    let surface_frame = match surface_result {
        Ok(f) => f,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            let size = ecs.resource::<WindowHandle>().0.inner_size();
            ecs.resource_mut::<SurfaceState>().resize(&ctx.device, size.width, size.height);
            return;
        }
        Err(wgpu::SurfaceError::Timeout) => {
            log::warn!("Surface timeout");
            return;
        }
        Err(e) => {
            log::error!("Surface error: {:?}", e);
            return;
        }
    };

    let surface_view = surface_frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    // --- Debug lines update ---
    {
        let show_debug = ecs.resource::<UiState>().params.debug.show_chunk_boundaries;
        if show_debug {
            let chunk_positions: Vec<IVec3> = ecs
                .resource::<VoxelWorld>()
                .0
                .chunks
                .keys()
                .copied()
                .collect();
            ecs.resource_scope::<DebugLinePass, _>(|ecs, mut debug| {
                let sim = ecs.resource::<SimulationManager>();
                debug.update(&ctx, &chunk_positions, &sim.frustum);
            });
        }
    }

    // --- Cap mesh update ---
    let (cap_enabled, cos_r, sin_r, clip_max, clip_min) = {
        let frame = ecs.resource::<FrameState>();
        let cs_enabled = ecs.resource::<UiState>().params.cross_section.enabled;
        (
            cs_enabled && frame.clip_enabled != 0,
            frame.cos_r,
            frame.sin_r,
            frame.clip_max,
            frame.clip_min,
        )
    };

    if cap_enabled {
        let clip_dirs: [f32; 3] = [
            if cos_r >= 0.0 { 1.0 } else { -1.0 },
            1.0,
            if sin_r >= 0.0 { 1.0 } else { -1.0 },
        ];
        let clip_pos = [
            if clip_dirs[0] > 0.0 { clip_max[0] } else { clip_min[0] },
            clip_max[1],
            if clip_dirs[2] > 0.0 { clip_max[2] } else { clip_min[2] },
        ];
        ecs.resource_scope::<CapPass, _>(|ecs, mut cap| {
            let world = ecs.resource::<VoxelWorld>();
            let ui = ecs.resource::<UiState>();
            cap.maybe_rebuild(&ctx, &world.0, &ui.params.cross_section, clip_pos, clip_dirs);
        });
    }

    // --- Build and execute render graph ---
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render_encoder"),
    });

    {
        let ui = ecs.resource::<UiState>();
        let world = ecs.resource::<VoxelWorld>();
        let sim = ecs.resource::<SimulationManager>();
        let pipeline_registry = ecs.resource::<PipelineRegistry>();
        let shadow_bind_group = ecs.resource::<ShadowBindGroup>();
        let shadow_depth_view = ecs.resource::<ShadowDepthView>();
        let uniform_bind_group = ecs.resource::<GlobalUniformBindGroup>();
        let render_targets = ecs.resource::<RenderTargets>();
        let vegetation_pass = ecs.resource::<VegetationPass>();
        let water_pass = ecs.resource::<WaterPass>();
        let debug_line_pass = ecs.resource::<DebugLinePass>();
        let cap_pass = ecs.resource::<CapPass>();
        let outline_pass = ecs.resource::<OutlinePass>();
        let post_process = ecs.resource::<PostProcessPass>();
        let palette_pass = ecs.resource::<PalettePass>();
        let upscale_pass = ecs.resource::<UpscalePass>();

        let terrain_pipeline = if ui.params.debug.show_wireframe {
            &pipeline_registry.terrain_wireframe_pipeline
        } else {
            &pipeline_registry.terrain_pipeline
        };

        // Collect chunk references from HashMap for render passes
        let chunk_refs: Vec<&Chunk> = world.0.chunks.values().collect();

        let shadow_node = ShadowPassNode {
            pipeline: &pipeline_registry.shadow_pipeline,
            bind_group: &shadow_bind_group.0,
            shadow_depth_view: &shadow_depth_view.0,
            chunks: &chunk_refs,
        };

        let sky = ui.params.render_pipeline.sky_color;
        let scene_node = MainScenePassNode {
            sky_color: wgpu::Color {
                r: sky[0] as f64,
                g: sky[1] as f64,
                b: sky[2] as f64,
                a: 1.0,
            },
            terrain_pipeline,
            uniform_bind_group: &uniform_bind_group.0,
            chunks: &chunk_refs,
            frustum: &sim.frustum,
            cap_pass: &cap_pass,
            cap_config: CapConfig {
                enabled: cap_enabled,
                clip_dirs: [
                    if cos_r >= 0.0 { 1.0 } else { -1.0 },
                    1.0,
                    if sin_r >= 0.0 { 1.0 } else { -1.0 },
                ],
                clip_pos: [0.0; 3],
            },
            vegetation_pass: &vegetation_pass,
            vegetation_pipeline: &pipeline_registry.vegetation_pipeline,
            water_pass: &water_pass,
            water_pipeline: &pipeline_registry.water_pipeline,
            debug_line_pass: &debug_line_pass,
            show_debug_lines: ui.params.debug.show_chunk_boundaries,
        };

        let mut graph = RenderGraph::new();
        graph.add_pass(&shadow_node);
        graph.add_pass(&scene_node);
        graph.add_pass(&*outline_pass);
        graph.add_pass(&*post_process);
        graph.add_pass(&*palette_pass);
        graph.add_pass(&*upscale_pass);

        #[cfg(debug_assertions)]
        graph
            .validate(&[
                ResourceId::SHADOW_DEPTH,
                ResourceId::SCENE,
                ResourceId::NORMAL,
                ResourceId::DEPTH,
                ResourceId::PROCESSED,
                ResourceId::SURFACE,
            ])
            .unwrap();

        let mut resources = ResourceMap::new();
        resources.insert(ResourceId::SCENE, &render_targets.scene_view);
        resources.insert(ResourceId::NORMAL, &render_targets.normal_view);
        resources.insert(ResourceId::DEPTH, &render_targets.depth_view);
        resources.insert(ResourceId::PROCESSED, &render_targets.processed_view);
        resources.insert(ResourceId::SHADOW_DEPTH, &shadow_depth_view.0);
        resources.insert(ResourceId::SURFACE, &surface_view);

        graph.execute(&mut encoder, &resources);
    }

    // --- Egui draw ---
    let window_arc = ecs.resource::<WindowHandle>().0.clone();
    ecs.resource_scope::<UiState, _>(|ecs, mut ui_state| {
        let mut egui = ecs.non_send_resource_mut::<ui::EguiRenderer>();
        egui.draw(
            &mut *ui_state,
            &ctx,
            &mut encoder,
            &surface_view,
            &window_arc,
        );
    });

    // --- Submit + present ---
    ctx.queue.submit(std::iter::once(encoder.finish()));
    surface_frame.present();
}

// ==========================================================================
// PostFrame stage
// ==========================================================================

pub fn world_regen_system(
    mut regen: ResMut<WorldRegenCoordinator>,
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    mut vegetation: ResMut<VegetationPass>,
    mut water_pass: ResMut<WaterPass>,
    mut ui: ResMut<UiState>,
    ctx: Res<RenderContext>,
) {
    regen.tick(
        &mut world.0,
        &mut meshing,
        &mut vegetation,
        &mut water_pass,
        &mut ui,
        &ctx,
    );
}
